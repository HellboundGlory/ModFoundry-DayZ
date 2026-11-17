//! Workshop item details for the Publish dialog, `mf` and agents.

use std::path::{Path, PathBuf};
use std::time::Duration;

use modfoundry_build::publish::checks::{build_files, signature_checks, structure_checks};
use modfoundry_build::publish::details::{
    details_checks, details_checks_for, details_revision, item_tags, meta_cpp_id, read_details,
    write_details, Details, ItemDetails, PREVIEW_FILE,
};
use modfoundry_build::publish::facts::mod_facts;
use modfoundry_build::publish::meta::{dotnet_utc_binary, render_meta_cpp};
use modfoundry_build::publish::preview::preview_jpeg;
use modfoundry_build::publish::tags::{key_value_tags, ModFacts};
use modfoundry_build::publish::PREVIEW_EXTENSIONS;
use modfoundry_build::remove_unbuilt_addons;
use modfoundry_core::{CommandError, Diagnostic, Severity};
use modfoundry_platform::steam::{KeyValueTag, Visibility};
use modfoundry_vfs::{root_name, write::WriteTxn};
use serde::{Deserialize, Serialize};

use crate::{build_project, BuildArgs};

/// The arguments of a command that reads one mod's details.
#[derive(Debug, Clone, Deserialize)]
pub struct ModRootArgs {
    pub mod_root: PathBuf,
}

/// The arguments of `publish.save_details`: fields, description and an optional picture.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveDetailsArgs {
    pub mod_root: PathBuf,
    pub details: ItemDetails,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview_from: Option<PathBuf>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub clear_id: bool,
}

impl SaveDetailsArgs {
    /// Refuses the arguments a save cannot mean anything for, before anything is staged.
    pub fn check(&self) -> Result<(), CommandError> {
        if self.clear_id && self.details.id.is_some() {
            return Err(CommandError::InvalidArguments(
                "`clear_id` removes the saved id; leave `details.id` out".into(),
            ));
        }
        Ok(())
    }
}

/// A mod's Workshop details with the problems found in them.
#[derive(Debug, Clone, Serialize)]
pub struct DetailsView {
    #[serde(flatten)]
    pub details: Details,
    pub revision: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// A mod's Workshop details and what is wrong with them.
pub fn details_view(mod_root: &Path) -> anyhow::Result<DetailsView> {
    // Hashed first, so an edit between the two reads makes a save refuse rather than overwrite it.
    let revision = details_revision(mod_root)?;
    let details = read_details(mod_root)?;
    let diagnostics = details_checks(&details, details.item.id.is_none());
    Ok(DetailsView {
        details,
        revision,
        diagnostics,
    })
}

/// Stage the details and a re-encoded preview into `txn`; returns the files written.
pub fn save_details(args: &SaveDetailsArgs, txn: &mut WriteTxn) -> anyhow::Result<usize> {
    if let Some(expected) = &args.expected_sha256 {
        let current = details_revision(&args.mod_root)?;
        if *expected != current {
            anyhow::bail!(
                "stale Workshop details: those of {} changed since they were read (now revision {current}); read publish.details again",
                args.mod_root.display()
            );
        }
    }
    if args.clear_id {
        if let Some(id) = meta_cpp_id(&args.mod_root)? {
            anyhow::bail!(
                "the root meta.cpp still names item {id}, so the id would come back; remove its publishedid first"
            );
        }
    }
    let mut item = args.details.clone();
    // The id is the item's link to Steam; a save that leaves it out must not create a new item.
    item.id = match (args.clear_id, item.id) {
        (true, _) => None,
        (false, Some(id)) => Some(id),
        (false, None) => read_details(&args.mod_root)?.item.id,
    };
    write_details(txn, &item, &args.description)?;
    let Some(picture) = &args.preview_from else {
        return Ok(2);
    };
    let picture = args.mod_root.join(picture);
    let bytes =
        std::fs::read(&picture).map_err(|e| anyhow::anyhow!("{}: {e}", picture.display()))?;
    txn.write(PREVIEW_FILE, preview_jpeg(&bytes)?)?;
    for path in other_preview_files() {
        txn.remove(path)?;
    }
    Ok(3)
}

/// The other preview pictures beside a saved one, which would come back as the preview later.
fn other_preview_files() -> Vec<String> {
    let (base, kept) = PREVIEW_FILE.rsplit_once('.').unwrap_or((PREVIEW_FILE, ""));
    PREVIEW_EXTENSIONS
        .iter()
        .filter(|ext| **ext != kept)
        .map(|ext| format!("{base}.{ext}"))
        .collect()
}

/// The arguments of `publish.prepare`: the build to check and whether only the content is sent.
#[derive(Debug, Clone, Deserialize)]
pub struct PrepareArgs {
    #[serde(flatten)]
    pub build: BuildArgs,
    #[serde(default)]
    pub content_only: bool,
}

/// What an upload would send, and whether it may.
#[derive(Debug, Clone, Serialize)]
pub struct UploadPlan {
    pub mod_root: PathBuf,
    pub content_dir: PathBuf,
    pub preview: Option<PathBuf>,
    pub id: Option<u64>,
    pub title: String,
    pub description: String,
    pub visibility: Option<Visibility>,
    pub tags: Vec<String>,
    pub kv_tags: Vec<KeyValueTag>,
    pub content_only: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub ready: bool,
}

/// Builds the mod, checks it like Publisher and stages its `meta.cpp` and preview.
pub fn prepare(args: &PrepareArgs, now: Duration) -> anyhow::Result<UploadPlan> {
    let details = read_details(&args.build.mod_root)?;
    let report = build_project(&args.build)?;
    let removed = remove_unbuilt_addons(&report)?;
    let content_dir = std::path::absolute(&report.output_dir)?;
    let facts = mod_facts(&content_dir)?;
    let mut diagnostics = check_content(&content_dir, &details)?;
    diagnostics.extend(removed.iter().map(|path| {
        Diagnostic::new(
            "MF-P020",
            Severity::Warning,
            format!("removed {path} from the build output: this build did not write it"),
        )
    }));
    diagnostics.extend(
        report
            .warnings
            .iter()
            .map(|warning| Diagnostic::new("MF-P019", Severity::Warning, warning.clone())),
    );
    let preview = stage_preview(&details, &preview_folder(&args.build)?, &mut diagnostics)?;
    let item = &details.item;
    stage_meta(&content_dir, item.id.unwrap_or(0), &item.title, now)?;
    if args.content_only && item.id.is_none() {
        diagnostics.push(Diagnostic::new(
            "MF-P018",
            Severity::Error,
            "content-only needs an existing item; this mod has no Workshop id, so there is nothing to update. Publish it in full or set the id in workshop/item.json",
        ));
    }
    let tags = item_tags(&item.tags, has_content_review(&diagnostics));
    let ready = !diagnostics.iter().any(|d| d.severity == Severity::Error);
    Ok(UploadPlan {
        mod_root: std::path::absolute(&args.build.mod_root)?,
        content_dir,
        preview,
        id: item.id,
        title: item.title.clone(),
        description: details.description,
        visibility: item.visibility,
        tags,
        kv_tags: kv_tags(&facts, item),
        // Without saved details an existing item's fields would be overwritten with defaults.
        content_only: item.id.is_some() && (args.content_only || !details.has_item_file),
        diagnostics,
        ready,
    })
}

fn check_content(content_dir: &Path, details: &Details) -> anyhow::Result<Vec<Diagnostic>> {
    let mut diagnostics = structure_checks(&build_files(content_dir)?);
    diagnostics.extend(signature_checks(content_dir)?);
    let content_review = has_content_review(&diagnostics);
    diagnostics.extend(details_checks_for(
        details,
        details.item.id.is_none(),
        content_review,
    ));
    Ok(diagnostics)
}

fn has_content_review(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.code == "MF-P005")
}

/// A picture that cannot be re-encoded is a diagnostic, not a failed prepare.
fn stage_preview(
    details: &Details,
    folder: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(picture) = &details.preview else {
        return Ok(None);
    };
    let jpeg = std::fs::read(picture)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| preview_jpeg(&bytes));
    let jpeg = match jpeg {
        Ok(jpeg) => jpeg,
        Err(e) => {
            diagnostics.push(Diagnostic::new(
                "MF-P015",
                Severity::Error,
                format!("{e:#}"),
            ));
            return Ok(None);
        }
    };
    Ok(Some(stage_file(folder, "preview.jpg", jpeg)?))
}

/// `builds/<profile>/` of the mod, beside the default build output and never packed.
fn preview_folder(build: &BuildArgs) -> anyhow::Result<PathBuf> {
    let profile = build.profile.as_deref().unwrap_or("default");
    Ok(std::path::absolute(
        build.mod_root.join("builds").join(profile),
    )?)
}

fn kv_tags(facts: &ModFacts, item: &ItemDetails) -> Vec<KeyValueTag> {
    let info = [
        ("bis_version", item.version.as_str()),
        ("bis_website", item.website.as_str()),
        ("facebook", item.facebook.as_str()),
        ("youtube", item.youtube.as_str()),
        ("twitter", item.twitter.as_str()),
        ("reddit", item.reddit.as_str()),
    ];
    key_value_tags(facts, &info)
}

/// Writes the build's `meta.cpp`, which names its Workshop item.
pub(crate) fn stage_meta(
    content_dir: &Path,
    id: u64,
    title: &str,
    now: Duration,
) -> anyhow::Result<()> {
    let meta = render_meta_cpp(id, title, dotnet_utc_binary(now));
    stage_file(content_dir, "meta.cpp", meta).map(|_| ())
}

/// Output is regenerable, so nothing it replaces is kept for undo.
fn stage_file(folder: &Path, name: &str, content: impl Into<Vec<u8>>) -> anyhow::Result<PathBuf> {
    let mut txn = WriteTxn::unrecorded(root_name("stage", folder), folder);
    txn.write(name, content)?;
    txn.finish()?;
    Ok(folder.join(name))
}
