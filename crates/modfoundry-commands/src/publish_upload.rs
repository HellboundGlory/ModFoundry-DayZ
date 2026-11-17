//! The Workshop upload: create the item, write its id into the mod, send the update, and undo
//! the id when the update fails.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use modfoundry_build::publish::details::{read_details, write_details};
use modfoundry_build::publish::tags::KNOWN_KEYS;
use modfoundry_core::{Diagnostic, Severity};
use modfoundry_platform::steam::{
    CancelToken, ItemUpdate, SteamAccount, SteamEvent, SteamRequest, SteamRun, UploadStage,
};
use modfoundry_vfs::{root_name, write::WriteTxn, ChangeSet};
use serde::{Deserialize, Serialize};

use crate::publish_ops::{prepare, stage_meta, PrepareArgs, UploadPlan};

/// What the running upload is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum UploadState {
    Preparing,
    Creating,
    Uploading {
        stage: UploadStage,
        done: u64,
        total: u64,
    },
    Deleting,
}

/// The running upload's state and cancel flag; one upload at a time.
#[derive(Default)]
pub struct PublishStatus {
    state: Mutex<Option<UploadState>>,
    cancel: Mutex<CancelToken>,
}

impl PublishStatus {
    /// The state of the running upload, or `None` when none runs.
    pub fn current(&self) -> Option<UploadState> {
        *self.lock()
    }

    /// Stops the running upload; answers whether one was running.
    pub fn cancel(&self) -> bool {
        let state = self.lock();
        if state.is_none() {
            return false;
        }
        self.cancel_lock().cancel();
        true
    }

    // A panic while the state was held must not take the app down with it.
    fn lock(&self) -> MutexGuard<'_, Option<UploadState>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancel_lock(&self) -> MutexGuard<'_, CancelToken> {
        self.cancel.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(&self) -> anyhow::Result<StatusGuard<'_>> {
        let mut state = self.lock();
        if state.is_some() {
            anyhow::bail!("an upload is already running");
        }
        *state = Some(UploadState::Preparing);
        *self.cancel_lock() = CancelToken::default();
        Ok(StatusGuard { status: self })
    }

    fn set(&self, state: UploadState) {
        *self.lock() = Some(state);
    }

    /// The running upload's token; a clone shares the flag with the runner.
    fn token(&self) -> CancelToken {
        self.cancel_lock().clone()
    }
}

struct StatusGuard<'a> {
    status: &'a PublishStatus,
}

impl Drop for StatusGuard<'_> {
    fn drop(&mut self) {
        *self.status.lock() = None;
    }
}

/// The arguments of `publish.upload`: the build to send and the note for the change log.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadArgs {
    #[serde(flatten)]
    pub prepare: PrepareArgs,
    #[serde(default)]
    pub change_note: Option<String>,
}

/// The Workshop item an upload reached.
#[derive(Debug, Clone, Serialize)]
pub struct UploadResult {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub new_item: bool,
    pub account: SteamAccount,
    pub needs_legal_agreement: bool,
    pub warnings: Vec<Diagnostic>,
}

/// Sends a prepared build to the Workshop through `steam`.
pub fn upload(
    args: &UploadArgs,
    steam: &dyn SteamRun,
    status: &PublishStatus,
    now: Duration,
) -> anyhow::Result<UploadResult> {
    let _guard = status.begin()?;
    let cancel = status.token();
    let plan = prepare(&args.prepare, now)?;
    if !plan.ready {
        anyhow::bail!("not published: {}", error_list(&plan.diagnostics));
    }
    if cancel.is_cancelled() {
        anyhow::bail!("not published: cancelled");
    }
    let account = expect_account(steam)?;
    if cancel.is_cancelled() {
        anyhow::bail!("not published: cancelled");
    }
    let mut needs_legal_agreement = false;
    let mut id_change = None;
    let id = match plan.id {
        Some(id) => id,
        None => {
            status.set(UploadState::Creating);
            let (id, legal) = create_item(steam)?;
            needs_legal_agreement |= legal;
            let change = write_id(&plan, id, now).map_err(|error| {
                let error = anyhow::anyhow!("could not record the new item's id: {error:#}");
                delete_new_item(steam, status, id, None, error)
            })?;
            id_change = Some(change);
            id
        }
    };
    needs_legal_agreement |=
        send_update(steam, status, &cancel, &plan, id, args.change_note.clone()).map_err(
            |error| match &id_change {
                Some(change) => {
                    let error = anyhow::anyhow!("the upload failed: {error:#}");
                    delete_new_item(steam, status, id, Some(change), error)
                }
                None => anyhow::anyhow!("the upload failed: {error:#}"),
            },
        )?;
    Ok(UploadResult {
        id,
        title: plan.title.clone(),
        url: item_url(id),
        new_item: plan.id.is_none(),
        account,
        needs_legal_agreement,
        warnings: plan.diagnostics,
    })
}

fn expect_account(steam: &dyn SteamRun) -> anyhow::Result<SteamAccount> {
    match steam.run(&SteamRequest::Account, &mut |_, _, _| {})? {
        SteamEvent::Account(account) => Ok(account),
        other => Err(unexpected(other)),
    }
}

fn create_item(steam: &dyn SteamRun) -> anyhow::Result<(u64, bool)> {
    match steam.run(&SteamRequest::Create, &mut |_, _, _| {})? {
        SteamEvent::Created {
            id,
            needs_legal_agreement,
        } => Ok((id, needs_legal_agreement)),
        other => Err(unexpected(other)),
    }
}

/// The build's `meta.cpp` goes first: it is regenerable, so a failure leaves nothing to undo.
fn write_id(plan: &UploadPlan, id: u64, now: Duration) -> anyhow::Result<ChangeSet> {
    stage_meta(&plan.content_dir, id, &plan.title, now)?;
    let mut details = read_details(&plan.mod_root)?;
    details.item.id = Some(id);
    let mut txn = WriteTxn::new(root_name("mod", &plan.mod_root), &plan.mod_root);
    write_details(&mut txn, &details.item, &details.description)?;
    Ok(txn.finish()?)
}

/// Removes a new item after a failure; while it still exists on Steam the mod keeps its id, so the
/// next upload updates it instead of creating another.
fn delete_new_item(
    steam: &dyn SteamRun,
    status: &PublishStatus,
    id: u64,
    change: Option<&ChangeSet>,
    error: anyhow::Error,
) -> anyhow::Error {
    status.set(UploadState::Deleting);
    let deleted = match steam.run(&SteamRequest::Delete { id }, &mut |_, _, _| {}) {
        Ok(SteamEvent::Deleted { .. }) => Ok(()),
        Ok(other) => Err(unexpected(other)),
        Err(e) => Err(e.into()),
    };
    if let Err(e) = deleted {
        let next = match change {
            Some(_) => "the mod keeps its id, so the next upload updates it",
            None => "delete it on its Steam page",
        };
        return anyhow::anyhow!("{error}; the new item {id} could not be deleted ({e}); {next}");
    }
    match change.map(ChangeSet::undo) {
        Some(Err(e)) => anyhow::anyhow!(
            "{error}; the new item {id} was deleted, but its id is still in workshop/item.json ({e}); remove it before the next upload"
        ),
        _ => anyhow::anyhow!("{error}; the new item {id} was deleted"),
    }
}

fn send_update(
    steam: &dyn SteamRun,
    status: &PublishStatus,
    cancel: &CancelToken,
    plan: &UploadPlan,
    id: u64,
    change_note: Option<String>,
) -> anyhow::Result<bool> {
    let update = item_update(plan, id, change_note);
    let event = steam.run_cancellable(
        &SteamRequest::Update(update),
        &mut |stage, done, total| {
            status.set(UploadState::Uploading { stage, done, total });
        },
        cancel,
    )?;
    match event {
        SteamEvent::Submitted {
            needs_legal_agreement,
            ..
        } => Ok(needs_legal_agreement),
        other => Err(unexpected(other)),
    }
}

/// The fields to send; a content-only upload leaves the item's own fields to Steam.
fn item_update(plan: &UploadPlan, id: u64, change_note: Option<String>) -> ItemUpdate {
    let fields = (!plan.content_only).then_some(plan);
    ItemUpdate {
        id,
        content_dir: plan.content_dir.clone(),
        preview: fields.and_then(|p| p.preview.clone()),
        title: fields.map(|p| p.title.clone()),
        description: fields.map(|p| p.description.clone()),
        visibility: fields.and_then(|p| p.visibility),
        tags: fields.map(|p| p.tags.clone()),
        remove_keys: KNOWN_KEYS.iter().map(|key| key.to_string()).collect(),
        kv_tags: plan.kv_tags.clone(),
        change_note,
    }
}

fn item_url(id: u64) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

fn error_list(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn unexpected(event: SteamEvent) -> anyhow::Error {
    anyhow::anyhow!("unexpected Steam answer: {event:?}")
}
