//! The broker: where a call is authorized and where every outcome is recorded.
//!
//! `Core` gates by what an actor may do *by default*; the persisted
//! policy is the outer ring that can grant more or take it away. The
//! broker sits between them — it is the ring above the registry, so
//! `modfoundry-core` stays IO-free — and its shape is one call:
//!
//! ```text
//! authorize(policy) → Core::dispatch → audit
//! ```
//!
//! Two properties are load-bearing. A call the policy refuses never reaches the
//! body, and *every* dispatch lands in the audit trail — a refused agent call is
//! the most interesting row in it. The one exception is a quiet `Command`
//! (live editor lint): its runs are not recorded, while a refusal of it
//! still is. The authorization travels to the gate as an
//! [`Actor::with_grant`], so the gate in `Core::dispatch` still does the last
//! check rather than being bypassed.

use crate::actor::{actor_key, validate_actor_key};
use crate::confirm::{Confirm, ConfirmRequest};
use modfoundry_core::{
    is_preview, Actor, Command, CommandError, Core, EffectKind, Outcome, HOST_UPDATE,
    WORKSPACE_UNDO,
};
use modfoundry_store::audit::{AuditEntry, AuditOutcome, NewAuditEntry};
use modfoundry_store::confirm::ConfirmAnswer;
use modfoundry_store::policy::{Decision, ANY};
use modfoundry_store::undo::UndoRecord;
use modfoundry_store::{Store, StoreError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

/// Errors a brokered dispatch surfaces to its frontend.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("command not found: {0}")]
    NotFound(String),
    /// Refused by the permission policy, before the body ran.
    #[error("command rejected by permission policy: {0}")]
    Denied(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// A change-set that [`Broker::undo`] reversed.
#[derive(Debug)]
pub struct Undone {
    /// `undo_log.id` of the reversed change-set.
    pub change_set: i64,
    /// Whether *this* call is what reversed it; `false` means another session
    /// had already replayed the same change-set.
    pub first: bool,
    /// What the inversion reported.
    pub outcome: Outcome,
}

/// What the policy says about one call.
enum Verdict {
    Allow,
    Deny(String),
    /// The policy wants a human: only the prompting paths may resolve this.
    Ask,
}

/// A verdict after this session's remembered answers and its hook have spoken.
/// `Ask` cannot reach here, so the no-prompt guarantee is a type, not a promise.
enum Settled {
    Allow,
    Deny(String),
}

/// Authorizes and audits every dispatch over one command registry.
pub struct Broker {
    core: Core,
    store: Store,
    session_mutations: bool,
    confirm: Option<Box<dyn Confirm>>,
    /// The (actor, command, effect) triples this session may keep running on
    /// one `Session` answer.
    session_allowed: RefCell<HashSet<(String, String, EffectKind)>>,
}

impl Broker {
    /// A broker over a fresh in-memory store: the policy and trail live for this
    /// process only. What an agent host gets when it is not pointed at a
    /// database.
    pub fn in_memory(core: Core) -> Result<Self, StoreError> {
        Ok(Self::with_store(core, Store::open_in_memory()?))
    }

    /// A broker backed by the SQLite database at `db`,
    /// so grants and the audit trail outlive the process.
    pub fn open(core: Core, db: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self::with_store(core, Store::open(db)?))
    }

    /// A broker over a store the caller already holds (both `Store` handles then
    /// address the same database file).
    pub fn with_store(core: Core, store: Store) -> Self {
        Self {
            core,
            store,
            session_mutations: false,
            confirm: None,
            session_allowed: RefCell::new(HashSet::new()),
        }
    }

    /// Install the hook that answers policy `ask` rules. Without one an `ask` is
    /// refused outright; nothing is ever approved by default.
    pub fn set_confirm(&mut self, c: Box<dyn Confirm>) {
        self.confirm = Some(c);
    }

    /// Grant `Mutate` and `Launch` for this broker instance only — the
    /// `--allow-mutations` switch. A persisted `deny` still outranks it, and
    /// `Destructive`, `Network` and `Publish` are never implied: those need a
    /// rule.
    pub fn allow_session_mutations(&mut self) {
        self.session_mutations = true;
    }

    /// The registry this broker dispatches over.
    pub fn core(&self) -> &Core {
        &self.core
    }

    /// Dispatch over `core` from now on; the store and the policy stay as they are.
    pub fn replace_core(&mut self, core: Core) {
        self.core = core;
    }

    /// The store holding this broker's policy and audit trail.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Whether `actor` may run `id` right now. Answers from the policy without
    /// touching the body and without writing an audit row. An unregistered id is
    /// not runnable (use [`authorize`](Self::authorize) to tell "unknown" from
    /// "refused").
    pub fn can_run(&self, actor: Actor, id: &str) -> Result<bool, AgentError> {
        let Some(command) = self.core.command(id) else {
            return Ok(false);
        };
        self.can_run_as(actor, id, command.spec().effect)
    }

    /// Whether `actor` may run `id` *as* `effect`. A preview is authorized as a
    /// read, so a frontend can ask the one question that decides whether a
    /// mutation an agent may not run is still worth advertising to it.
    ///
    /// Never prompts: an `ask` rule answers `true` exactly when this session has
    /// already been told to allow it, or a hook exists that *could* answer it.
    pub fn can_run_as(
        &self,
        actor: Actor,
        id: &str,
        effect: EffectKind,
    ) -> Result<bool, AgentError> {
        if self.core.command(id).is_none() {
            return Ok(false);
        }
        match self.verdict(actor, id, effect)? {
            Verdict::Allow => Ok(true),
            Verdict::Deny(_) => Ok(false),
            Verdict::Ask => Ok(self.session_allows(actor, id, effect) || self.confirm.is_some()),
        }
    }

    /// The effect class `id` would run as, or the reason `actor` may not run it.
    /// Unlike [`can_run`](Self::can_run) this may put an `ask` rule to the
    /// session's confirmation hook.
    pub fn authorize(&self, actor: Actor, id: &str) -> Result<EffectKind, AgentError> {
        let cmd = self
            .core
            .command(id)
            .ok_or_else(|| AgentError::NotFound(id.to_owned()))?;
        let effect = cmd.spec().effect;
        match self.settle(actor, id, effect, &Value::Null)? {
            Settled::Allow => Ok(effect),
            Settled::Deny(reason) => Err(AgentError::Denied(reason)),
        }
    }

    /// Authorize, run, and record the result against the current time.
    pub fn dispatch(&self, actor: Actor, id: &str, args: Value) -> Result<Outcome, AgentError> {
        self.dispatch_at(actor, id, args, now_unix())
    }

    /// [`dispatch`](Self::dispatch) with the operation's timestamp supplied by
    /// the caller, so the trail records when the call was dispatched (not when
    /// it was persisted) and tests stay deterministic.
    ///
    /// A `{"preview": true}` call is authorized as a read (it returns the
    /// change-set without running the body), and a mutation that reports a
    /// change-set has it recorded on the undo stack, referenced from its audit
    /// row. `workspace.undo` is refused for an agent or automation: its
    /// change-set comes from the undo log, so those call [`undo`](Self::undo).
    pub fn dispatch_at(
        &self,
        actor: Actor,
        id: &str,
        args: Value,
        at_unix: i64,
    ) -> Result<Outcome, AgentError> {
        let cmd = self.command(id)?;
        // A caller-supplied change-set names its own host root, so only the host
        // path, which reads it from the undo log, may replay one.
        if id == WORKSPACE_UNDO && matches!(actor, Actor::Agent { .. } | Actor::Automation { .. }) {
            let reason = "workspace.undo replays the undo log; call Broker::undo".to_owned();
            return self.refuse(actor, id, effect_of(cmd, &args), &args, at_unix, reason);
        }
        self.run(actor, id, cmd, args, at_unix)
    }

    fn command(&self, id: &str) -> Result<&dyn Command, AgentError> {
        self.core
            .command(id)
            .ok_or_else(|| AgentError::NotFound(id.to_owned()))
    }

    /// Authorize, run the body, and record the result.
    fn run(
        &self,
        actor: Actor,
        id: &str,
        cmd: &dyn Command,
        args: Value,
        at_unix: i64,
    ) -> Result<Outcome, AgentError> {
        let effect = effect_of(cmd, &args);
        let key = actor_key(actor);
        let quiet = cmd.quiet();

        if let Settled::Deny(reason) = self.settle(actor, id, effect, &args)? {
            return self.refuse(actor, id, effect, &args, at_unix, reason);
        }

        let args_hash = (!quiet).then(|| args_hash(&args));
        // The grant rides on the actor so the registry's own gate stays the last
        // check instead of being bypassed here.
        let result = self.core.dispatch(actor.with_grant(effect), id, args);
        let (outcome, detail, change_set) = match &result {
            Ok(returned) => {
                let outcome = match returned {
                    Outcome::Preview(_) => AuditOutcome::Preview,
                    _ => AuditOutcome::Ok,
                };
                // The body already ran, so a failed undo write is a lost record,
                // not a failed call.
                match returned.change_set() {
                    Some(changes) => {
                        match self.store.record_change_set(&key, id, changes, at_unix) {
                            Ok(change_set) => (outcome, None, Some(change_set)),
                            Err(e) => (outcome, Some(format!("undo record not saved: {e}")), None),
                        }
                    }
                    None => (outcome, None, None),
                }
            }
            Err(e) => (AuditOutcome::Error, Some(e.to_string()), None),
        };
        if let Some(args_hash) = args_hash {
            // The trail records the call; losing the row does not undo it.
            let _ = self.store.record_audit(&NewAuditEntry {
                at_unix,
                actor: key,
                command: id.to_owned(),
                effect,
                args_hash,
                outcome,
                detail,
                change_set,
            });
        }
        result.map_err(AgentError::Command)
    }

    /// Record a refusal as `Denied` and return it, so every refused call reads
    /// the same in the trail.
    fn refuse(
        &self,
        actor: Actor,
        id: &str,
        effect: EffectKind,
        args: &Value,
        at_unix: i64,
        reason: String,
    ) -> Result<Outcome, AgentError> {
        self.store.record_audit(&NewAuditEntry {
            at_unix,
            actor: actor_key(actor),
            command: id.to_owned(),
            effect,
            args_hash: args_hash(args),
            outcome: AuditOutcome::Denied,
            detail: Some(reason.clone()),
            change_set: None,
        })?;
        Err(AgentError::Denied(reason))
    }

    /// Roll back the newest change-set on the undo stack: resolve it, replay its
    /// inversion through the registry, and mark it reversed.
    ///
    /// `Ok(None)` means the stack is empty — and an actor that may not undo
    /// still gets the refusal, so a missing grant never looks like an empty
    /// stack.
    pub fn undo(&self, actor: Actor, at_unix: i64) -> Result<Option<Undone>, AgentError> {
        self.undo_record(actor, self.store.last_undoable()?, at_unix)
    }

    /// Like [`Broker::undo`], but only the newest change-set recorded under
    /// `root` (or at `root` itself). The caller makes `root` absolute; both it
    /// and each recorded root are resolved through links before comparing.
    pub fn undo_within(
        &self,
        actor: Actor,
        root: &Path,
        at_unix: i64,
    ) -> Result<Option<Undone>, AgentError> {
        self.undo_record(actor, self.store.last_undoable_within(root)?, at_unix)
    }

    fn undo_record(
        &self,
        actor: Actor,
        record: Option<UndoRecord>,
        at_unix: i64,
    ) -> Result<Option<Undone>, AgentError> {
        let Some(record) = record else {
            self.authorize(actor, WORKSPACE_UNDO)?;
            return Ok(None);
        };
        // The change-set travels as the argument, because a command body cannot
        // reach the store; the host is the only party that supplies it.
        let cmd = self.command(WORKSPACE_UNDO)?;
        let outcome = self.run(
            actor,
            WORKSPACE_UNDO,
            cmd,
            serde_json::json!({ "change_set": record.changes }),
            at_unix,
        )?;
        // Marking after the inversion: a crash in between re-runs an idempotent
        // inversion rather than losing it.
        let first = self.store.mark_undone(record.id, at_unix)?;
        Ok(Some(Undone {
            change_set: record.id,
            first,
            outcome,
        }))
    }

    /// Write (or replace) a policy rule. `actor`, `command` and `effect` are
    /// selectors; `*` (or `None` for the effect) matches anything. Selectors
    /// that could never match are refused rather than persisted — a rule for
    /// `agent0` would sit in the store looking like a grant that never applies.
    pub fn grant(
        &self,
        actor: &str,
        command: &str,
        effect: Option<EffectKind>,
        decision: Decision,
        at_unix: i64,
    ) -> Result<i64, StoreError> {
        check_grant(&self.core, actor, command)?;
        self.store
            .set_policy_rule(actor, command, effect, decision, at_unix)
    }

    /// Drop a rule, reporting whether one matched. Deliberately unvalidated:
    /// removing a rule is always meaningful, including one written by an older
    /// build for a command this build no longer registers.
    pub fn revoke(
        &self,
        actor: &str,
        command: &str,
        effect: Option<EffectKind>,
    ) -> Result<bool, StoreError> {
        self.store.remove_policy_rule(actor, command, effect)
    }

    /// The `limit` most recent audit rows (newest first), optionally only for
    /// one actor key. Reading the trail does not itself write to it.
    pub fn audit(&self, actor: Option<&str>, limit: usize) -> Result<Vec<AuditEntry>, StoreError> {
        match actor {
            Some(actor) => self.store.audit_entries_for(actor, limit),
            None => self.store.audit_entries(limit),
        }
    }

    /// Resolve one call: `host.update` is refused to agents and automation outright; otherwise the
    /// persisted policy, then this session's switch, then the actor's default reach.
    fn verdict(&self, actor: Actor, id: &str, effect: EffectKind) -> Result<Verdict, StoreError> {
        if id == HOST_UPDATE && matches!(actor, Actor::Agent { .. } | Actor::Automation { .. }) {
            return Ok(Verdict::Deny(
                "host.update is run only by the user (mf update)".to_owned(),
            ));
        }
        let key = actor_key(actor);
        if let Some(decision) = self.store.decision_for(&key, id, effect)? {
            return Ok(match decision {
                Decision::Allow => Verdict::Allow,
                Decision::Deny => Verdict::Deny(format!("policy denies {id} for {key}")),
                Decision::Ask => Verdict::Ask,
            });
        }
        if self.session_mutations && matches!(effect, EffectKind::Mutate | EffectKind::Launch) {
            return Ok(Verdict::Allow);
        }
        if actor.may_run(effect) {
            return Ok(Verdict::Allow);
        }
        Ok(Verdict::Deny(format!(
            "{key} needs a grant for effect {effect:?} on {id}"
        )))
    }

    /// [`verdict`](Self::verdict) plus the answer to any `ask` it returns, so the
    /// caller never has to handle `Ask`. Only the paths that would run the body
    /// may call this: a listing must not block on a human.
    fn settle(
        &self,
        actor: Actor,
        id: &str,
        effect: EffectKind,
        args: &Value,
    ) -> Result<Settled, StoreError> {
        let outer = match self.verdict(actor, id, effect)? {
            Verdict::Allow => Settled::Allow,
            Verdict::Deny(reason) => Settled::Deny(reason),
            Verdict::Ask => self.ask(actor, id, effect, args),
        };
        match outer {
            Settled::Allow => self.settle_runs(actor, id, args),
            denied => Ok(denied),
        }
    }

    /// Applies the rules that name, exactly, a command `id` runs inside its body.
    /// Wildcard rules were already judged on the outer call.
    fn settle_runs(&self, actor: Actor, id: &str, args: &Value) -> Result<Settled, StoreError> {
        let Some(command) = self.core.command(id) else {
            return Ok(Settled::Allow);
        };
        let key = actor_key(actor);
        for inner in command.runs() {
            let Some(inner_command) = self.core.command(inner) else {
                return Ok(Settled::Deny(format!(
                    "{id} runs {inner}, which is not registered"
                )));
            };
            let effect = inner_command.spec().effect;
            match self.store.named_decision(&key, inner, effect)? {
                Some(Decision::Deny) => {
                    return Ok(Settled::Deny(format!(
                        "policy denies {inner} for {key}, which {id} runs"
                    )));
                }
                // The question carries the outer call's arguments; the inner commands have no own summary.
                Some(Decision::Ask) => {
                    if let Settled::Deny(reason) = self.ask(actor, inner, effect, args) {
                        return Ok(Settled::Deny(reason));
                    }
                }
                _ => {}
            }
        }
        Ok(Settled::Allow)
    }

    /// Put an `ask` to this session: a `Session` answer it gave earlier first,
    /// then the hook. Never approves without one.
    fn ask(&self, actor: Actor, id: &str, effect: EffectKind, args: &Value) -> Settled {
        if self.session_allows(actor, id, effect) {
            return Settled::Allow;
        }
        let Some(confirm) = &self.confirm else {
            return Settled::Deny(format!(
                "policy requires confirmation for {id}, and this session cannot ask"
            ));
        };
        let key = actor_key(actor);
        let summary = self
            .core
            .command(id)
            .and_then(|cmd| cmd.confirm_summary(args))
            .unwrap_or_else(|| format!("{key} wants to run {id} ({effect:?})"));
        let request = ConfirmRequest {
            actor: key.clone(),
            command: id.to_owned(),
            effect,
            args_hash: args_hash(args),
            summary,
        };
        match confirm.confirm(&request) {
            ConfirmAnswer::Once => Settled::Allow,
            ConfirmAnswer::Session => {
                self.session_allowed
                    .borrow_mut()
                    .insert((key, id.to_owned(), effect));
                Settled::Allow
            }
            ConfirmAnswer::Deny => Settled::Deny(format!("confirmation declined for {id}")),
        }
    }

    fn session_allows(&self, actor: Actor, id: &str, effect: EffectKind) -> bool {
        self.session_allowed
            .borrow()
            .contains(&(actor_key(actor), id.to_owned(), effect))
    }
}

/// Refuse a rule whose actor or command could never match, so nothing is persisted that
/// looks like a grant and never applies.
pub fn check_grant(core: &Core, actor: &str, command: &str) -> Result<(), StoreError> {
    validate_actor_key(actor)?;
    if command != ANY && core.command(command).is_none() {
        return Err(StoreError::Invalid(format!(
            "unknown command `{command}`: use a registered id or `*` to match any"
        )));
    }
    Ok(())
}

/// Current Unix time in seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The effect a call runs as: a preview is authorized as a read.
fn effect_of(cmd: &dyn Command, args: &Value) -> EffectKind {
    if is_preview(args) {
        EffectKind::Read
    } else {
        cmd.spec().effect
    }
}

/// SHA-256 of the canonical JSON of `args`, so the trail identifies a call's
/// arguments without persisting them.
fn args_hash(args: &Value) -> String {
    let bytes = serde_json::to_vec(args).expect("a json Value always serializes");
    format!("{:x}", Sha256::digest(bytes))
}
