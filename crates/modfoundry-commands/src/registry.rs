//! The command registry: every ModFoundry operation, registered once.
//!
//! Frontends (CLI, MCP, GUI, automation) never call a body directly: they
//! enumerate [`core`] for menus/tool lists and dispatch through it, so the
//! effect gate (agent/automation permissions) and the shared [`Outcome`] shape
//! apply to every path. The bodies themselves stay in this crate's area modules
//! (`build_ops`, `run_ops`, `vfs_ops`, the root read/validate/lint/scaffold
//! helpers); the command impls here are thin, typed adapters from JSON args to
//! those bodies.

use crate::build_ops::{BuildTarget, SetBuild, SetRequest};
use crate::ce_classes::{self, ConfigLookup};
use crate::ce_ops::{ce_edit_op_schema, ce_tree, ce_validate};
use crate::config_host::{ConfigHost, ConfigIndexRequest};
use crate::diag_run::{diag_log_dirs, plan_diag, NO_TIMER_SECS};
use crate::economy_host::EconomyHost;
use crate::economy_load::EconomyIndexRequest;
use crate::font_ops::{self, FontArgs};
use crate::image_ops::{self, ImageArgs};
use crate::imageset_ops::{self, ImagesetArgs};
use crate::known_errors::{plan_learn, KnownErrors};
use crate::layout_render::{
    check_out, parse_background, parse_size, picture_change, render_layout, RenderArgs,
    RenderRequest,
};
use crate::paths::{folded, resolved_through_links};
use crate::pbo_ops::{self, PboUnpackArgs};
use crate::project_deps::{self, EditAction, EditOutcome, EditTarget};
use crate::publish_ops::{
    details_view, prepare, save_details, ModRootArgs, PrepareArgs, SaveDetailsArgs,
};
use crate::publish_upload::{upload, PublishStatus, UploadArgs};
use crate::run_ops::{
    client_log_dirs, errors_from_logs, parse_patterns, plan_client, plan_server, resolve_game_dir,
    resolve_server_dir, scratch_profiles_dir, server_log_dirs, wants_verdict, RunControl,
};
use crate::run_record::{self, run_launch, Ending, Launch};
use crate::script_check::{
    check_folders, parse_target, target_name, unpack_sources, ScriptCheckArgs, VanillaSource,
    TARGETS,
};
use crate::script_host::ScriptHost;
use crate::script_index::IndexRequest;
use crate::script_lint::ScriptLint;
use crate::script_outline::outline_tree;
use crate::settings_fill::{self, SettingsUsed};
use crate::unpack_cache;
use crate::update::{self, GitHubReleases};
use crate::workshop_api::{fetch_details, HttpFetch, WorkshopDetailsArgs};
use crate::{
    binarize_to, build_contents, build_project, config_file, derapify_config, derapify_to,
    edit_text_to, errors_from_dir, keygen, lint_target_with, plan_binarize, plan_build_project,
    plan_derapify, plan_edit_text, plan_keygen, plan_scaffold, read_text, run_client, run_diag,
    run_server, scaffold, validate_file, BuildArgs, BuildReport, RunClientArgs, RunDiagArgs,
    RunServerArgs, NO_GAME_INSTALL,
};
use enforce_check::Target;
use modfoundry_build::publish::checks::{
    build_files, server_key_checks, signature_checks, structure_checks,
};
use modfoundry_build::publish::details::read_details;
use modfoundry_ce::classes;
use modfoundry_ce::kind::CeKind;
use modfoundry_config::edit::{plan_all, EditOp};
use modfoundry_config::ScaffoldOptions;
use modfoundry_core::{
    ChangeSummary, Command, CommandError, CommandSpec, Core, EffectKind, Outcome, HOST_UPDATE,
    PREVIEW_ARG,
};
use modfoundry_diag::{mark_baseline, LogStates, StopSignal};
use modfoundry_enforce::{
    classify, first_arms, lint_text, parse_tree, read_source, FileClass, Finding,
};
use modfoundry_platform::stage::dead_run_leftovers;
use modfoundry_platform::steam::{SteamEvent, SteamHelper, SteamRequest, SteamRun, Visibility};
use modfoundry_search::chunk::Kind;
use modfoundry_store::run::{RunRecord, RunState};
use modfoundry_store::Store;
use modfoundry_vfs::write::WriteTxn;
use modfoundry_vfs::{root_name, ChangeSet};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The in-memory state one frontend session keeps across every core it builds:
/// the script index, the config index, the economy index, the live runs and the running
/// Workshop upload. A frontend that builds a core per operation (the GUI's job runner) holds
/// one [`Session`] and hands out clones.
#[derive(Clone)]
pub struct Session {
    script: Arc<ScriptHost>,
    config: Arc<ConfigHost>,
    economy: Arc<EconomyHost>,
    runs: RunControl,
    publish: Arc<PublishStatus>,
    db: Option<PathBuf>,
    fill_settings: bool,
    actor: String,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            script: Arc::default(),
            config: Arc::default(),
            economy: Arc::default(),
            runs: RunControl::default(),
            publish: Arc::default(),
            db: None,
            fill_settings: true,
            actor: "user".to_owned(),
        }
    }
}

impl Session {
    /// A session with an empty script index, an empty config index and no live
    /// runs.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read project settings from, and later record runs in, the agent database at `db`.
    pub fn with_db(mut self, db: PathBuf) -> Self {
        self.db = Some(db);
        self
    }

    /// Record runs in the database without filling left-out arguments from saved settings: the
    /// app builds its arguments from its own settings already.
    pub fn without_settings_fill(mut self) -> Self {
        self.fill_settings = false;
        self
    }

    fn settings_db(&self) -> Option<PathBuf> {
        self.db.clone().filter(|_| self.fill_settings)
    }

    /// The actor key launches are recorded under (`user`, `cli`, `agent:<id>`).
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = actor.into();
        self
    }

    /// Whether launches can be recorded, and so watched and stopped from another process.
    pub fn has_db(&self) -> bool {
        self.db.is_some()
    }

    /// Stop this session's live runs and wait up to `timeout` for them to clean up.
    pub fn shutdown(&self, timeout: Duration) {
        self.runs.stop_all_and_wait(timeout);
    }

    /// The running Workshop upload's state, shared with every core of this session.
    pub fn publish_status(&self) -> Arc<PublishStatus> {
        Arc::clone(&self.publish)
    }
}

/// The full registry: one [`Core`] with every operation this build exposes.
pub fn core() -> Core {
    core_with(&Session::new())
}

/// The full registry over `session`'s state: a core per operation shares the
/// session's script index and live runs with every other core built from it.
pub fn core_with(session: &Session) -> Core {
    let mut core = Core::new();
    register_all_with(&mut core, session);
    core
}

/// Register every operation into `core` over a session of its own (frontends
/// that need to add host-specific commands register on top of this).
pub fn register_all(core: &mut Core) {
    register_all_with(core, &Session::new());
}

/// Register every operation into `core` over `session`'s state.
pub fn register_all_with(core: &mut Core, session: &Session) {
    let control = session.runs.clone();
    let script = Arc::clone(&session.script);
    let config = Arc::clone(&session.config);
    let economy = Arc::clone(&session.economy);
    core.register(Box::new(ReadText));
    core.register(Box::new(VfsList));
    core.register(Box::new(CeGetTree));
    core.register(Box::new(CeIndex {
        host: Arc::clone(&economy),
    }));
    core.register(Box::new(CeReference {
        host: Arc::clone(&economy),
    }));
    core.register(Box::new(CeSource {
        host: Arc::clone(&economy),
    }));
    core.register(Box::new(CeValidate {
        host: Arc::clone(&economy),
        config: Arc::clone(&config),
    }));
    core.register(Box::new(CeCoverage {
        economy,
        config: Arc::clone(&config),
    }));
    core.register(Box::new(CeEdit));
    core.register(Box::new(ScriptDiagnostics {
        host: Arc::clone(&script),
    }));
    core.register(Box::new(ConfigGetTree));
    core.register(Box::new(ConfigValidate {
        host: Arc::clone(&config),
    }));
    core.register(Box::new(ConfigEdit));
    core.register(Box::new(ConfigBinarize));
    core.register(Box::new(ConfigDerapify));
    core.register(Box::new(ImageConvert));
    core.register(Box::new(ImagesetBuild));
    core.register(Box::new(FontBuild));
    core.register(Box::new(LayoutCheck {
        settings: session.settings_db(),
    }));
    core.register(Box::new(LayoutRender {
        settings: session.settings_db(),
    }));
    core.register(Box::new(GameDiff));
    core.register(Box::new(ConflictCheck));
    core.register(Box::new(GameSnapshot));
    core.register(Box::new(ScriptOutline));
    core.register(Box::new(ScriptCheck {
        settings: session.settings_db(),
    }));
    core.register(Box::new(SearchIndex));
    core.register(Box::new(SearchQuery));
    core.register(Box::new(PboUnpack));
    core.register(Box::new(SignCheck));
    core.register(Box::new(ProjectScaffold));
    core.register(Box::new(ProjectDependencies {
        db: session.settings_db(),
    }));
    core.register(Box::new(ProjectEditDependencies {
        db: session.settings_db(),
    }));
    core.register(Box::new(BuildRun {
        db: session.settings_db(),
    }));
    core.register(Box::new(SignKeygen));
    core.register(Box::new(WorkspaceUndo));
    core.register(Box::new(TextEdit));
    core.register(Box::new(DiagRun {
        control: control.clone(),
        db: session.db.clone(),
        settings: session.settings_db(),
        actor: session.actor.clone(),
    }));
    core.register(Box::new(DiagRunClient {
        control: control.clone(),
        db: session.db.clone(),
        settings: session.settings_db(),
        actor: session.actor.clone(),
    }));
    core.register(Box::new(DiagRunDiag {
        control: control.clone(),
        db: session.db.clone(),
        settings: session.settings_db(),
        actor: session.actor.clone(),
    }));
    core.register(Box::new(DiagStop {
        control,
        db: session.db.clone(),
    }));
    core.register(Box::new(DiagStatus {
        db: session.db.clone(),
    }));
    core.register(Box::new(DiagErrors));
    core.register(Box::new(DiagLearnError));
    core.register(Box::new(HostChangelog));
    core.register(Box::new(HostDoctor));
    core.register(Box::new(HostUpdateCheck));
    core.register(Box::new(HostUpdate));
    core.register(Box::new(FormatsList));
    core.register(Box::new(WorkshopDetails {
        fetch: Box::new(HttpFetch),
    }));
    core.register(Box::new(PublishDetails));
    core.register(Box::new(PublishSaveDetails));
    core.register(Box::new(PublishPrepare {
        db: session.settings_db(),
    }));
    let steam: Arc<dyn SteamRun> = Arc::new(SteamHelper::beside_current_exe());
    core.register(Box::new(PublishUpload {
        steam: Arc::clone(&steam),
        status: session.publish_status(),
        db: session.settings_db(),
    }));
    core.register(Box::new(PublishItems {
        steam: Arc::clone(&steam),
    }));
    core.register(Box::new(PublishStatusCommand {
        status: session.publish_status(),
    }));
    core.register(Box::new(PublishCancel {
        status: session.publish_status(),
    }));

    core.register(Box::new(ScriptIndex {
        host: Arc::clone(&script),
        settings: session.settings_db(),
    }));
    core.register(Box::new(ScriptLint::new(
        Box::new(ScriptIndex {
            host: Arc::clone(&script),
            settings: session.settings_db(),
        }),
        Box::new(ScriptCheck {
            settings: session.settings_db(),
        }),
        Box::new(ScriptDiagnostics {
            host: Arc::clone(&script),
        }),
    )));
    core.register(Box::new(ScriptIndexStatus {
        host: Arc::clone(&script),
    }));
    core.register(Box::new(ScriptQuery {
        host: Arc::clone(&script),
        id: "script.hover",
        summary: "Resolve the symbol at a byte offset in a buffer for a hover card.",
        run: ScriptHost::hover,
    }));
    core.register(Box::new(ScriptQuery {
        host: Arc::clone(&script),
        id: "script.definition",
        summary: "Resolve the symbol lifted at a byte offset to its definition targets.",
        run: ScriptHost::definition,
    }));
    core.register(Box::new(ScriptQuery {
        host: Arc::clone(&script),
        id: "script.complete",
        summary: "Completion candidates at a byte offset in a buffer.",
        run: ScriptHost::complete,
    }));
    core.register(Box::new(ScriptFindClass {
        host: Arc::clone(&script),
    }));
    core.register(Box::new(ScriptSourceCommand { host: script }));

    core.register(Box::new(ConfigResolve {
        host: Arc::clone(&config),
    }));
    core.register(Box::new(ConfigSource {
        host: Arc::clone(&config),
    }));
    core.register(Box::new(ConfigIndex { host: config }));
    core.register(Box::new(ConfigPatches));
    core.register(Box::new(BuildContents {
        db: session.settings_db(),
    }));
}

/// `path` argument shared by the file-addressed read/analyze commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathArgs {
    pub path: PathBuf,
}

/// A config addressed either on disk (`path`) or by an unsaved buffer's text.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TextOrPath {
    pub path: PathBuf,
    #[serde(default)]
    pub text: Option<String>,
}

impl TextOrPath {
    fn read(self) -> Result<String, CommandError> {
        match self.text {
            Some(text) => Ok(text),
            None => read_utf8(&config_file(&self.path)),
        }
    }
}

/// Arguments for `config.get_tree`: the config source plus the class path whose
/// [`modfoundry_config::outline::ClassView`] the caller wants.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GetTreeArgs {
    #[serde(flatten)]
    pub source: TextOrPath,
    #[serde(default)]
    pub class: Option<Vec<String>>,
}

/// The directory a config's sibling files resolve against: the path itself when
/// it is a directory, else its parent.
fn config_root(path: &Path) -> &Path {
    if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

/// `path` + optional `out` argument for the config converters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertArgs {
    pub path: PathBuf,
    #[serde(default)]
    pub out: Option<PathBuf>,
}

/// Arguments for the `text.edit` command: the file the session edited and its
/// replacement text. `out` redirects the write to another file (a save-as),
/// leaving the edited path alone. `expected_sha256` is the revision of `path`
/// the caller read — with it, the edit refuses to land on content that has
/// moved since. It names `path`, so it cannot be combined with `out`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEditArgs {
    pub path: PathBuf,
    pub text: String,
    #[serde(default)]
    pub out: Option<PathBuf>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

impl TextEditArgs {
    /// The file the edit writes — one resolution for the preview and for the
    /// apply, so what a caller is shown is what runs. `expected_sha256`
    /// describes `path`, so a save-as cannot carry one.
    fn target(&self) -> Result<&std::path::Path, CommandError> {
        if self.out.is_some() && self.expected_sha256.is_some() {
            return Err(CommandError::InvalidArguments(
                "`expected_sha256` checks `path`; it cannot be combined with `out`".into(),
            ));
        }
        Ok(self.out.as_deref().unwrap_or(&self.path))
    }
}

/// Arguments for the `config.edit` command: the `config.cpp` the session is
/// editing, the structured ops to apply to it in order, and the revision the
/// caller read. With `expected_sha256`, the edit refuses to land on content
/// that has moved since.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEditArgs {
    pub path: PathBuf,
    pub ops: Vec<EditOp>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// Arguments for the `ce.edit` command: the Central Economy XML file the
/// session is editing, the record ops to apply in order, and the revision the
/// caller read. With `expected_sha256`, the edit refuses to land on content
/// that has moved since.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CeEditArgs {
    pub path: PathBuf,
    pub ops: Vec<modfoundry_ce::ops::CeOp>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// Arguments for the `project.scaffold` command. `dir` is required: a command
/// must not silently write into a caller's current directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldArgs {
    pub name: String,
    pub dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// The CfgPatches class name (default: `<name>_Scripts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_class: Option<String>,
    /// CfgPatches names listed in requiredAddons after DZ_Data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_addons: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<bool>,
}

impl ScaffoldArgs {
    pub fn options(&self) -> ScaffoldOptions {
        let mut options = ScaffoldOptions::new(self.name.clone());
        options.display_name = self.display_name.clone();
        options.description = self.description.clone();
        options.author = self.author.clone();
        if let Some(version) = &self.version {
            options.version = version.clone();
        }
        options.prefix = self.prefix.clone();
        options.patch_class = self.patch_class.clone();
        if let Some(addons) = &self.required_addons {
            options.required_addons = addons.clone();
        }
        if let Some(modules) = &self.modules {
            options.modules = modules.clone();
        }
        if let Some(starter) = self.starter {
            options.starter = starter;
        }
        options
    }
}

/// Arguments for the `sign.keygen` command. `dir` is required for the same
/// reason as [`ScaffoldArgs::dir`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeygenArgs {
    pub authority: String,
    pub dir: PathBuf,
    #[serde(default)]
    pub tools: Option<PathBuf>,
}

/// A lint finding as frontends receive it: the shared diagnostic plus the file
/// it came from (findings are per-file, unlike whole-config diagnostics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingReport {
    #[serde(flatten)]
    pub diagnostic: modfoundry_core::Diagnostic,
    pub path: PathBuf,
}

/// Arguments for the `workspace.undo` command: the recorded change-set to
/// invert. The host resolves "the last one" from the undo log and supplies it,
/// so an undo cannot name a change-set the host never made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoArgs {
    pub change_set: ChangeSet,
}

/// Arguments for the `diag.errors` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorArgs {
    pub dir: PathBuf,
    #[serde(default)]
    pub source_root: Option<PathBuf>,
    /// A log file or a folder from a run without this project's mods: errors that also occur there are marked baseline.
    #[serde(default)]
    pub baseline: Option<PathBuf>,
    /// The project's known-error file (default `server/known-errors.json` under `source_root`).
    #[serde(default)]
    pub known_errors: Option<PathBuf>,
}

/// Arguments for the `diag.learn_error` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnErrorArgs {
    /// The profiles or log dir the learned error is read from.
    pub dir: PathBuf,
    /// The project root; also names the default known-error file.
    #[serde(default)]
    pub source_root: Option<PathBuf>,
    /// The known-error file to append to (default `server/known-errors.json` under `source_root`).
    #[serde(default)]
    pub known_errors: Option<PathBuf>,
    /// The id of the error to learn, as `diag.errors` reported it.
    pub id: String,
    #[serde(default)]
    pub meaning: String,
    #[serde(default)]
    pub hint: String,
}

/// Arguments for the `diag.run_client` command. The `-connect=host:port` target
/// is validated here, so the frontend stays free of launch-argument rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunClientCommandArgs {
    #[serde(flatten)]
    pub client: RunClientArgs,
    /// `-connect=host:gameport[:queryport]`, parsed into `client.connect` before launching.
    #[serde(default)]
    pub connect: Option<String>,
    /// Optional id under which the caller can stop this run via `diag.stop`.
    #[serde(default)]
    pub run_id: Option<String>,
}

/// Arguments for the `diag.stop` and `diag.status` commands: the run id a launch was
/// recorded under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopArgs {
    pub run_id: String,
}

/// The host report `mf doctor` / the `host.doctor` tool renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostReport {
    pub platform: String,
    pub steam_libraries: Vec<SteamLibraryReport>,
    pub dayz_game: Option<PathBuf>,
    pub dayz_tools: Option<PathBuf>,
    #[serde(default)]
    pub gpu: GpuReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
}

/// The GPU search helper, the NVIDIA driver and the CUDA libraries, as the host report sees them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuReport {
    pub helper: Option<PathBuf>,
    pub driver: bool,
    pub libraries: Option<PathBuf>,
    pub missing: Vec<String>,
    #[serde(default)]
    pub device_ok: Option<bool>,
    #[serde(default)]
    pub device_error: Option<String>,
}

/// One Steam library as seen by the host report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamLibraryReport {
    pub root: PathBuf,
    pub dayz_workshop_root: PathBuf,
}

fn spec(id: &'static str, effect: EffectKind, summary: &'static str) -> CommandSpec<'static> {
    CommandSpec {
        id,
        effect,
        summary,
    }
}

fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, CommandError> {
    serde_json::from_value(args).map_err(|e| CommandError::InvalidArguments(e.to_string()))
}

/// Whether a launch call waits for its run to end. `wait: false` needs the agent database,
/// the only way `diag.stop` can reach a run that outlives the call.
fn wait_for_run(args: &Value, db: Option<&Path>) -> Result<bool, CommandError> {
    let wait = args.get("wait").map_or(Ok(true), |wait| {
        wait.as_bool()
            .ok_or_else(|| CommandError::InvalidArguments("wait must be a boolean".to_owned()))
    })?;
    if !wait && db.is_none() {
        return Err(CommandError::InvalidArguments(
            "wait: false needs the agent database".to_owned(),
        ));
    }
    Ok(wait)
}

/// The limit a detached launch reports: the one its caller gave or the saved settings filled.
fn run_limit(wait: bool, args: &Value) -> Option<u64> {
    if wait {
        return None;
    }
    args.get("timeout_secs").and_then(Value::as_u64)
}

/// The server arguments a `diag.run` call launches with, and the limit its answer names.
fn diag_run_args(wait: bool, args: Value) -> Result<(RunServerArgs, Option<u64>), CommandError> {
    let limit_secs = run_limit(wait, &args);
    let limit_given = args.get("timeout_secs").is_some();
    let mut parsed: RunServerArgs = parse_args(args)?;
    if !wait && !limit_given {
        parsed.timeout_secs = NO_TIMER_SECS;
    }
    Ok((parsed, limit_secs))
}

/// Refuses an unusable verdict pattern before anything is staged.
fn check_verdict_patterns(
    expect: &[String],
    fail: &[String],
    allow: &[String],
) -> Result<(), CommandError> {
    for lines in [expect, fail, allow] {
        parse_patterns(lines).map_err(CommandError::InvalidArguments)?;
    }
    Ok(())
}

/// A broken known-errors file refuses a run before it is planned, staged or detached.
fn check_known_errors(source_root: Option<&Path>) -> Result<(), CommandError> {
    crate::known_errors::KnownErrors::load(None, source_root)
        .map(|_| ())
        .map_err(|e| CommandError::InvalidArguments(e.to_string()))
}

/// A verdict needs the run to wait: a detached run returns before its logs are complete.
fn check_detached_verdict(
    wait: bool,
    expect: &[String],
    fail: &[String],
    allow: &[String],
) -> Result<(), CommandError> {
    if !wait && wants_verdict(expect, fail, allow) {
        return Err(CommandError::InvalidArguments(
            "a verdict needs the run to wait; pass wait: true".to_owned(),
        ));
    }
    Ok(())
}

/// Starts a server launch that outlives the call, and reports it: `running` once its server is
/// ready, or `ended` with the outcome when it has already finished.
fn detached_data<T: Serialize + Ending + Send + 'static>(
    control: &RunControl,
    launch: Launch,
    body: impl FnOnce(&StopSignal) -> anyhow::Result<T> + Send + 'static,
    settings_used: SettingsUsed,
    limit_secs: Option<u64>,
) -> Result<Outcome, CommandError> {
    let log_dirs = launch.log_dirs.clone();
    let detached = run_record::spawn_server(control, launch, body);
    let run_id = detached.run_id.clone();
    let ready = detached.ready;
    let (state, mut value) = match detached.outcome() {
        Some(result) => ("ended", json_of(&result.map_err(failed)?)?),
        None => ("running", json!({})),
    };
    value["state"] = json!(state);
    value["ready"] = json!(ready);
    value["run_id"] = json!(run_id);
    if let Some(limit_secs) = limit_secs {
        value["stops_after_secs"] = json!(limit_secs);
    }
    value["profiles_dir"] = json!(log_dirs.first());
    value["log_dirs"] = json!(log_dirs);
    settings_used.write_into(&mut value);
    Ok(Outcome::Data(value))
}

fn failed(e: anyhow::Error) -> CommandError {
    CommandError::Failed(format!("{e:#}"))
}

fn json_of<T: Serialize>(value: &T) -> Result<Value, CommandError> {
    serde_json::to_value(value).map_err(|e| CommandError::Failed(e.to_string()))
}

/// The result as data, with the saved settings the call filled in written beside it.
fn data_using_settings<T: Serialize>(
    result: &T,
    settings_used: &SettingsUsed,
) -> Result<Outcome, CommandError> {
    let mut value = json_of(result)?;
    settings_used.write_into(&mut value);
    Ok(Outcome::Data(value))
}

fn launch_data<T: Serialize>(
    result: &T,
    run_id: Option<&str>,
    settings_used: &SettingsUsed,
) -> Result<Outcome, CommandError> {
    let mut value = json_of(result)?;
    settings_used.write_into(&mut value);
    if let Some(run_id) = run_id {
        value["run_id"] = json!(run_id);
    }
    Ok(Outcome::Data(value))
}

/// Schema for a run's expect/fail/allow list.
fn verdict_list_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": description,
    })
}

/// Schema for the `test_mods` argument the diag launches take.
fn test_mods_schema() -> Value {
    json!({
        "type": "array",
        "description": "extra mods loaded only for this run, in order",
        "items": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the mod dir staged as the @<link_name> link" },
                "link_name": { "type": "string", "description": "bare link name (no @, no separators)" },
                "role": { "type": "string", "enum": ["both", "server_only"], "description": "both = -mod, server_only = -serverMod" },
            },
            "required": ["path", "link_name", "role"],
        },
    })
}

/// Schema for the single-`path` argument commands.
fn path_schema(description: &str) -> Value {
    json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": description } },
        "required": ["path"],
    })
}

fn expected_sha256_property() -> Value {
    json!({ "type": "string", "description": "hex SHA-256 of the file's current content (the revision the caller read); the edit refuses to land if the file has moved since" })
}

/// Give a mutation's schema the reserved `preview` argument, so an MCP client
/// (or a GUI form) can ask for the change-set instead of the change.
fn preview_schema(mut schema: Value) -> Value {
    schema["properties"][PREVIEW_ARG] = json!({
        "type": "boolean",
        "description": "return the change-set instead of applying it",
    });
    schema
}

fn planned(change_set: &ChangeSet) -> Result<Option<Value>, CommandError> {
    Ok(Some(json_of(change_set)?))
}

/// A plan as data, naming the saved settings the call it plans would use.
fn planned_using<T: Serialize>(
    plan: &T,
    settings_used: &SettingsUsed,
) -> Result<Value, CommandError> {
    let mut value = json_of(plan)?;
    settings_used.write_into(&mut value);
    Ok(value)
}

fn applied(
    files_changed: usize,
    description: String,
    change_set: &ChangeSet,
) -> Result<Outcome, CommandError> {
    Ok(Outcome::applied(
        ChangeSummary {
            kind: EffectKind::Mutate,
            files_changed,
            description,
        },
        json_of(change_set)?,
    ))
}

fn read_utf8(path: &Path) -> Result<String, CommandError> {
    std::fs::read_to_string(path)
        .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))
}

fn plan_ops_edit(
    path: &Path,
    text: &str,
    expected_sha256: Option<&str>,
) -> Result<Option<Value>, CommandError> {
    let change_set = plan_edit_text(path, text, expected_sha256).map_err(failed)?;
    planned(&change_set)
}

fn apply_ops_edit(
    path: &Path,
    text: &str,
    expected_sha256: Option<&str>,
    ops: usize,
    noun: &str,
) -> Result<Outcome, CommandError> {
    let (target, bytes, change_set) = edit_text_to(path, text, expected_sha256).map_err(failed)?;
    applied(
        1,
        format!(
            "applied {ops} {noun} edit(s) to {} ({bytes} bytes)",
            target.display()
        ),
        &change_set,
    )
}

/// A lint finding serialized as the shared diagnostic plus its file.
fn finding_json(finding: modfoundry_enforce::Finding) -> Result<Value, CommandError> {
    json_of(&FindingReport {
        diagnostic: finding.diagnostic,
        path: finding.path,
    })
}

struct ReadText;

impl Command for ReadText {
    fn spec(&self) -> CommandSpec<'static> {
        spec("vfs.read", EffectKind::Read, "Read a file's UTF-8 text.")
    }

    fn args_schema(&self) -> Value {
        path_schema("path to a file to read")
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let PathArgs { path } = parse_args(args)?;
        Ok(Outcome::Data(json!(read_text(&path).map_err(failed)?)))
    }
}

struct VfsList;

impl Command for VfsList {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "vfs.list",
            EffectKind::Read,
            "List every file visible through a stack of VFS layers, with the layer that provides it.",
        )
    }

    fn args_schema(&self) -> Value {
        crate::vfs_ops::args_schema()
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args = parse_args(args)?;
        crate::vfs_ops::list_layers(&args).map(Outcome::Data)
    }
}

struct ScriptDiagnostics {
    host: Arc<ScriptHost>,
}

impl ScriptDiagnostics {
    fn file_findings(&self, path: &Path, text: Option<&str>) -> Result<Vec<Finding>, CommandError> {
        let source = match text {
            Some(text) => Cow::Borrowed(text),
            None => Cow::Owned(read_source(path).map_err(|e| failed(e.into()))?),
        };
        let selected = first_arms(&source);
        let tree = parse_tree(&selected);
        let diagnostics = lint_text(path, &tree, &selected)
            .into_iter()
            .chain(self.host.semantic_diagnostics(path, &tree, &selected));
        Ok(diagnostics
            .map(|diagnostic| Finding {
                path: path.to_path_buf(),
                diagnostic,
            })
            .collect())
    }
}

impl Command for ScriptDiagnostics {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.diagnostics",
            EffectKind::Read,
            "Lint Enforce Script: a .c file or a directory tree. Returns MF-E* errors and MF-W* warnings.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to a .c file or a directory" },
                "text": { "type": "string", "description": "lint this text as the content of `path` without reading the file; `path` need not exist" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let TextOrPath { path, text } = parse_args(args)?;
        let findings = match text.as_deref() {
            None if !path.is_file() => lint_target_with(&path, &mut |file, tree, source| {
                self.host.semantic_diagnostics(file, tree, source)
            })
            .map_err(failed)?,
            None if classify(&path) == FileClass::NotEnforce => Vec::new(),
            text => self.file_findings(&path, text)?,
        };
        let out: Result<Vec<Value>, CommandError> = crate::script_bindings::drop_bound(findings)
            .into_iter()
            .map(finding_json)
            .collect();
        Ok(Outcome::Data(Value::Array(out?)))
    }
}

/// Arguments for `script.index`: the workspace root plus optional dependency
/// roots and an explicit game dir (default: host discovery).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexArgs {
    pub root: PathBuf,
    #[serde(default)]
    pub dependencies: Vec<PathBuf>,
    #[serde(default)]
    pub game: Option<PathBuf>,
}

/// Arguments for the buffer-carrying queries (`script.hover`,
/// `script.definition`): the path/id the text belongs to and its live content.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScriptQueryArgs {
    pub path: String,
    pub text: String,
    pub offset: usize,
}

/// Arguments for `script.source`: the `Location.id` to fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceArgs {
    pub id: String,
}

/// Arguments for `script.find_class`: the exact Enforce class name.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FindClassArgs {
    pub name: String,
}

fn script_query_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "file path or source id the text belongs to" },
            "text": { "type": "string", "description": "the live (possibly unsaved) buffer text" },
            "offset": { "type": "integer", "description": "byte offset into `text`" },
        },
        "required": ["path", "text", "offset"],
    })
}

fn validate_offset(text: &str, offset: usize) -> Result<(), CommandError> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(CommandError::InvalidArguments(format!(
            "offset {offset} is not a char boundary in a {}-byte text",
            text.len()
        )));
    }
    Ok(())
}

struct ScriptIndex {
    host: Arc<ScriptHost>,
    settings: Option<PathBuf>,
}

impl Command for ScriptIndex {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.index",
            EffectKind::Read,
            "Build the Enforce Script index from the workspace, dependency and vanilla sources.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "workspace source root to index" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "dependency roots (PBO or dir); left out, the saved dependency mods of the project at `root`" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
            },
            "required": ["root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (args, settings_used) = settings_fill::script_index(self.settings.as_deref(), args)?;
        let args: IndexArgs = parse_args(args)?;
        let mut value = self.host.index(IndexRequest {
            workspace: args.root,
            dependencies: args.dependencies,
            game: args.game,
        });
        settings_used.write_into(&mut value);
        Ok(Outcome::Data(value))
    }
}

struct ScriptIndexStatus {
    host: Arc<ScriptHost>,
}

impl Command for ScriptIndexStatus {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.index_status",
            EffectKind::Read,
            "The script index's state and progress.",
        )
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        Ok(Outcome::Data(self.host.status()))
    }
}

struct ScriptQuery {
    host: Arc<ScriptHost>,
    id: &'static str,
    summary: &'static str,
    run: fn(&ScriptHost, &str, &str, usize) -> Value,
}

impl Command for ScriptQuery {
    fn spec(&self) -> CommandSpec<'static> {
        spec(self.id, EffectKind::Read, self.summary)
    }

    fn args_schema(&self) -> Value {
        script_query_schema()
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: ScriptQueryArgs = parse_args(args)?;
        validate_offset(&args.text, args.offset)?;
        Ok(Outcome::Data((self.run)(
            &self.host,
            &args.path,
            &args.text,
            args.offset,
        )))
    }
}

struct ScriptFindClass {
    host: Arc<ScriptHost>,
}

impl Command for ScriptFindClass {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.find_class",
            EffectKind::Read,
            "Every indexed declaration of a script class, by exact name.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "exact Enforce class name (case-sensitive)" } },
            "required": ["name"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let FindClassArgs { name } = parse_args(args)?;
        Ok(Outcome::Data(self.host.find_class(&name)))
    }
}

/// Arguments for `search.index`; a missing game, model or list uses the default.
#[derive(Debug, Deserialize)]
struct SearchIndexArgs {
    #[serde(default)]
    game: Option<PathBuf>,
    #[serde(default)]
    dependencies: Vec<PathBuf>,
    #[serde(default)]
    docs: Vec<PathBuf>,
    #[serde(default)]
    reference_only: Vec<String>,
    #[serde(default)]
    model: Option<PathBuf>,
}

struct SearchIndex;

impl Command for SearchIndex {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "search.index",
            EffectKind::IndexWrite,
            "Build the search index for a game install, dependencies and docs folders.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "dependency PBOs, mod folders or loose source folders" },
                "docs": { "type": "array", "items": { "type": "string" }, "description": "docs folders of .md and .txt files" },
                "reference_only": { "type": "array", "items": { "type": "string" }, "description": "source labels whose hits are marked reference-only" },
                "model": { "type": "string", "description": "local model folder (default: downloaded once into the app cache)" },
            },
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: SearchIndexArgs = parse_args(args)?;
        let game = crate::sources::game_dir(args.game.as_ref())
            .ok_or_else(|| CommandError::Failed(crate::NO_GAME_INSTALL.into()))?;
        let mut outcome = crate::search_index::run_search_index(
            game.clone(),
            args.dependencies,
            args.docs,
            args.reference_only,
            args.model,
        )
        .map_err(CommandError::Failed)?;
        let snapshots = crate::game_snapshot::default_snapshot_dir();
        if let Some(line) = crate::game_snapshot::auto_snapshot(&game, snapshots.as_deref()) {
            outcome.notes.push(line);
        }
        Ok(Outcome::Data(json_of(&outcome)?))
    }
}

/// Arguments for `search.query`; a missing game uses the default, and `top` is kept to 1..=50.
#[derive(Debug, Deserialize)]
struct SearchQueryArgs {
    question: String,
    #[serde(default)]
    game: Option<PathBuf>,
    #[serde(default)]
    kind: Vec<String>,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default = "default_top")]
    top: usize,
    #[serde(default)]
    model: Option<PathBuf>,
}

fn default_top() -> usize {
    8
}

struct SearchQuery;

impl Command for SearchQuery {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "search.query",
            EffectKind::Read,
            "Search the game's indexed scripts, configs, layouts and docs for a question: fused keyword and meaning ranking, with file, lines and staleness.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "the question in plain words" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "kind": { "type": "array", "items": { "type": "string", "enum": ["script", "config", "layout", "docs"] }, "description": "only pieces of these kinds" },
                "source": { "type": "array", "items": { "type": "string" }, "description": "only pieces from these source labels" },
                "top": { "type": "integer", "minimum": 1, "maximum": 50, "description": "hits to return (default 8)" },
                "model": { "type": "string", "description": "local model folder (default: the app cache)" },
            },
            "required": ["question"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: SearchQueryArgs = parse_args(args)?;
        let kinds = args
            .kind
            .iter()
            .map(|word| kind_named(word))
            .collect::<Result<Vec<_>, _>>()?;
        let game = crate::sources::game_dir(args.game.as_ref())
            .ok_or_else(|| CommandError::Failed(crate::NO_GAME_INSTALL.into()))?;
        let query = crate::search_query::SearchQuery {
            question: args.question,
            kinds,
            sources: args.source,
            top: args.top.clamp(1, 50),
        };
        let report = crate::search_query::run_search_query(game, query, args.model)
            .map_err(CommandError::Failed)?;
        Ok(Outcome::Data(json_of(&report)?))
    }
}

fn kind_named(word: &str) -> Result<Kind, CommandError> {
    match word {
        "script" => Ok(Kind::Script),
        "config" => Ok(Kind::Config),
        "layout" => Ok(Kind::Layout),
        "docs" => Ok(Kind::Docs),
        _ => Err(CommandError::InvalidArguments(format!(
            "unknown kind {word}; use script, config, layout or docs"
        ))),
    }
}

/// Arguments for `layout.check`; a missing game uses the default install.
#[derive(Debug, Deserialize)]
struct LayoutCheckArgs {
    path: PathBuf,
    #[serde(default)]
    root: Option<PathBuf>,
    #[serde(default)]
    game: Option<PathBuf>,
    #[serde(default)]
    dependencies: Vec<PathBuf>,
}

struct LayoutCheck {
    settings: Option<PathBuf>,
}

/// Fills a missing `dependencies` argument from the project's saved settings; returns the keys it used.
fn fill_dependencies(
    settings: Option<&Path>,
    args: &mut Value,
) -> Result<Vec<String>, CommandError> {
    if args.get("dependencies").is_some() {
        return Ok(Vec::new());
    }
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    let root = match args.get("root").and_then(Value::as_str) {
        Some(root) => PathBuf::from(root),
        None => crate::layout_check::mod_root(Path::new(path), None),
    };
    let (filled, settings_used) = settings_fill::script_index(settings, json!({ "root": root }))?;
    if let Some(dependencies) = filled.get("dependencies") {
        args["dependencies"] = dependencies.clone();
    }
    Ok(settings_used.keys)
}

impl Command for LayoutCheck {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "layout.check",
            EffectKind::Read,
            "Check .layout files against the game's widget classes, properties, images, fonts and styles.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "a .layout file or a folder of them" },
                "root": { "type": "string", "description": "mod root (default: the nearest folder with a config.cpp)" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "dependency PBOs, mod folders or loose source folders; left out, the saved dependency mods of the project at root (or the nearest folder above path holding a config.cpp)" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, mut args: Value) -> Result<Outcome, CommandError> {
        let settings_used = fill_dependencies(self.settings.as_deref(), &mut args)?;
        let args: LayoutCheckArgs = parse_args(args)?;
        let game = crate::sources::game_dir(args.game.as_ref())
            .ok_or_else(|| CommandError::Failed(crate::NO_GAME_INSTALL.into()))?;
        let cache = modfoundry_platform::app_cache_dir().ok_or_else(|| {
            CommandError::Failed("no cache folder: set XDG_CACHE_HOME or HOME".into())
        })?;
        let request = crate::layout_check::LayoutCheckRequest {
            path: args.path,
            root: args.root,
            game,
            dependencies: args.dependencies,
        };
        let report =
            crate::layout_check::check_layouts(&request, &cache).map_err(CommandError::Failed)?;
        let mut value = json_of(&report)?;
        value["settings_used"] = json!(settings_used);
        Ok(Outcome::Data(value))
    }
}

/// Arguments for `layout.render`; the PNG is written only with `out`.
#[derive(Debug, Deserialize)]
struct LayoutRenderArgs {
    path: PathBuf,
    #[serde(default)]
    root: Option<PathBuf>,
    #[serde(default)]
    game: Option<PathBuf>,
    #[serde(default)]
    dependencies: Vec<PathBuf>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    show_hidden: bool,
    #[serde(default)]
    boxes: bool,
    #[serde(default)]
    out: Option<PathBuf>,
}

struct LayoutRender {
    settings: Option<PathBuf>,
}

impl LayoutRender {
    /// The report as data, and the change-set of the PNG when `out` is given.
    fn run(
        &self,
        mut args: Value,
        write: bool,
    ) -> Result<(Value, Option<ChangeSet>), CommandError> {
        let settings_used = fill_dependencies(self.settings.as_deref(), &mut args)?;
        let args: LayoutRenderArgs = parse_args(args)?;
        if let Some(out) = &args.out {
            check_out(out).map_err(CommandError::InvalidArguments)?;
        }
        let game = crate::sources::game_dir(args.game.as_ref())
            .ok_or_else(|| CommandError::Failed(crate::NO_GAME_INSTALL.into()))?;
        let size = args
            .size
            .as_deref()
            .map(parse_size)
            .transpose()
            .map_err(CommandError::InvalidArguments)?;
        let background = args
            .background
            .as_deref()
            .map(parse_background)
            .transpose()
            .map_err(CommandError::InvalidArguments)?;
        let request = RenderRequest::new(
            RenderArgs {
                path: args.path,
                root: args.root,
                dependencies: args.dependencies,
                size,
                background,
                show_hidden: args.show_hidden,
                language: args.language,
                boxes: args.boxes,
            },
            game,
        );
        let output = render_layout(&request).map_err(CommandError::Failed)?;
        let mut value = json_of(&output.report)?;
        let change_set = match &args.out {
            Some(out) => Some(picture_change(out, &output.image, write).map_err(failed)?),
            None => None,
        };
        value["png"] = json!(args.out);
        value["settings_used"] = json!(settings_used);
        Ok((value, change_set))
    }
}

impl Command for LayoutRender {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "layout.render",
            EffectKind::Mutate,
            "Draw a .layout to a PNG from the game, its dependencies and the mod's files; the findings list what could not be drawn.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "one .layout file" },
                "root": { "type": "string", "description": "mod root (default: the nearest folder with a config.cpp)" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "dependency PBOs, mod folders or loose source folders; left out, the saved dependency mods of the project" },
                "size": { "type": "string", "description": "WIDTHxHEIGHT (default 1920x1080)" },
                "background": { "type": "string", "description": "backdrop colour as #rrggbb or #rrggbbaa (default #202020)" },
                "show_hidden": { "type": "boolean", "description": "draw hidden widgets faded" },
                "language": { "type": "string", "description": "stringtable language for #keys (default english)" },
                "boxes": { "type": "boolean", "description": "list every widget's box in the report" },
                "out": { "type": "string", "description": "the PNG file to write; it must end in .png and is overwritten" },
            },
            "required": ["path"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        match self.run(args, false)?.1 {
            Some(change_set) => planned(&change_set),
            None => Ok(None),
        }
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (value, _) = self.run(args, true)?;
        Ok(Outcome::Data(value))
    }
}

/// Arguments for `game.diff`; a missing snapshots folder is the one in mf's data folder.
#[derive(Debug, Deserialize)]
struct GameDiffArgs {
    old: String,
    new: String,
    #[serde(default)]
    against: Vec<PathBuf>,
    #[serde(default)]
    snapshots: Option<PathBuf>,
}

struct GameDiff;

impl Command for GameDiff {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "game.diff",
            EffectKind::Read,
            "Compare two DayZ builds' scripts, configs and layouts and list where mods use what changed.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "old": { "type": "string", "description": "a game install, an unpacked game folder, a .pbo, a snapshot .json or a saved build number" },
                "new": { "type": "string", "description": "a game install, an unpacked game folder, a .pbo, a snapshot .json or a saved build number" },
                "against": { "type": "array", "items": { "type": "string" }, "description": "mod folders to check" },
                "snapshots": { "type": "string", "description": "folder of saved builds, default mf's data folder" },
            },
            "required": ["old", "new"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: GameDiffArgs = parse_args(args)?;
        let snapshots = args
            .snapshots
            .or_else(crate::game_snapshot::default_snapshot_dir);
        let request = crate::game_diff::GameDiffRequest {
            old: args.old,
            new: args.new,
            against: args.against,
            snapshots,
        };
        let report = crate::game_diff::game_diff(&request).map_err(CommandError::Failed)?;
        Ok(Outcome::Data(json_of(&report)?))
    }
}

/// Arguments for `conflict.check`: two or more mod folders, or a launch set of a project.
#[derive(Debug, Deserialize)]
struct ConflictCheckArgs {
    #[serde(default)]
    mods: Vec<PathBuf>,
    #[serde(default)]
    set: Option<String>,
    #[serde(default)]
    root: Option<PathBuf>,
}

struct ConflictCheck;

impl Command for ConflictCheck {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "conflict.check",
            EffectKind::Read,
            "List what several mods declare or modify in common: scripts, configs and packed files, in load order.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mods": { "type": "array", "items": { "type": "string" }, "description": "mod folders in load order; two or more" },
                "set": { "type": "string", "description": "a launch set of the project at `root`, in place of `mods`" },
                "root": { "type": "string", "description": "the project whose server/launch-sets.json holds the set; default the current folder" },
            },
            "description": "give mods, or set (with root), not both",
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: ConflictCheckArgs = parse_args(args)?;
        let roots = check_roots(args)?;
        let report = crate::conflict_check::conflict_check(&roots).map_err(CommandError::Failed)?;
        Ok(Outcome::Data(json_of(&report)?))
    }
}

/// The folders `conflict.check` reads, from the one form its arguments name.
fn check_roots(args: ConflictCheckArgs) -> Result<Vec<PathBuf>, CommandError> {
    distinct_roots(given_roots(args)?)
}

/// Two entries that resolve to one folder would report every name as shared with itself.
fn distinct_roots(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>, CommandError> {
    let mut seen = std::collections::HashSet::new();
    for root in &roots {
        if !seen.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone())) {
            return Err(CommandError::InvalidArguments(format!(
                "the same mod folder is given twice: {}",
                root.display()
            )));
        }
    }
    Ok(roots)
}

fn given_roots(args: ConflictCheckArgs) -> Result<Vec<PathBuf>, CommandError> {
    let ConflictCheckArgs { mods, set, root } = args;
    let gave_root = root.is_some();
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    match (mods.is_empty(), set) {
        (false, None) if gave_root => Err(CommandError::InvalidArguments(
            "root belongs to a launch set; pass either mods, or set with root".to_owned(),
        )),
        (false, None) if mods.len() < 2 => Err(CommandError::InvalidArguments(
            crate::conflict_check::NEEDS_TWO_MODS.to_owned(),
        )),
        (false, None) => Ok(mods),
        (true, Some(set)) => {
            crate::conflict_check::set_roots(&root, &set).map_err(CommandError::InvalidArguments)
        }
        (false, Some(_)) => Err(CommandError::InvalidArguments(
            "give either mods, or set, not both".to_owned(),
        )),
        (true, None) => Err(CommandError::InvalidArguments(
            "a conflict check needs mods, or a set with root; pass two or more mods, or set and root"
                .to_owned(),
        )),
    }
}

/// Arguments for `game.snapshot`; a missing snapshots folder is the one in mf's data folder.
#[derive(Debug, Deserialize)]
struct GameSnapshotArgs {
    #[serde(default)]
    game: Option<PathBuf>,
    #[serde(default)]
    snapshots: Option<PathBuf>,
}

struct GameSnapshot;

impl Command for GameSnapshot {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "game.snapshot",
            EffectKind::IndexWrite,
            "Save a snapshot of a DayZ install's scripts, configs and layouts under its build number, for game.diff.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "snapshots": { "type": "string", "description": "folder of saved builds, default mf's data folder" },
            },
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: GameSnapshotArgs = parse_args(args)?;
        let game = crate::sources::game_dir(args.game.as_ref())
            .ok_or_else(|| CommandError::Failed(crate::NO_GAME_INSTALL.into()))?;
        let snapshots = args
            .snapshots
            .or_else(crate::game_snapshot::default_snapshot_dir)
            .ok_or_else(|| {
                CommandError::Failed("no data folder for snapshots: set --snapshots".into())
            })?;
        let saved = crate::game_snapshot::snapshot_install(&game, &snapshots)
            .map_err(CommandError::Failed)?;
        Ok(Outcome::Data(json_of(&saved)?))
    }
}

struct ScriptOutline;

impl Command for ScriptOutline {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.outline",
            EffectKind::Read,
            "The parsed outline of an Enforce file or folder as JSON: per file its layer, preprocessor blocks, classes, members, calls and strings (schema 1).",
        )
    }

    fn args_schema(&self) -> Value {
        path_schema("file or folder to outline")
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let PathArgs { path } = parse_args(args)?;
        let tree = outline_tree(&path).map_err(|e| failed(e.into()))?;
        Ok(Outcome::Data(json_of(&tree)?))
    }
}

struct ScriptCheck {
    settings: Option<PathBuf>,
}

impl Command for ScriptCheck {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.check",
            EffectKind::IndexWrite,
            "The compile errors the game itself would print for a mod, without starting it, for the retail server, Diag server and Diag client. Give it the mod and every mod it depends on, as source folders or packed mods; packed mods and the game's scripts are unpacked into mf's cache once.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "the mod to check: its source folder, or a packed or unpacked mod folder" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "folders of the mods it needs, as source trees or packed or unpacked mods; left out, the saved dependency mods of the project at root; without them every type they declare is reported as unknown" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery); its dta/scripts.pbo is unpacked into the cache" },
                "vanilla": { "type": "string", "description": "the game's scripts already unpacked: the folder that holds 1_Core to 5_Mission; used instead of game" },
                "cache": { "type": "string", "description": "folder for unpacked copies (default: mf's cache folder)" },
                "targets": { "type": "array", "items": { "type": "string", "enum": TARGETS.map(target_name) }, "description": "the game builds to check; all three when left out" },
            },
            "required": ["root"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (args, settings_used) = settings_fill::script_index(self.settings.as_deref(), args)?;
        let args: ScriptCheckArgs = parse_args(args)?;
        require_folder("root", &args.root)?;
        for dependency in &args.dependencies {
            require_folder("dependency", dependency)?;
        }
        let targets = check_targets(&args.targets)?;
        let cache = args.cache.clone().or_else(unpack_cache::default_root);
        if let Some(cache) = &cache {
            let mods = std::iter::once(&args.root).chain(&args.dependencies);
            check_cache(cache, mods)?;
        }
        let vanilla = vanilla_source(&args)?;
        let packed = !pbo_ops::pbos_of(&args.root).is_empty();
        let mods = mods_once(args.dependencies, args.root);
        let sources = unpack_sources(cache.as_deref(), vanilla, mods)
            .map_err(|error| CommandError::Failed(format!("{error:#}")))?;
        let mut check = check_folders(&sources.vanilla, &sources.mods, &targets);
        if packed {
            check.checked_from = sources.mods.last().map(|(_, folder)| folder.clone());
        }
        check.notes = sources.notes.into_iter().chain(check.notes).collect();
        let mut value = json_of(&check)?;
        settings_used.write_into(&mut value);
        Ok(Outcome::Data(value))
    }
}

/// Refuses a cache that is a file, or one inside a mod given: the loader walks a mod's whole
/// folder, so copies kept there would be compiled as part of it.
fn check_cache<'a>(
    cache: &Path,
    mods: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<(), CommandError> {
    let refuse = |reason: String| Err(CommandError::InvalidArguments(reason));
    if cache.exists() && !cache.is_dir() {
        return refuse(format!("cache {} is not a folder", cache.display()));
    }
    // Both forms count: a link inside a mod may still be walked as part of it.
    let (given, resolved) = (folded(cache), resolved_through_links(cache));
    for folder in mods {
        if given.starts_with(folded(folder)) || resolved.starts_with(resolved_through_links(folder))
        {
            return refuse(format!(
                "cache {} is inside {}: its copies would be read as part of that mod",
                cache.display(),
                folder.display()
            ));
        }
    }
    Ok(())
}

fn vanilla_source(args: &ScriptCheckArgs) -> Result<VanillaSource, CommandError> {
    if let Some(vanilla) = &args.vanilla {
        require_folder("vanilla", vanilla)?;
        return Ok(VanillaSource::Folder(vanilla.clone()));
    }
    crate::sources::game_dir(args.game.as_ref())
        .map(VanillaSource::Game)
        .ok_or_else(|| CommandError::Failed(format!("{NO_GAME_INSTALL}: pass game or vanilla")))
}

fn check_targets(names: &[String]) -> Result<Vec<Target>, CommandError> {
    if names.is_empty() {
        return Ok(TARGETS.to_vec());
    }
    let mut targets = Vec::new();
    for name in names {
        let target = parse_target(name).ok_or_else(|| {
            CommandError::InvalidArguments(format!(
                "unknown target {name}: use retail-server, diag-server or diag-client"
            ))
        })?;
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(targets)
}

fn require_folder(argument: &str, path: &Path) -> Result<(), CommandError> {
    if path.is_dir() {
        return Ok(());
    }
    Err(CommandError::InvalidArguments(format!(
        "{argument} {} is not a folder",
        path.display()
    )))
}

/// The dependencies in the order given, then the mod, each folder once and named by its
/// folder: one given twice would be compiled twice, and every class in it reported as
/// declared twice.
fn mods_once(dependencies: Vec<PathBuf>, root: PathBuf) -> Vec<(String, PathBuf)> {
    let resolved =
        |folder: &Path| std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let named = |folder: PathBuf, resolved: &Path| {
        let name = resolved.file_name().unwrap_or(folder.as_os_str());
        (name.to_string_lossy().into_owned(), folder)
    };
    let root_resolved = resolved(&root);
    let mut seen = std::collections::HashSet::from([root_resolved.clone()]);
    let mut mods = Vec::with_capacity(dependencies.len() + 1);
    for dependency in dependencies {
        let at = resolved(&dependency);
        if seen.insert(at.clone()) {
            mods.push(named(dependency, &at));
        }
    }
    mods.push(named(root, &root_resolved));
    mods
}

struct ScriptSourceCommand {
    host: Arc<ScriptHost>,
}

impl Command for ScriptSourceCommand {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "script.source",
            EffectKind::Read,
            "Fetch an indexed source's text by its Location id (read-only when not workspace).",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string", "description": "a Location id (file path or pbo:… id)" } },
            "required": ["id"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let SourceArgs { id } = parse_args(args)?;
        self.host.source(&id).map(Outcome::Data)
    }
}

struct ConfigIndex {
    host: Arc<ConfigHost>,
}

impl Command for ConfigIndex {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.index",
            EffectKind::Read,
            "Load every vanilla, dependency and workspace config into the session's resolver.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "workspace source root to index" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "dependency roots (PBO or dir)" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
            },
            "required": ["root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: IndexArgs = parse_args(args)?;
        Ok(Outcome::Data(self.host.index(ConfigIndexRequest {
            workspace: args.root,
            dependencies: args.dependencies,
            game: args.game,
        })))
    }
}

/// Arguments for `config.patches`: the mod root to inspect.
#[derive(Debug, Clone, Deserialize)]
struct PatchesArgs {
    pub root: PathBuf,
}

struct ConfigPatches;

impl Command for ConfigPatches {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.patches",
            EffectKind::Read,
            "Read the CfgPatches and requiredAddons one mod declares, without the index.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "a .pbo, a dir with addons/*.pbo, or a source dir of loose config.cpp files" },
            },
            "required": ["root"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let PatchesArgs { root } = parse_args(args)?;
        crate::config_host::patches(&root).map(Outcome::Data)
    }
}

/// Arguments for `config.resolve`: a config path or an indexed source id, the
/// path's optional unsaved text, and an optional class path to resolve.
#[derive(Debug, Clone, Deserialize)]
struct ResolveArgs {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub class: Option<Vec<String>>,
}

struct ConfigResolve {
    host: Arc<ConfigHost>,
}

impl Command for ConfigResolve {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.resolve",
            EffectKind::Read,
            "One config file's externals, or one class resolved against the session's index.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the config.cpp (or its directory) to resolve; one of `path` or `source`" },
                "source": { "type": "string", "description": "an indexed config source id; one of `path` or `source`" },
                "text": { "type": "string", "description": "unsaved text to resolve as this path's content" },
                "class": { "type": "array", "items": { "type": "string" }, "description": "class path; omit for the file's forward-declared externals" },
            },
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ResolveArgs {
            path,
            source,
            text,
            class,
        } = parse_args(args)?;
        let path = path.as_deref().map(crate::config_file);
        self.host
            .resolve(
                path.as_deref(),
                source.as_deref(),
                text.as_deref(),
                class.as_deref(),
            )
            .map(Outcome::Data)
    }
}

/// Arguments for `config.source`: one loaded config by id or by vpath.
#[derive(Debug, Clone, Deserialize)]
struct ConfigSourceArgs {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub vpath: Option<String>,
}

struct ConfigSource {
    host: Arc<ConfigHost>,
}

impl Command for ConfigSource {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.source",
            EffectKind::Read,
            "One loaded config's raw text, by source id or virtual path.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "config source id" },
                "vpath": { "type": "string", "description": "virtual path (case-insensitive; the last source loaded wins)" },
            },
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ConfigSourceArgs { id, vpath } = parse_args(args)?;
        self.host
            .source(id.as_deref(), vpath.as_deref())
            .map(Outcome::Data)
    }
}

/// Arguments for `ce.index`: the workspace root and optional overrides for the
/// three reference discovery paths.
#[derive(Debug, Clone, Deserialize)]
struct EconomyIndexArgs {
    pub root: PathBuf,
    #[serde(default)]
    pub mission: Option<PathBuf>,
    #[serde(default)]
    pub server: Option<PathBuf>,
    #[serde(default)]
    pub game: Option<PathBuf>,
}

/// Arguments for `ce.reference`: the CE kind whose reference rows to read.
#[derive(Debug, Clone, Deserialize)]
struct CeReferenceArgs {
    pub kind: CeKind,
}

/// Arguments for `ce.source`: the `mission:<rel>` id to fetch.
#[derive(Debug, Clone, Deserialize)]
struct CeSourceArgs {
    pub id: String,
}

struct CeIndex {
    host: Arc<EconomyHost>,
}

impl Command for CeIndex {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.index",
            EffectKind::Read,
            "Load the reference mission and the workspace's Central Economy files into the session's economy.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "the project's source root to index" },
                "mission": { "type": "string", "description": "an explicit mission folder (default: discovery)" },
                "server": { "type": "string", "description": "a DayZ server dir (default: host discovery)" },
                "game": { "type": "string", "description": "a DayZ install dir (default: host discovery)" },
            },
            "required": ["root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: EconomyIndexArgs = parse_args(args)?;
        Ok(Outcome::Data(self.host.index(EconomyIndexRequest {
            workspace: args.root,
            mission: args.mission,
            server: args.server,
            game: args.game,
        })))
    }
}

struct CeReference {
    host: Arc<EconomyHost>,
}

impl Command for CeReference {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.reference",
            EffectKind::Read,
            "The reference mission's loaded records of one Central Economy kind, later files overriding earlier ones.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "the CE kind, snake_case (e.g. `types`)" },
            },
            "required": ["kind"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let CeReferenceArgs { kind } = parse_args(args)?;
        Ok(Outcome::Data(self.host.reference(kind)))
    }
}

struct CeSource {
    host: Arc<EconomyHost>,
}

impl Command for CeSource {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.source",
            EffectKind::Read,
            "One reference mission file by its `mission:<rel>` id: its display path, kind and text.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "a reference file id, `mission:<rel>`" },
            },
            "required": ["id"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let CeSourceArgs { id } = parse_args(args)?;
        Ok(Outcome::Data(self.host.source(&id)?))
    }
}

struct CeGetTree;

impl Command for CeGetTree {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.get_tree",
            EffectKind::Read,
            "The kind, record sections and XML parse issues of one Central Economy XML file.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to a Central Economy XML file" },
                "text": { "type": "string", "description": "parse this text as the content of `path` without reading the file; `path` need not exist" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let TextOrPath { path, text } = parse_args(args)?;
        ce_tree(&path, text.as_deref())
            .map(Outcome::Data)
            .map_err(failed)
    }
}

struct CeValidate {
    host: Arc<EconomyHost>,
    config: Arc<ConfigHost>,
}

impl Command for CeValidate {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.validate",
            EffectKind::Read,
            "Lint a Central Economy XML file, or every CE file under a directory, against the schema.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to a Central Economy XML file or a directory of them" },
                "text": { "type": "string", "description": "validate this text as the content of `path` without reading the file" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let TextOrPath { path, text } = parse_args(args)?;
        if path.is_dir() && text.is_some() {
            return Err(CommandError::InvalidArguments(
                "`text` cannot be given for a directory".into(),
            ));
        }
        let findings = ce_validate(
            &path,
            text.as_deref(),
            &self.host,
            ConfigLookup::from_host(&self.config).as_ref(),
        )
        .map_err(failed)?;
        Ok(Outcome::Data(json_of(&findings)?))
    }
}

struct CeCoverage {
    economy: Arc<EconomyHost>,
    config: Arc<ConfigHost>,
}

impl Command for CeCoverage {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.coverage",
            EffectKind::Read,
            "The workspace's spawnable classes with and without a types entry, and their MF-X120 hints.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        let empty = json!({
            "ready": false,
            "classes": [],
            "covered": 0,
            "missing": 0,
            "findings": [],
        });
        let (Some(loaded), true) = (self.economy.loaded(), self.config.loaded().is_some()) else {
            return Ok(Outcome::Data(empty));
        };
        let classes = ce_classes::workspace_classes(&self.config);
        let coverage = classes::coverage(&classes, &loaded.economy);
        let findings: Vec<FindingReport> = classes::coverage_findings(&coverage)
            .into_iter()
            .map(|(file, diagnostic)| FindingReport {
                diagnostic,
                path: PathBuf::from(file),
            })
            .collect();
        Ok(Outcome::Data(json!({
            "ready": true,
            "classes": json_of(&coverage.classes)?,
            "covered": coverage.covered,
            "missing": coverage.missing,
            "findings": json_of(&findings)?,
        })))
    }
}

struct CeEdit;

impl Command for CeEdit {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "ce.edit",
            EffectKind::Mutate,
            "Edit a DayZ Central Economy XML file by records (preview-first: `preview` returns the change-set). Ops keep comments and formatting.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the Central Economy XML file the edit targets" },
                "ops": {
                    "type": "array",
                    "description": "record edits applied in order, e.g. [{\"set_field\": {\"record\": {\"key\": \"FieldCanteen\"}, \"field\": \"nominal\", \"value\": \"12\"}}]",
                    "items": ce_edit_op_schema(),
                },
                "expected_sha256": expected_sha256_property(),
            },
            "required": ["path", "ops"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: CeEditArgs = parse_args(args)?;
        let text = planned_ce(&args)?;
        plan_ops_edit(&args.path, &text, args.expected_sha256.as_deref())
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: CeEditArgs = parse_args(args)?;
        let text = planned_ce(&args)?;
        apply_ops_edit(
            &args.path,
            &text,
            args.expected_sha256.as_deref(),
            args.ops.len(),
            "record",
        )
    }
}

struct ConfigGetTree;

impl Command for ConfigGetTree {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.get_tree",
            EffectKind::Read,
            "The class tree and parse issues of one config.cpp, without resolution.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to config.cpp or its directory" },
                "text": { "type": "string", "description": "outline this text as the content of `path` without reading the file; `path` need not exist" },
                "class": { "type": "array", "items": { "type": "string" }, "description": "class path to return a ClassView for (case-insensitive)" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let GetTreeArgs { source, class } = parse_args(args)?;
        let text = source.read()?;
        let (outline, diagnostics) = modfoundry_config::outline::parse_outline(&text);
        let mut payload = serde_json::Map::new();
        payload.insert("outline".into(), json_of(&outline)?);
        payload.insert("diagnostics".into(), json_of(&diagnostics)?);
        if let Some(path) = class {
            let view = modfoundry_config::outline::class_view(&text, &path);
            payload.insert("class".into(), json_of(&view)?);
        }
        Ok(Outcome::Data(Value::Object(payload)))
    }
}

struct ConfigValidate {
    host: Arc<ConfigHost>,
}

impl Command for ConfigValidate {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.validate",
            EffectKind::Read,
            "Validate a DayZ config.cpp (file or containing dir). Returns MF-C* diagnostics. A config.cpp the project's .pboignore leaves out gets no findings.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to config.cpp or its directory" },
                "text": { "type": "string", "description": "validate this text as the content of `path` without reading the file; `path` need not exist" },
            },
            "required": ["path"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let TextOrPath { path, text } = parse_args(args)?;
        let left_out = left_out_by_pboignore(&path);
        let (mut diagnostics, indexed) = match text {
            Some(text) => (
                modfoundry_config::validate_text(&text, Some(config_root(&path))).diagnostics,
                text,
            ),
            None => {
                let (read, diagnostics) = validate_file(&path).map_err(failed)?;
                (diagnostics, read)
            }
        };
        if left_out {
            return Ok(Outcome::Diagnostics {
                count: 0,
                diagnostics: Vec::new(),
            });
        }
        diagnostics.extend(
            self.host
                .index_findings(&crate::config_file(&path), &indexed),
        );
        Ok(Outcome::Diagnostics {
            count: diagnostics.len(),
            diagnostics,
        })
    }
}

/// Whether a `.pboignore` rule leaves `config` out of the PBO, seen from the nearest folder
/// above it that holds a `.pboignore`.
fn left_out_by_pboignore(config: &Path) -> bool {
    let file = crate::config_file(config);
    // Resolve the folder, not the file: a linked config.cpp follows the rules beside the link.
    let (Some(parent), Some(name)) = (file.parent(), file.file_name()) else {
        return false;
    };
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    let file = parent.join(name);
    let Some(folder) = parent
        .ancestors()
        .find(|folder| folder.join(".pboignore").is_file())
    else {
        return false;
    };
    let Ok(rel) = file.strip_prefix(folder) else {
        return false;
    };
    let rel = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let Ok(rules) = modfoundry_build::pack_rules::load(folder) else {
        return false;
    };
    let decision = rules.decide(&rel);
    !decision.included
        && matches!(
            decision.reason,
            modfoundry_build::pack_rules::Reason::Rule { .. }
        )
}

fn config_path_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "class names from the file root down; matching is case-insensitive",
    })
}

fn config_target_schema() -> Value {
    json!({
        "description": "the class or property an op addresses",
        "oneOf": [
            {
                "type": "object",
                "properties": { "class": config_path_schema() },
                "required": ["class"],
            },
            {
                "type": "object",
                "properties": {
                    "property": {
                        "type": "object",
                        "properties": {
                            "class": config_path_schema(),
                            "name": { "type": "string" },
                        },
                        "required": ["class", "name"],
                    },
                },
                "required": ["property"],
            },
        ],
    })
}

fn config_value_schema(nesting: u8) -> Value {
    let items = if nesting == 0 {
        json!({ "description": "a value" })
    } else {
        config_value_schema(nesting - 1)
    };
    json!({
        "description": "a value to write; numbers and identifiers are spelled exactly as given",
        "oneOf": [
            { "type": "object", "properties": { "str": { "type": "string" } }, "required": ["str"] },
            { "type": "object", "properties": { "number": { "type": "string" } }, "required": ["number"] },
            { "type": "object", "properties": { "ident": { "type": "string" } }, "required": ["ident"] },
            {
                "type": "object",
                "properties": { "array": { "type": "array", "items": items } },
                "required": ["array"],
            },
        ],
    })
}

fn config_position_schema() -> Value {
    json!({
        "description": "where among its siblings a new member goes",
        "oneOf": [
            { "type": "string", "enum": ["first", "last"] },
            {
                "type": "object",
                "properties": { "before": { "type": "string" } },
                "required": ["before"],
            },
            {
                "type": "object",
                "properties": { "after": { "type": "string" } },
                "required": ["after"],
            },
        ],
    })
}

fn config_edit_op_schema() -> Value {
    let target = config_target_schema();
    let value = config_value_schema(4);
    let position = config_position_schema();
    let class = config_path_schema();
    let object = |properties: Value, required: Value| json!({ "type": "object", "properties": properties, "required": required });
    let op = |tag: &str, body: Value| json!({ "type": "object", "properties": { tag: body }, "required": [tag] });
    json!({
        "description": "one structured edit; the key names the op and holds its arguments",
        "oneOf": [
            op("set_value", object(
                json!({ "property": target.clone(), "value": value.clone() }),
                json!(["property", "value"]),
            )),
            op("add_property", object(
                json!({ "class": class.clone(), "name": { "type": "string" }, "value": value.clone(), "position": position.clone() }),
                json!(["class", "name", "value", "position"]),
            )),
            op("remove", object(
                json!({ "target": target.clone() }),
                json!(["target"]),
            )),
            op("rename", object(
                json!({ "target": target.clone(), "new_name": { "type": "string" } }),
                json!(["target", "new_name"]),
            )),
            op("add_class", object(
                json!({
                    "parent": class.clone(),
                    "name": { "type": "string" },
                    "base": { "type": ["string", "null"], "description": "the base class name, null for none" },
                    "forward": { "type": "boolean", "description": "declare the class forward only" },
                    "position": position.clone(),
                }),
                json!(["parent", "name", "position"]),
            )),
            op("set_base", object(
                json!({ "class": class.clone(), "base": { "type": ["string", "null"] } }),
                json!(["class", "base"]),
            )),
            op("array_insert", object(
                json!({ "property": target.clone(), "index": { "type": "integer", "minimum": 0 }, "value": value.clone() }),
                json!(["property", "index", "value"]),
            )),
            op("array_remove", object(
                json!({ "property": target.clone(), "index": { "type": "integer", "minimum": 0 } }),
                json!(["property", "index"]),
            )),
            op("array_set", object(
                json!({ "property": target.clone(), "index": { "type": "integer", "minimum": 0 }, "value": value.clone() }),
                json!(["property", "index", "value"]),
            )),
            op("array_move", object(
                json!({ "property": target.clone(), "from": { "type": "integer", "minimum": 0 }, "to": { "type": "integer", "minimum": 0 } }),
                json!(["property", "from", "to"]),
            )),
            op("set_array", object(
                json!({ "target": target.clone(), "array": { "type": "boolean", "description": "true writes `name[]`, false drops the brackets; the value must already fit" } }),
                json!(["target", "array"]),
            )),
            op("remove_at", object(
                json!({ "class": class.clone(), "name": { "type": "string" }, "occurrence": { "type": "integer", "minimum": 0, "description": "0-based among same-named siblings; use it to remove one duplicate" } }),
                json!(["class", "name", "occurrence"]),
            )),
        ],
    })
}

fn planned_config(args: &ConfigEditArgs) -> Result<String, CommandError> {
    let text = read_utf8(&args.path)?;
    plan_all(&text, &args.ops).map_err(|e| CommandError::InvalidArguments(e.to_string()))
}

fn planned_ce(args: &CeEditArgs) -> Result<String, CommandError> {
    let text = read_utf8(&args.path)?;
    let name = args.path.file_name().and_then(|name| name.to_str());
    modfoundry_ce::ops::plan_all(&text, name, &args.ops)
        .map_err(|e| CommandError::InvalidArguments(e.to_string()))
}

struct ConfigEdit;

impl Command for ConfigEdit {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.edit",
            EffectKind::Mutate,
            "Edit a DayZ config.cpp structurally (preview-first: `preview` returns the change-set). Ops keep comments and formatting.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the config.cpp the edit targets" },
                "ops": {
                    "type": "array",
                    "description": "structured edits applied in order, e.g. [{\"add_property\": {\"class\": [\"CfgVehicles\", \"Item\"], \"name\": \"scope\", \"value\": {\"number\": \"2\"}, \"position\": \"last\"}}]",
                    "items": config_edit_op_schema(),
                },
                "expected_sha256": expected_sha256_property(),
            },
            "required": ["path", "ops"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: ConfigEditArgs = parse_args(args)?;
        let text = planned_config(&args)?;
        plan_ops_edit(&args.path, &text, args.expected_sha256.as_deref())
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: ConfigEditArgs = parse_args(args)?;
        let text = planned_config(&args)?;
        apply_ops_edit(
            &args.path,
            &text,
            args.expected_sha256.as_deref(),
            args.ops.len(),
            "structured",
        )
    }
}

struct ConfigBinarize;

impl Command for ConfigBinarize {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.binarize",
            EffectKind::Mutate,
            "Binarize a config.cpp into RaP config.bin (CfgConvert -bin parity).",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "config.cpp to binarize" },
                "out": { "type": "string", "description": "output path (default: config.bin beside the input)" },
            },
            "required": ["path"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let ConvertArgs { path, out } = parse_args(args)?;
        let change_set = plan_binarize(&path, out.as_deref()).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ConvertArgs { path, out } = parse_args(args)?;
        let (output, bytes, change_set) = binarize_to(&path, out.as_deref()).map_err(failed)?;
        applied(
            1,
            format!(
                "binarized {} -> {} ({bytes} bytes)",
                path.display(),
                output.display()
            ),
            &change_set,
        )
    }
}

struct ConfigDerapify;

impl Command for ConfigDerapify {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "config.derapify",
            EffectKind::Mutate,
            "Convert a RaP config.bin back into config text.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "config.bin to convert" },
                "out": { "type": "string", "description": "output path (default: return the text)" },
            },
            "required": ["path"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let ConvertArgs { path, out } = parse_args(args)?;
        let out = out.ok_or_else(|| {
            CommandError::InvalidArguments(
                "config.derapify writes a file only with `out`; without it the text is the result"
                    .into(),
            )
        })?;
        let change_set = plan_derapify(&path, &out).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ConvertArgs { path, out } = parse_args(args)?;
        let Some(out) = out else {
            return Ok(Outcome::Data(
                json!(derapify_config(&path).map_err(failed)?),
            ));
        };
        let change_set = derapify_to(&path, &out).map_err(failed)?;
        applied(
            1,
            format!("derapified {} -> {}", path.display(), out.display()),
            &change_set,
        )
    }
}

struct ImageConvert;

/// The `image.convert` arguments, with a bad value refused before any file is read.
fn image_args(args: Value) -> Result<ImageArgs, CommandError> {
    let args: ImageArgs = parse_args(args)?;
    image_ops::target_of(&args).map_err(|e| CommandError::InvalidArguments(e.to_string()))?;
    Ok(args)
}

impl Command for ImageConvert {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "image.convert",
            EffectKind::Mutate,
            "Convert a PNG, or every PNG in a folder, into a DayZ .edds or .paa texture. A folder whose names differ only in case, or are not valid UTF-8, is refused whole.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "a PNG file, or a folder whose PNGs (up to six folders deep) are all converted" },
                "to": { "type": "string", "enum": ["edds", "paa"] },
                "out": { "type": "string", "description": "for a file, the output file; for a folder, the folder the outputs mirror into (default: beside each PNG)" },
                "format": { "type": "string", "enum": ["dxt1", "dxt5"], "description": "only with \"paa\" (default: picked from the image)" },
            },
            "required": ["path", "to"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args = image_args(args)?;
        let change_set = image_ops::plan_convert_images(&args).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args = image_args(args)?;
        let (summary, change_set) = image_ops::convert_images(&args).map_err(failed)?;
        applied(change_set.len(), summary, &change_set)
    }
}

struct ImagesetBuild;

/// The `imageset.build` arguments, with a bad value refused before any PNG is read.
fn imageset_args(args: Value) -> Result<ImagesetArgs, CommandError> {
    let args: ImagesetArgs = parse_args(args)?;
    imageset_ops::check_args(&args).map_err(|e| CommandError::InvalidArguments(e.to_string()))?;
    Ok(args)
}

impl Command for ImagesetBuild {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "imageset.build",
            EffectKind::Mutate,
            "Pack a folder of PNGs into one DayZ texture and write the .imageset naming each image.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the folder whose PNGs become the set's images; its subfolders are not read" },
                "out": { "type": "string", "description": "the .imageset file to write; the textures are written beside it" },
                "game_dir": { "type": "string", "description": "the in-game folder the textures are packed at, e.g. \"MyMod/gui/imagesets\"" },
            },
            "required": ["path", "out", "game_dir"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args = imageset_args(args)?;
        let change_set = imageset_ops::plan_build_imageset(&args).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args = imageset_args(args)?;
        let (summary, change_set) = imageset_ops::build_imageset(&args).map_err(failed)?;
        applied(change_set.len(), summary, &change_set)
    }
}

struct FontBuild;

/// The `font.build` arguments, with a bad value refused before the font is read.
fn font_args(args: Value) -> Result<FontArgs, CommandError> {
    let args: FontArgs = parse_args(args)?;
    font_ops::check_args(&args).map_err(|e| CommandError::InvalidArguments(e.to_string()))?;
    Ok(args)
}

impl Command for FontBuild {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "font.build",
            EffectKind::Mutate,
            "Turn a TTF or OTF font into DayZ .fnt and .edds files at the given pixel sizes, with a size list a layout can name, or into one distance-field font that draws sharp at any size.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the TTF or OTF font to render" },
                "sizes": { "type": "array", "items": { "type": "integer" }, "description": "pixel sizes to write, e.g. [10, 12, 14]; required unless msdf" },
                "out": { "type": "string", "description": "the folder the .fnt, .edds and .xml files are written into" },
                "name": { "type": "string", "description": "the file name stem (default: the font file's stem, lowercased, other characters turned into _)" },
                "chars": { "type": "string", "enum": ["default", "all"], "description": "the character set (default: default)" },
                "sizing": { "type": "string", "enum": ["line", "em"], "description": "what a size measures: line, the cell height as in vanilla's fonts, so text drawn at that size fills it (default), or em, as DayZ Tools' FontToTga; refused with msdf" },
                "msdf": { "type": "boolean", "description": "one distance-field font that draws sharp at any size; no sizes" },
            },
            "required": ["path", "out"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args = font_args(args)?;
        let change_set = font_ops::plan_build_fonts(&args).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args = font_args(args)?;
        let (summary, change_set) = font_ops::build_fonts(&args).map_err(failed)?;
        applied(change_set.len(), summary, &change_set)
    }
}

struct PboUnpack;

/// The `pbo.unpack` arguments, with a bad value refused before any file is read.
fn pbo_args(args: Value) -> Result<PboUnpackArgs, CommandError> {
    let args: PboUnpackArgs = parse_args(args)?;
    pbo_ops::check_args(&args).map_err(|e| CommandError::InvalidArguments(e.to_string()))?;
    Ok(args)
}

impl Command for PboUnpack {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "pbo.unpack",
            EffectKind::Mutate,
            "Unpack a PBO's files into a folder as they are stored in it, with its prefix in a `$PBOPREFIX$` file, to read them as references. A mod folder unpacks every PBO it holds, one folder per PBO. Refuses to overwrite.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the PBO file to unpack, or a mod folder: every PBO in its addons folder, or else directly in it, is unpacked into its own folder under out" },
                "out": { "type": "string", "description": "the folder to unpack into; created if missing, never overwritten" },
            },
            "required": ["path", "out"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args = pbo_args(args)?;
        let change_set = pbo_ops::plan_unpack_pbo(&args).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args = pbo_args(args)?;
        let (summary, change_set) = pbo_ops::unpack_pbo(&args).map_err(failed)?;
        applied(change_set.len(), summary, &change_set)
    }
}

/// Arguments for `sign.check`: the built mod and an optional server install whose keys the
/// signatures are checked against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignCheckArgs {
    pub mod_dir: PathBuf,
    #[serde(default)]
    pub server_dir: Option<PathBuf>,
}

struct SignCheck;

impl Command for SignCheck {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "sign.check",
            EffectKind::Read,
            "Check a build's signatures, and with `server_dir` the server keys that verify them.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mod_dir": { "type": "string", "description": "the built @Mod folder to check" },
                "server_dir": { "type": "string", "description": "a DayZServer install whose keys/ the build is checked against" },
            },
            "required": ["mod_dir"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let SignCheckArgs {
            mod_dir,
            server_dir,
        } = parse_args(args)?;
        if !mod_dir.is_dir() {
            return Err(CommandError::InvalidArguments(format!(
                "{} is not a folder",
                mod_dir.display()
            )));
        }
        if let Some(server_dir) = server_dir.as_ref().filter(|dir| !dir.is_dir()) {
            return Err(CommandError::InvalidArguments(format!(
                "{} is not a folder",
                server_dir.display()
            )));
        }
        let mut diagnostics: Vec<_> = structure_checks(&build_files(&mod_dir).map_err(failed)?)
            .into_iter()
            .filter(|diagnostic| {
                matches!(diagnostic.code.as_ref(), "MF-P001" | "MF-P002" | "MF-P007")
            })
            .collect();
        diagnostics.extend(signature_checks(&mod_dir).map_err(failed)?);
        if let Some(server_dir) = server_dir {
            diagnostics.extend(server_key_checks(&mod_dir, &server_dir).map_err(failed)?);
        }
        Ok(Outcome::Data(json!({
            "diagnostics": json_of(&diagnostics)?,
        })))
    }
}

struct ProjectScaffold;

impl Command for ProjectScaffold {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "project.scaffold",
            EffectKind::Mutate,
            "Scaffold a new DayZ script mod skeleton into a directory.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "mod name: a config class name (a letter or _ first, then letters, digits and _); also the class prefix" },
                "dir": { "type": "string", "description": "target directory to create the mod in" },
                "display_name": { "type": "string", "description": "launcher name (default: the mod name)" },
                "description": { "type": "string", "description": "launcher description: first line is the tooltip, all of it the overview" },
                "author": { "type": "string", "description": "author name for mod.cpp" },
                "version": { "type": "string", "description": "mod.cpp version (default: 0.1.0)" },
                "prefix": { "type": "string", "description": "PBO prefix for the files[] paths (default: the mod name)" },
                "patch_class": { "type": "string", "description": "the CfgPatches class name (default: <name>_Scripts)" },
                "required_addons": { "type": "array", "items": { "type": "string" }, "description": "CfgPatches names listed in requiredAddons after DZ_Data" },
                "modules": { "type": "array", "items": { "type": "string" }, "description": "script module dirs to bind (default: 3_Game, 4_World, 5_Mission)" },
                "starter": { "type": "boolean", "description": "write the 5_Mission starter script (default: true)" },
            },
            "required": ["name", "dir"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: ScaffoldArgs = parse_args(args)?;
        let change_set = plan_scaffold(&args.dir, &args.options()).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: ScaffoldArgs = parse_args(args)?;
        let (files_changed, change_set) = scaffold(&args.dir, &args.options()).map_err(failed)?;
        applied(
            files_changed,
            format!("scaffolded mod `{}` in {}", args.name, args.dir.display()),
            &change_set,
        )
    }
}

#[derive(Deserialize)]
struct DependenciesArgs {
    root: PathBuf,
}

struct ProjectDependencies {
    db: Option<PathBuf>,
}

impl Command for ProjectDependencies {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "project.dependencies",
            EffectKind::Read,
            "List the dependency mods saved for a mod project (the ones `mf lint` and the game check use).",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "mod project root whose saved dependencies are listed" },
            },
            "required": ["root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: DependenciesArgs = parse_args(args)?;
        let dependencies = match &self.db {
            Some(db) => project_deps::list(db, &args.root, settings_fill::discovered_steam_root)
                .map_err(failed)?,
            None => project_deps::list_shared(&args.root, settings_fill::discovered_steam_root)
                .map_err(failed)?,
        };
        Ok(Outcome::Data(json!({ "dependencies": dependencies })))
    }
}

#[derive(Deserialize)]
struct EditDependencyArgs {
    root: PathBuf,
    action: EditAction,
    dependency: String,
    #[serde(default, rename = "where")]
    target: EditTarget,
}

struct ProjectEditDependencies {
    db: Option<PathBuf>,
}

fn edit_dependency(
    db: Option<&Path>,
    args: &EditDependencyArgs,
    steam_root: Option<&Path>,
    apply: bool,
) -> Result<EditOutcome, CommandError> {
    let dependency = project_deps::parse_dependency(&args.dependency);
    project_deps::edit(
        db,
        &args.root,
        args.action,
        args.target,
        &dependency,
        steam_root,
        apply,
    )
    .map_err(failed)
}

impl Command for ProjectEditDependencies {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "project.edit_dependencies",
            EffectKind::Mutate,
            "Add or remove a dependency mod of a mod project: a mod folder or a Workshop id. Saved in the mod's .modfoundry/dependencies.json unless where is machine. Not on the undo stack; the opposite action undoes it.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "root": { "type": "string", "description": "mod project root whose saved dependencies change" },
                "action": { "type": "string", "enum": ["add", "remove"], "description": "add or remove the dependency" },
                "dependency": { "type": "string", "description": "a mod folder, or a Workshop id (all digits); remove also takes workshop:<id> or the name project.dependencies lists" },
                "where": { "type": "string", "enum": ["project", "machine"], "description": "project: the mod's .modfoundry/dependencies.json (default); machine: this machine's saved list" },
                "steam_dir": { "type": "string", "description": "Steam library root (default: platform discovery)" },
            },
            "required": ["root", "action", "dependency"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let steam_root = settings_fill::steam_root(&args);
        let args: EditDependencyArgs = parse_args(args)?;
        let outcome = edit_dependency(self.db.as_deref(), &args, steam_root.as_deref(), false)?;
        Ok(Some(json_of(&outcome)?))
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let steam_root = settings_fill::steam_root(&args);
        let args: EditDependencyArgs = parse_args(args)?;
        let outcome = edit_dependency(self.db.as_deref(), &args, steam_root.as_deref(), true)?;
        Ok(Outcome::changed(ChangeSummary {
            kind: EffectKind::Mutate,
            files_changed: usize::from(args.target.writes_file() && outcome.note.is_none()),
            description: project_deps::describe_edit(
                &outcome,
                args.action,
                args.target,
                &args.dependency,
                &args.root,
            ),
        }))
    }
}

/// The build arguments of the publish commands, which also take `content_only`.
fn publish_build_properties() -> Value {
    let mut properties = build_properties();
    properties["content_only"] = json!({ "type": "boolean", "description": "send only the content, change note and key-value tags to an existing item" });
    properties
}

fn build_properties() -> Value {
    json!({
        "mod_root": { "type": "string", "description": "mod root to pack; a left-out setting takes the app's saved value" },
        "name": { "type": "string", "description": "override the mod name (default: target dir name)" },
        "output": { "type": "string", "description": "output dir override" },
        "profile": { "type": "string", "description": "build profile (default: default)" },
        "authority": { "type": "string", "description": "signing authority" },
        "keys_dir": { "type": "string", "description": "dir holding the keypair (default: <mod_root>/keys)" },
        "prefix": { "type": "string", "description": "PBO header prefix override" },
        "tools": { "type": "string", "description": "DsUtils dir: sign with the official DSSignFile" },
        "binarize_config": { "type": "boolean", "description": "pack config.bin instead of the root config.cpp" },
        "strip_comments": { "type": "boolean", "description": "remove comments from packed .c files; line numbers stay" },
    })
}

struct BuildRun {
    db: Option<PathBuf>,
}

/// One mod of a set whose arguments saved settings have filled.
struct PendingBuild {
    name: String,
    args: BuildArgs,
    settings_used: SettingsUsed,
}

/// The same failure, saying which mod of the set it belongs to.
fn for_mod(name: &str, error: CommandError) -> CommandError {
    match error {
        CommandError::InvalidArguments(message) => {
            CommandError::InvalidArguments(format!("mod {name}: {message}"))
        }
        CommandError::Failed(message) => CommandError::Failed(format!("mod {name}: {message}")),
        other => other,
    }
}

impl BuildRun {
    /// Every local mod of a set, in order, with its arguments filled from saved settings.
    fn set_mods(&self, request: SetRequest) -> Result<Vec<PendingBuild>, CommandError> {
        let mut pending = Vec::new();
        for build in request.builds().map_err(CommandError::InvalidArguments)? {
            let SetBuild { name, args } = build;
            let (args, mut settings_used) =
                settings_fill::build(self.db.as_deref(), args).map_err(|e| for_mod(&name, e))?;
            settings_used.resolved_root = request.resolved_root.clone();
            let args: BuildArgs = parse_args(args).map_err(|e| for_mod(&name, e))?;
            pending.push(PendingBuild {
                name,
                args,
                settings_used,
            });
        }
        Ok(pending)
    }

    /// A build's answer: its report, the settings filled for it and its unsigned warning.
    fn report_value(
        &self,
        report: &BuildReport,
        args: &BuildArgs,
        settings_used: SettingsUsed,
    ) -> Result<Value, CommandError> {
        let mut value = json_of(report)?;
        settings_used.write_into(&mut value);
        let warning =
            settings_fill::unsigned_warning(self.db.as_deref(), &json_of(args)?, report.signed);
        if let Some(warning) = warning {
            value["warning"] = json!(warning);
        }
        Ok(value)
    }
}

impl Command for BuildRun {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "build.run",
            EffectKind::Mutate,
            "Deterministically pack (+ optionally sign) a mod, or every local mod of a launch set, into builds/<profile>/@Mod.",
        )
    }

    fn args_schema(&self) -> Value {
        let mut properties = build_properties();
        properties["set"] = json!({ "type": "string", "description": "a launch set in the project's server/launch-sets.json: its local mods are built in order, stopping at the first failure" });
        properties["source_root"] = json!({ "type": "string", "description": "project root holding server/launch-sets.json (default: the current folder)" });
        preview_schema(json!({
            "type": "object",
            "properties": properties,
            "description": "give mod_root, or set (with source_root), not both",
        }))
    }

    /// What the build would stage. Build output is regenerable, so a build is
    /// never recorded for `workspace.undo` — the plan is a preview only.
    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        match BuildTarget::parse(args).map_err(CommandError::InvalidArguments)? {
            BuildTarget::One(args) => {
                let (args, settings_used) = settings_fill::build(self.db.as_deref(), args)?;
                let args: BuildArgs = parse_args(args)?;
                let change_set = plan_build_project(&args).map_err(failed)?;
                Ok(Some(planned_using(&change_set, &settings_used)?))
            }
            BuildTarget::Set(request) => {
                let mut plans = Vec::new();
                for pending in self.set_mods(request)? {
                    let plan = plan_build_project(&pending.args)
                        .map_err(|e| for_mod(&pending.name, failed(e)))?;
                    let mut entry = json!({ "name": pending.name, "plan": json_of(&plan)? });
                    pending.settings_used.write_into(&mut entry);
                    plans.push(entry);
                }
                Ok(Some(Value::Array(plans)))
            }
        }
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        match BuildTarget::parse(args).map_err(CommandError::InvalidArguments)? {
            BuildTarget::One(args) => {
                let (args, settings_used) = settings_fill::build(self.db.as_deref(), args)?;
                let args: BuildArgs = parse_args(args)?;
                let report = build_project(&args).map_err(failed)?;
                Ok(Outcome::Data(self.report_value(
                    &report,
                    &args,
                    settings_used,
                )?))
            }
            BuildTarget::Set(request) => {
                let mut reports = Vec::new();
                for pending in self.set_mods(request)? {
                    let report = build_project(&pending.args)
                        .map_err(|e| for_mod(&pending.name, failed(e)))?;
                    reports.push(self.report_value(
                        &report,
                        &pending.args,
                        pending.settings_used,
                    )?);
                }
                Ok(Outcome::Data(Value::Array(reports)))
            }
        }
    }
}

struct SignKeygen;

impl Command for SignKeygen {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "sign.keygen",
            EffectKind::Mutate,
            "Generate a <authority>.bikey + .biprivatekey signing keypair.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "authority": { "type": "string", "description": "authority name (becomes the key file stem)" },
                "dir": { "type": "string", "description": "where to write the keys" },
                "tools": { "type": "string", "description": "DsUtils dir: use the official DSCreateKey" },
            },
            "required": ["authority", "dir"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let KeygenArgs { authority, dir, .. } = parse_args(args)?;
        let change_set = plan_keygen(&authority, &dir).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let KeygenArgs {
            authority,
            dir,
            tools,
        } = parse_args(args)?;
        let (private_key, public_key, change_set) =
            keygen(&authority, &dir, tools).map_err(failed)?;
        let summary = ChangeSummary {
            kind: EffectKind::Mutate,
            files_changed: change_set.as_ref().map_or(0, ChangeSet::len),
            description: format!(
                "created private key {} and public key {}",
                private_key.display(),
                public_key.display()
            ),
        };
        match change_set {
            Some(change_set) => Ok(Outcome::applied(summary, json_of(&change_set)?)),
            // The official DSCreateKey wrote the files; an external process is
            // not something the VFS can record.
            None => Ok(Outcome::changed(summary)),
        }
    }
}

struct WorkspaceUndo;

impl Command for WorkspaceUndo {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "workspace.undo",
            EffectKind::Mutate,
            "Undo the last recorded change-set (`mf undo`).",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "change_set": {
                    "type": "object",
                    "description": "the recorded change-set to invert; the host supplies it from the undo log",
                },
            },
            "required": ["change_set"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let UndoArgs { change_set } = parse_args(args)?;
        let restored = change_set
            .undo()
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        // The undo offers no further undo of its own: the stack only ever walks
        // backwards, so repeated `mf undo` replays earlier operations.
        Ok(Outcome::changed(ChangeSummary {
            kind: EffectKind::Mutate,
            files_changed: restored,
            description: format!(
                "undid {} change(s) under {}",
                change_set.len(),
                change_set.root
            ),
        }))
    }
}

struct TextEdit;

impl Command for TextEdit {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "text.edit",
            EffectKind::Mutate,
            "Replace a text file's contents (preview-first: `preview` returns the change-set).",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "the text file the edit targets" },
                "text": { "type": "string", "description": "the file's complete replacement content" },
                "out": { "type": "string", "description": "write the result here instead of `path` (a save-as)" },
                "expected_sha256": { "type": "string", "description": "hex SHA-256 of `path`'s current content (the revision the caller read); the edit refuses to land if the file has moved since, and it cannot be combined with `out`" },
            },
            "required": ["path", "text"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: TextEditArgs = parse_args(args)?;
        let change_set =
            plan_edit_text(args.target()?, &args.text, args.expected_sha256.as_deref())
                .map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: TextEditArgs = parse_args(args)?;
        let (target, bytes, change_set) =
            edit_text_to(args.target()?, &args.text, args.expected_sha256.as_deref())
                .map_err(failed)?;
        applied(
            1,
            format!("wrote {bytes} bytes to {}", target.display()),
            &change_set,
        )
    }
}

struct DiagRun {
    control: RunControl,
    db: Option<PathBuf>,
    settings: Option<PathBuf>,
    actor: String,
}

impl DiagRun {
    /// The server arguments a call launches with: saved settings filled, and a detached run's
    /// length applied when it may not wait.
    fn prepared(
        &self,
        args: Value,
        wait: bool,
    ) -> Result<(RunServerArgs, Option<u64>, SettingsUsed), CommandError> {
        let (args, set_config) = settings_fill::launch_set(args, settings_fill::SetUse::Server)?;
        let (args, settings_used) = if wait {
            settings_fill::server(self.settings.as_deref(), args)?
        } else {
            settings_fill::detached_server(self.settings.as_deref(), args)?
        };
        let (mut args, limit_secs) = diag_run_args(wait, args)?;
        args.set_config = set_config;
        check_verdict_patterns(&args.expect_lines, &args.fail_lines, &args.allow_errors)?;
        check_known_errors(args.source_root.as_deref())?;
        Ok((args, limit_secs, settings_used))
    }
}

impl Command for DiagRun {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.run",
            EffectKind::Launch,
            "Launch the native DayZServer with a built @Mod loaded, capture its logs, map its errors.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "mod_dir": { "type": "string", "description": "built @Mod dir to load via -mod (default with set: the build of the set's local mod at source_root)" },
                "server_dir": { "type": "string", "description": "DayZServer install dir (default: platform discovery)" },
                "config": { "type": "string", "description": "-config= server config file (default: the project's server/serverDZ.cfg, matched in any letter case, when source_root is given, else the install's)" },
                "port": { "type": "integer", "description": "-port= listen port" },
                "profiles_dir": { "type": "string", "description": "capture dir (default: <source_root>/server/profiles, kept between runs; a fresh temp dir without source_root)" },
                "timeout_secs": { "type": "integer", "description": "bounded launch window; a run left going with wait: false has none unless given or saved" },
                "source_root": { "type": "string", "description": "loose mod source root for file-mapped errors; a left-out launch setting takes the app's saved value" },
                "test_mods": test_mods_schema(),
                "set": { "type": "string", "description": "a named launch set in server/launch-sets.json whose mods and config this run takes" },
                "expect_lines": verdict_list_schema("lines the run's logs must show for its verdict to pass; `re:` makes one a regular expression"),
                "fail_lines": verdict_list_schema("lines that fail the run's verdict when they appear; `re:` makes one a regular expression"),
                "allow_errors": verdict_list_schema("known script errors the run's verdict ignores; `re:` makes one a regular expression"),
                "run_id": { "type": "string", "description": "id under which diag.stop and diag.status find this run; a launch is refused while a run with this id is still going" },
                "wait": { "type": "boolean", "description": "false returns once the server reports ready and leaves the run going until diag.stop, or until timeout_secs from launch when one applies; the answer's stops_after_secs names it, and ready is false if the wait gave up first (default true)" },
            },
            "required": [],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        // Plan with the fill the run would use; a preview needs no database to say so.
        let wait = args.get("wait") != Some(&Value::Bool(false));
        let (args, _, settings_used) = self.prepared(args, wait)?;
        let server_dir = resolve_server_dir(args.server_dir.as_deref()).map_err(failed)?;
        let plan = plan_server(&args, &dead_run_leftovers(&server_dir)).map_err(failed)?;
        Ok(Some(planned_using(&plan, &settings_used)?))
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let wait = wait_for_run(&args, self.db.as_deref())?;
        let (mut args, limit_secs, settings_used) = self.prepared(args, wait)?;
        check_detached_verdict(
            wait,
            &args.expect_lines,
            &args.fail_lines,
            &args.allow_errors,
        )?;
        if args.profiles_dir.is_none() && args.source_root.is_none() {
            args.profiles_dir = Some(scratch_profiles_dir());
        }
        let launch = Launch {
            db: self.db.clone(),
            actor: self.actor.clone(),
            command: "diag.run",
            run_id: args.run_id.clone(),
            source_root: args.source_root.clone(),
            log_dirs: server_log_dirs(&args),
        };
        if !wait {
            return detached_data(
                &self.control,
                launch,
                move |signal| run_server(&args, signal),
                settings_used,
                limit_secs,
            );
        }
        let (result, run_id) =
            run_launch(&self.control, launch, |signal| run_server(&args, signal));
        launch_data(&result.map_err(failed)?, run_id.as_deref(), &settings_used)
    }
}

struct DiagRunClient {
    control: RunControl,
    db: Option<PathBuf>,
    settings: Option<PathBuf>,
    actor: String,
}

impl DiagRunClient {
    fn prepared(
        &self,
        args: Value,
    ) -> Result<(RunClientArgs, Option<String>, SettingsUsed), CommandError> {
        let (args, _) = settings_fill::launch_set(args, settings_fill::SetUse::Client)?;
        let (args, settings_used) = settings_fill::client(self.settings.as_deref(), args)?;
        let RunClientCommandArgs {
            mut client,
            connect,
            run_id,
        } = parse_args(args)?;
        client.connect = match connect {
            Some(s) => Some(modfoundry_diag::Connect::parse(&s).ok_or_else(|| {
                CommandError::InvalidArguments(format!(
                    "--connect must be host:gameport[:queryport], got `{s}`"
                ))
            })?),
            None => None,
        };
        check_verdict_patterns(
            &client.expect_lines,
            &client.fail_lines,
            &client.allow_errors,
        )?;
        check_known_errors(client.source_root.as_deref())?;
        Ok((client, run_id, settings_used))
    }
}

impl Command for DiagRunClient {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.run_client",
            EffectKind::Launch,
            "Launch the DayZ client under Proton with local mods staged, capture its logs, map its errors.",
        )
    }

    fn args_schema(&self) -> Value {
        let mut test_mods = test_mods_schema();
        test_mods["description"] = json!("extra mods loaded only for this run, in order; the client loads them before mods, including the mods a set fills");
        preview_schema(json!({
            "type": "object",
            "properties": {
                "mods": { "type": "array", "items": { "type": "string" }, "description": "built @Mod dirs to stage and load, in order; required unless set is given. The last one is the run's own mod: the verdict ties script errors to it and to test_mods, not to the other mods" },
                "game_dir": { "type": "string", "description": "DayZ client install dir (default: platform discovery)" },
                "steam_dir": { "type": "string", "description": "Steam library root (default: platform discovery)" },
                "launcher": { "type": "string", "description": "launcher executable override (default: direct Proton launch)" },
                "connect": { "type": "string", "description": "-connect=host:gameport[:queryport] target" },
                "profile_name": { "type": "string", "description": "-name= game profile name" },
                "extra": { "type": "array", "items": { "type": "string" }, "description": "extra game args" },
                "window": { "type": "object", "description": "windowed client size {width,height} (default 1280x720)" },
                "windowed": { "type": "boolean", "description": "open the client in a window; false runs it full screen" },
                "timeout_secs": { "type": "integer", "description": "bounded capture window" },
                "settle_secs": { "type": "integer", "description": "quiet period judged as a complete capture" },
                "source_root": { "type": "string", "description": "loose mod source root for file-mapped errors; a left-out launch setting takes the app's saved value" },
                "test_mods": test_mods,
                "set": { "type": "string", "description": "a named launch set in server/launch-sets.json whose shared mods this run loads; needs source_root, and fills mods when mods is left out" },
                "expect_lines": verdict_list_schema("lines the run's logs must show for its verdict to pass; `re:` makes one a regular expression"),
                "fail_lines": verdict_list_schema("lines that fail the run's verdict when they appear; `re:` makes one a regular expression"),
                "allow_errors": verdict_list_schema("known script errors the run's verdict ignores; `re:` makes one a regular expression"),
                "run_id": { "type": "string", "description": "id under which diag.stop and diag.status find this run; a launch is refused while a run with this id is still going" },
            },
            "description": "give mods, or set (with source_root)",
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let (client, _, settings_used) = self.prepared(args)?;
        let game_dir = resolve_game_dir(client.game_dir.as_deref()).map_err(failed)?;
        let plan = plan_client(&client, &dead_run_leftovers(&game_dir)).map_err(failed)?;
        Ok(Some(planned_using(&plan, &settings_used)?))
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (client, run_id, settings_used) = self.prepared(args)?;
        let launch = Launch {
            db: self.db.clone(),
            actor: self.actor.clone(),
            command: "diag.run_client",
            run_id,
            source_root: client.source_root.clone(),
            log_dirs: client_log_dirs(client.steam_dir.as_deref()),
        };
        let (result, recorded_id) =
            run_launch(&self.control, launch, |signal| run_client(&client, signal));
        launch_data(
            &result.map_err(failed)?,
            recorded_id.as_deref(),
            &settings_used,
        )
    }
}

struct DiagRunDiag {
    control: RunControl,
    db: Option<PathBuf>,
    settings: Option<PathBuf>,
    actor: String,
}

impl DiagRunDiag {
    fn prepared(&self, args: Value) -> Result<(RunDiagArgs, SettingsUsed), CommandError> {
        let (args, _) = settings_fill::launch_set(args, settings_fill::SetUse::Diag)?;
        let (args, settings_used) = settings_fill::diag(self.settings.as_deref(), args)?;
        let args: RunDiagArgs = parse_args(args)?;
        check_verdict_patterns(&args.expect_lines, &args.fail_lines, &args.allow_errors)?;
        check_known_errors(Some(&args.source_root))?;
        Ok((args, settings_used))
    }
}

impl Command for DiagRunDiag {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.run_diag",
            EffectKind::Launch,
            "Launch the Diag server (+ Diag client) from the client install, capture both logs, map their errors.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "mod_dir": { "type": "string", "description": "built @Mod dir to load via -mod (default with set: the build of the set's local mod at source_root)" },
                "source_root": { "type": "string", "description": "loose mod source root staged as the PBO prefix; a left-out launch setting takes the app's saved value" },
                "mod_prefix": { "type": "string", "description": "PBO prefix the loose source is staged as (default: the prefix of the one PBO in the built mod's addons, refused when the source's files[] does not match it; else the source folder name without a leading @)" },
                "game_dir": { "type": "string", "description": "DayZ client install dir (default: platform discovery)" },
                "server_dir": { "type": "string", "description": "DayZServer install dir the mpmissions/config come from" },
                "steam_dir": { "type": "string", "description": "Steam library root (default: platform discovery)" },
                "profiles_dir": { "type": "string", "description": "real profiles dir the relative link points at (default: the project's server/profiles, kept between runs)" },
                "port": { "type": "integer", "description": "Diag server port (default 2302)" },
                "client": { "type": "boolean", "description": "also start the Diag client that joins the server (default true)" },
                "connect_name": { "type": "string", "description": "-name= game profile name for the client" },
                "window": { "type": "object", "description": "windowed client size {width,height} (default 1280x720)" },
                "windowed": { "type": "boolean", "description": "open the client in a window (default true); false runs it full screen" },
                "config": { "type": "string", "description": "server config template under the project's server/ folder, matched in any letter case (default serverDZ.cfg)" },
                "set": { "type": "string", "description": "a named launch set in server/launch-sets.json whose mods and config this run takes" },
                "test_mods": test_mods_schema(),
                "expect_lines": verdict_list_schema("lines the run's logs must show for its verdict to pass; `re:` makes one a regular expression"),
                "fail_lines": verdict_list_schema("lines that fail the run's verdict when they appear; `re:` makes one a regular expression"),
                "allow_errors": verdict_list_schema("known script errors the run's verdict ignores; `re:` makes one a regular expression"),
                "timeout_secs": { "type": "integer", "description": "optional; omit to keep running until stopped" },
                "run_id": { "type": "string", "description": "id under which diag.stop and diag.status find this run; a launch is refused while a run with this id is still going" },
                "wait": { "type": "boolean", "description": "false returns once the server reports ready and leaves the run going until diag.stop, or until timeout_secs from launch when one applies; the answer's stops_after_secs names it, and ready is false if the wait gave up first (default true)" },
            },
            "required": ["source_root"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let (args, settings_used) = self.prepared(args)?;
        let plan = plan_diag(&args).map_err(failed)?;
        Ok(Some(planned_using(&plan, &settings_used)?))
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let wait = wait_for_run(&args, self.db.as_deref())?;
        let (args, settings_used) = self.prepared(args)?;
        check_detached_verdict(
            wait,
            &args.expect_lines,
            &args.fail_lines,
            &args.allow_errors,
        )?;
        let limit_secs = args.timeout_secs;
        let launch = Launch {
            db: self.db.clone(),
            actor: self.actor.clone(),
            command: "diag.run_diag",
            run_id: args.run_id.clone(),
            source_root: Some(args.source_root.clone()),
            log_dirs: diag_log_dirs(&args),
        };
        if !wait {
            return detached_data(
                &self.control,
                launch,
                move |signal| run_diag(&args, signal),
                settings_used,
                limit_secs,
            );
        }
        let (result, run_id) = run_launch(&self.control, launch, |signal| run_diag(&args, signal));
        launch_data(&result.map_err(failed)?, run_id.as_deref(), &settings_used)
    }
}

struct DiagStop {
    control: RunControl,
    db: Option<PathBuf>,
}

impl DiagStop {
    /// Asks the launcher of a live recorded run to stop it; best effort, like the recording.
    fn request_recorded_stop(&self, run_id: &str) -> bool {
        // A stop lookup must not create a database on a machine where no run was ever recorded.
        let Some(db) = self.db.as_deref().filter(|db| db.is_file()) else {
            return false;
        };
        let Ok(store) = Store::open(db) else {
            return false;
        };
        let live = store
            .live_runs(run_record::now_unix(), run_record::STALE_AFTER_SECS)
            .is_ok_and(|runs| runs.iter().any(|run| run.id == run_id));
        live && store.request_run_stop(run_id).unwrap_or(false)
    }
}

impl Command for DiagStop {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.stop",
            EffectKind::Launch,
            "Stop a live diag.run / diag.run_client / diag.run_diag launch by its run_id.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string", "description": "id passed to a diag.run* launch" },
            },
            "required": ["run_id"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let StopArgs { run_id } = parse_args(args)?;
        let stopped = self.control.stop(&run_id) || self.request_recorded_stop(&run_id);
        Ok(Outcome::Data(json!({ "stopped": stopped })))
    }
}

struct DiagStatus {
    db: Option<PathBuf>,
}

impl Command for DiagStatus {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.status",
            EffectKind::Read,
            "Say whether a recorded diag.run / diag.run_client / diag.run_diag launch is running, ended or lost, \
             and give its outcome and, once ended, its result.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string", "description": "id of a recorded run: the run_id a diag.run* answer names" },
            },
            "required": ["run_id"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let StopArgs { run_id } = parse_args(args)?;
        let missing = || CommandError::Failed(format!("no run `{run_id}` is recorded"));
        let db = self
            .db
            .as_deref()
            .filter(|db| db.is_file())
            .ok_or_else(missing)?;
        let store = Store::open(db).map_err(|_| missing())?;
        let row = store.run(&run_id).ok().flatten().ok_or_else(missing)?;
        let result = store.run_result(&run_id).ok().flatten();
        Ok(Outcome::Data(run_status_value(
            &row,
            result,
            run_record::now_unix(),
        )))
    }
}

fn run_status_value(row: &RunRecord, result: Option<Value>, now_unix: i64) -> Value {
    let state = match row.state {
        RunState::Ended => "ended",
        RunState::Running if row.seen_unix >= now_unix - run_record::STALE_AFTER_SECS => "running",
        RunState::Running => "lost",
    };
    let end = if state == "running" {
        now_unix
    } else {
        row.seen_unix
    };
    let mut value = json!({
        "run_id": row.id,
        "state": state,
        "command": row.command,
        "actor": row.actor,
        "started_millis": row.started_millis,
        "elapsed_secs": (end - row.started_unix).max(0),
        "source_root": row.source_root.as_ref().map(|p| p.display().to_string()),
        "log_dirs": row.log_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "stop_requested": row.stop_requested,
        "outcome": row.outcome,
    });
    if state == "ended" {
        if let Some(result) = result {
            value["result"] = result;
        }
    }
    value
}

struct DiagErrors;

impl Command for DiagErrors {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.errors",
            EffectKind::Read,
            "Parse RPT/script.log files under a dir and map the errors back to source files, marking the ones a baseline run also logged and the ones the project's known-error file explains.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "dir": { "type": "string", "description": "profiles or log dir to parse" },
                "source_root": { "type": "string", "description": "mod source root used to resolve logged paths" },
                "baseline": { "type": "string", "description": "a log file or a folder from a run without this project's mods; errors that also occur there are marked baseline" },
                "known_errors": { "type": "string", "description": "the project's known-error file (default `server/known-errors.json` under `source_root`)" },
            },
            "required": ["dir"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ErrorArgs {
            dir,
            source_root,
            baseline,
            known_errors,
        } = parse_args(args)?;
        let mut errors = errors_from_dir(&dir, source_root.as_deref()).map_err(failed)?;
        if let Some(baseline) = baseline {
            let logs = baseline_logs(&baseline).map_err(failed)?;
            let theirs = errors_from_logs(&logs, &LogStates::new(), None).map_err(failed)?;
            mark_baseline(&mut errors, &theirs);
        }
        KnownErrors::load(known_errors.as_deref(), source_root.as_deref())
            .map_err(failed)?
            .mark(&mut errors);
        json_of(&errors).map(Outcome::Data)
    }
}

/// The logs a `baseline` path names: the file itself, or every log a folder holds.
fn baseline_logs(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if path.is_dir() {
        return Ok(modfoundry_diag::find_logs(path));
    }
    anyhow::bail!("{}: no such log file or folder", path.display())
}

struct DiagLearnError;

impl Command for DiagLearnError {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "diag.learn_error",
            EffectKind::Mutate,
            "Append one of a log folder's errors to the project's known-error file, by id (preview-first: `preview` returns the change-set).",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "dir": { "type": "string", "description": "profiles or log dir the error is read from" },
                "source_root": { "type": "string", "description": "the project root; also names the default known-error file" },
                "known_errors": { "type": "string", "description": "the known-error file to append to (default `server/known-errors.json` under `source_root`)" },
                "id": { "type": "string", "description": "the id of the error to learn, as `diag.errors` reported it" },
                "meaning": { "type": "string", "description": "what the error means, printed with it from now on" },
                "hint": { "type": "string", "description": "how to fix it, printed once per run" },
            },
            "required": ["dir", "id"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: LearnErrorArgs = parse_args(args)?;
        let (path, text, expected) = plan_learn(&args).map_err(failed)?;
        let change_set = plan_edit_text(&path, &text, expected.as_deref()).map_err(failed)?;
        planned(&change_set)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: LearnErrorArgs = parse_args(args)?;
        let id = args.id.clone();
        let (path, text, expected) = plan_learn(&args).map_err(failed)?;
        let (written, _bytes, change_set) =
            edit_text_to(&path, &text, expected.as_deref()).map_err(failed)?;
        applied(
            1,
            format!("learned {id} into {}", written.display()),
            &change_set,
        )
    }
}

struct HostDoctor;

impl Command for HostDoctor {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "host.doctor",
            EffectKind::Read,
            "Detect DayZ game / DayZ Tools / Steam workshop installations on this host.",
        )
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        use modfoundry_platform::{
            discover_steam_installations, find_dayz_installs, workshop_content_root, HostPlatform,
            DAYZ_APP_ID,
        };
        let steam_libraries = discover_steam_installations()
            .into_iter()
            .map(|lib| SteamLibraryReport {
                dayz_workshop_root: workshop_content_root(&lib, DAYZ_APP_ID),
                root: lib.root,
            })
            .collect();
        let (dayz_game, dayz_tools) = find_dayz_installs();
        let update = update::saved_notice();
        json_of(&HostReport {
            platform: HostPlatform::detect().name().into(),
            steam_libraries,
            dayz_game,
            dayz_tools,
            gpu: gpu_report(),
            update,
        })
        .map(Outcome::Data)
    }
}

struct HostUpdateCheck;

impl Command for HostUpdateCheck {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "host.update_check",
            EffectKind::Network,
            "Check GitHub for a newer ModFoundry release (`mf update --check`). Only a release install checks.",
        )
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        let current = env!("CARGO_PKG_VERSION");
        if let Err(reason) = update::update_target(update::release_install_dir(), process_env) {
            return Ok(Outcome::Data(
                json!({"current": current, "checked": false, "reason": reason}),
            ));
        }
        let state = update::state_path()
            .ok_or_else(|| CommandError::Failed("no cache folder for the update check".into()))?;
        let source = GitHubReleases {
            timeout: update::CHECK_WAIT,
        };
        let now = unix_now()?.as_secs();
        let answer =
            update::run_check(&source, current, now, &state).map_err(CommandError::Failed)?;
        let mut value = json_of(&answer)?;
        value["checked"] = json!(true);
        Ok(Outcome::Data(value))
    }
}

/// The process environment's value for `key`, lossy when it is not UTF-8.
fn process_env(key: &str) -> Option<String> {
    std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())
}

struct HostUpdate;

impl Command for HostUpdate {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            HOST_UPDATE,
            EffectKind::Destructive,
            "Install the newest ModFoundry release over this one (`mf update`). Only for a release install; MCP clients need a restart.",
        )
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        let current = env!("CARGO_PKG_VERSION");
        let lib = update::update_target(update::release_install_dir(), process_env)
            .map_err(|reason| CommandError::Failed(reason.into()))?;
        let work = update::work_dir()
            .ok_or_else(|| CommandError::Failed("no cache folder for the update".into()))?;
        let source = GitHubReleases {
            timeout: update::DOWNLOAD_WAIT,
        };
        let installed = update::run_update(
            &source,
            current,
            &lib,
            &work,
            update::agents_skill_path().as_deref(),
        )
        .map_err(CommandError::Failed)?;
        Ok(Outcome::Data(match installed {
            update::Updated::Installed(installed) => {
                json!({"updated": true, "from": installed.from, "to": installed.to})
            }
            update::Updated::NotNewer { latest } => {
                json!({"updated": false, "current": current, "latest": latest})
            }
        }))
    }
}

struct HostChangelog;

impl Command for HostChangelog {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "host.changelog",
            EffectKind::Read,
            "The embedded changelog (added, changed, fixed and removed per release), optionally only the releases newer than a version.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "since": { "type": "string", "description": "a version like 1.8.0; only releases newer than it are answered" },
            },
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        #[derive(Deserialize)]
        struct ChangelogArgs {
            #[serde(default)]
            since: Option<String>,
        }
        let ChangelogArgs { since } = parse_args(args)?;
        let changelog = crate::changelog::changelog(since.as_deref())
            .map_err(CommandError::InvalidArguments)?;
        json_of(&changelog).map(Outcome::Data)
    }
}

/// The doctor answers MCP and GUI calls, so it waits less than a search run does.
const DEVICE_CHECK_LIMIT: Duration = Duration::from_secs(15);

fn gpu_report() -> GpuReport {
    use modfoundry_platform::cuda::{cuda_lib_dir, loader_cache, GPU_HELPER};
    use modfoundry_platform::program::sibling_program;
    let cache = loader_cache();
    gpu_report_with(
        sibling_program(GPU_HELPER),
        cuda_lib_dir(),
        &cache,
        DEVICE_CHECK_LIMIT,
    )
}

/// The GPU report for `helper`, `libraries` and the loader `cache`, asking the helper for a usable device.
pub fn gpu_report_with(
    helper: Option<PathBuf>,
    libraries: Option<PathBuf>,
    cache: &str,
    limit: Duration,
) -> GpuReport {
    use modfoundry_platform::cuda::{in_loader_cache, missing_cuda_libraries, CUDA_DRIVER};
    let helper = helper.filter(|path| path.is_file());
    let (device_ok, device_error) = match &helper {
        Some(program) => {
            match modfoundry_search::helper::check_helper(program, libraries.as_deref(), limit) {
                Ok(()) => (Some(true), None),
                Err(err) => (Some(false), Some(err)),
            }
        }
        None => (None, None),
    };
    GpuReport {
        helper,
        driver: in_loader_cache(cache, CUDA_DRIVER),
        missing: missing_cuda_libraries(libraries.as_deref(), cache)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        libraries,
        device_ok,
        device_error,
    }
}

struct FormatsList;

impl Command for FormatsList {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "formats.list",
            EffectKind::Read,
            "List the file formats ModFoundry recognizes.",
        )
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        json_of(&modfoundry_formats::Format::all()).map(Outcome::Data)
    }
}

struct WorkshopDetails {
    fetch: Box<dyn crate::workshop_api::WorkshopFetch>,
}

impl Command for WorkshopDetails {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "workshop.details",
            EffectKind::Network,
            "Fetch Steam workshop titles, sizes, update times and cached preview icons.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ids": { "type": "array", "items": { "type": "string" }, "description": "published file ids to look up" },
                "cache_dir": { "type": "string", "description": "directory the preview icons are cached in" },
                "download_icons": { "type": "boolean", "description": "fetch and cache preview icons (default true)" },
            },
            "required": ["ids", "cache_dir"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: WorkshopDetailsArgs = parse_args(args)?;
        let details = fetch_details(&args, self.fetch.as_ref())
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        json_of(&details).map(Outcome::Data)
    }
}

struct PublishDetails;

impl Command for PublishDetails {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.details",
            EffectKind::Read,
            "Read a mod's Workshop item details and what is wrong with them.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mod_root": { "type": "string", "description": "the mod's root folder" },
            },
            "required": ["mod_root"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let ModRootArgs { mod_root } = parse_args(args)?;
        let view = details_view(&mod_root).map_err(failed)?;
        json_of(&view).map(Outcome::Data)
    }
}

struct PublishSaveDetails;

impl Command for PublishSaveDetails {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.save_details",
            EffectKind::Mutate,
            "Save a mod's Workshop item details into its workshop/ folder.",
        )
    }

    fn args_schema(&self) -> Value {
        preview_schema(json!({
            "type": "object",
            "properties": {
                "mod_root": { "type": "string", "description": "the mod's root folder" },
                "details": { "type": "object", "description": "every field of the item, replacing the saved ones (read publish.details first): title, visibility (public, friends_only, unlisted or private), tags, version, website, facebook, youtube, twitter, reddit; a missing id keeps the saved id (clear_id removes it)" },
                "description": { "type": "string", "description": "the item's description, replacing the saved one" },
                "preview_from": { "type": "string", "description": "a JPEG, PNG or BMP picture to save as the preview, relative to mod_root or absolute; it is re-encoded as a JPEG" },
                "expected_sha256": { "type": "string", "description": "the revision publish.details answered when the details were read; the save is refused if the saved item file or description changed since" },
                "clear_id": { "type": "boolean", "description": "remove the saved id so the next upload creates a new item (for an item deleted on Steam); cannot be combined with details.id; refused while the root meta.cpp names an id" },
            },
            "required": ["mod_root", "details"],
        }))
    }

    fn previewable(&self) -> bool {
        true
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        let args: SaveDetailsArgs = parse_args(args)?;
        args.check()?;
        let mut txn = WriteTxn::dry_run(root_name("mod", &args.mod_root), &args.mod_root);
        save_details(&args, &mut txn).map_err(failed)?;
        planned(&txn.finish().map_err(|e| failed(e.into()))?)
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: SaveDetailsArgs = parse_args(args)?;
        args.check()?;
        let mut txn = WriteTxn::new(root_name("mod", &args.mod_root), &args.mod_root);
        let files = save_details(&args, &mut txn).map_err(failed)?;
        let change_set = txn.finish().map_err(|e| failed(e.into()))?;
        applied(
            files,
            format!("saved the Workshop details of {}", args.mod_root.display()),
            &change_set,
        )
    }
}

struct PublishPrepare {
    db: Option<PathBuf>,
}

impl Command for PublishPrepare {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.prepare",
            EffectKind::Mutate,
            "Build, check and stage a mod for a Workshop upload; answers the upload plan (build content warnings come back as MF-P019 warnings; a .pbo or .bisign an earlier build left in addons/ is removed and reported as MF-P020; the preview is turned upright by its EXIF orientation and put on white).",
        )
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": publish_build_properties(),
            "required": ["mod_root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (args, settings_used) = settings_fill::build(self.db.as_deref(), args)?;
        let args: PrepareArgs = parse_args(args)?;
        let plan = prepare(&args, unix_now()?).map_err(failed)?;
        data_using_settings(&plan, &settings_used)
    }
}

struct PublishUpload {
    steam: Arc<dyn SteamRun>,
    status: Arc<PublishStatus>,
    db: Option<PathBuf>,
}

impl Command for PublishUpload {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.upload",
            EffectKind::Publish,
            "Build, check and upload a mod to the Steam Workshop through the running Steam client.",
        )
    }

    fn args_schema(&self) -> Value {
        let mut properties = publish_build_properties();
        properties["change_note"] = json!({ "type": "string", "description": "what changed, shown on the item's change notes" });
        json!({
            "type": "object",
            "properties": properties,
            "required": ["mod_root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (args, settings_used) = settings_fill::build(self.db.as_deref(), args)?;
        let args: UploadArgs = parse_args(args)?;
        let result =
            upload(&args, self.steam.as_ref(), &self.status, unix_now()?).map_err(failed)?;
        data_using_settings(&result, &settings_used)
    }

    fn confirm_summary(&self, args: &Value) -> Option<String> {
        let args = UploadArgs::deserialize(args).ok()?;
        let mod_root = args.prepare.build.mod_root.display();
        let item = read_details(&args.prepare.build.mod_root).ok()?.item;
        let content_only = if args.prepare.content_only {
            ", content only"
        } else {
            ""
        };
        Some(match item.id {
            None => match item.visibility {
                Some(visibility) => format!(
                    "Upload \"{}\" as a new {} item{content_only} from {mod_root}",
                    item.title,
                    visibility_word(visibility),
                ),
                None => format!(
                    "Upload \"{}\" as a new item (visibility not set){content_only} from {mod_root}",
                    item.title,
                ),
            },
            Some(id) => {
                let visibility = match item.visibility {
                    Some(visibility) => format!("({})", visibility_word(visibility)),
                    None => "(visibility unchanged)".to_owned(),
                };
                format!(
                    "Update item {id} \"{}\" {visibility}{content_only} from {mod_root}",
                    item.title
                )
            }
        })
    }
}

fn visibility_word(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::FriendsOnly => "friends-only",
        Visibility::Unlisted => "unlisted",
        Visibility::Private => "private",
    }
}

struct PublishItems {
    steam: Arc<dyn SteamRun>,
}

impl Command for PublishItems {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.items",
            EffectKind::Network,
            "List the DayZ Workshop items the logged-in Steam account has published.",
        )
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        let event = self
            .steam
            .run(&SteamRequest::MyItems, &mut |_, _, _| {})
            .map_err(|e| failed(e.into()))?;
        match event {
            SteamEvent::Items { account, items } => {
                Ok(Outcome::Data(json!({ "account": account, "items": items })))
            }
            other => Err(CommandError::Failed(format!(
                "unexpected Steam answer: {other:?}"
            ))),
        }
    }
}

fn unix_now() -> Result<Duration, CommandError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| failed(e.into()))
}

struct PublishStatusCommand {
    status: Arc<PublishStatus>,
}

impl Command for PublishStatusCommand {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.status",
            EffectKind::Read,
            "The running Workshop upload's step and progress, or null.",
        )
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        json_of(&self.status.current()).map(Outcome::Data)
    }
}

struct PublishCancel {
    status: Arc<PublishStatus>,
}

impl Command for PublishCancel {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "publish.cancel",
            EffectKind::Publish,
            "Cancel the running Workshop upload; a new item it created is deleted. Answers whether an upload was running.",
        )
    }

    fn args_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        Ok(Outcome::Data(json!({ "cancelled": self.status.cancel() })))
    }
}

struct BuildContents {
    db: Option<PathBuf>,
}

impl Command for BuildContents {
    fn spec(&self) -> CommandSpec<'static> {
        spec(
            "build.contents",
            EffectKind::Read,
            "List every file under a mod root and whether a build packs it, with warnings (name clashes refuse the build); linked folders are followed, a link back to a parent is skipped; left-out build arguments take the saved settings, as build.run does.",
        )
    }

    fn args_schema(&self) -> Value {
        let properties = build_properties();
        json!({
            "type": "object",
            "properties": {
                "mod_root": properties["mod_root"].clone(),
                "name": properties["name"].clone(),
                "output": properties["output"].clone(),
                "profile": properties["profile"].clone(),
                "binarize_config": properties["binarize_config"].clone(),
            },
            "required": ["mod_root"],
        })
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let (args, settings_used) = settings_fill::build(self.db.as_deref(), args)?;
        let args: BuildArgs = parse_args(args)?;
        let listing = build_contents(&args).map_err(failed)?;
        let mut value = json_of(&listing)?;
        settings_used.write_into(&mut value);
        Ok(Outcome::Data(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_platform::steam::{SteamAccount, SteamError, UploadStage};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-registry-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn publish_items_answers_the_account_and_its_items() {
        struct Listing;
        impl SteamRun for Listing {
            fn run(
                &self,
                request: &SteamRequest,
                _: &mut dyn FnMut(UploadStage, u64, u64),
            ) -> Result<SteamEvent, SteamError> {
                assert_eq!(*request, SteamRequest::MyItems);
                Ok(SteamEvent::Items {
                    account: SteamAccount {
                        name: "sample".into(),
                        steam_id: 7,
                    },
                    items: Vec::new(),
                })
            }
        }
        let command = PublishItems {
            steam: Arc::new(Listing),
        };

        let Outcome::Data(listing) = command.execute(json!({})).unwrap() else {
            panic!("expected Data");
        };
        assert_eq!(listing["account"]["name"], "sample");
        assert_eq!(listing["items"], json!([]));
    }

    #[test]
    fn a_diag_preview_takes_the_mods_of_the_named_set() {
        let root = tmp("set-mod");
        let game = root.join("game");
        let server = root.join("server-install");
        let steam = root.join("steam");
        let source = root.join("mod");
        let own = source.join("builds/default/@mod");
        let folder_a = root.join("mods/@FolderA");
        let folder_b = root.join("mods/@FolderB");
        let proton = steam.join("steamapps/common/Proton - Experimental");
        let runtime = steam.join("steamapps/common/SteamLinuxRuntime_sniper");
        for dir in [
            &game,
            &server.join("mpmissions"),
            &proton,
            &runtime,
            &source.join("server"),
            &own,
            &folder_a,
            &folder_b,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for exe in [
            game.join(crate::diag_run::DIAG_EXE),
            proton.join("proton"),
            runtime.join("run"),
        ] {
            std::fs::write(exe, b"").unwrap();
        }
        std::fs::write(
            server.join(crate::run_ops::SERVER_CONFIG),
            "BattlEye = 1;\n",
        )
        .unwrap();
        std::fs::write(
            source.join("server/launch-sets.json"),
            json!({"sets": {"session": {"mods": [
                {"folder": folder_a},
                {"folder": folder_b},
                {"local": "."}
            ]}}})
            .to_string(),
        )
        .unwrap();

        let plan = DiagRunDiag {
            control: RunControl::new(),
            db: None,
            settings: None,
            actor: "user".to_owned(),
        }
        .plan(json!({
            "mod_dir": own,
            "source_root": source,
            "game_dir": game,
            "server_dir": server,
            "steam_dir": steam,
            "profiles_dir": root.join("profiles"),
            "set": "session",
        }))
        .unwrap()
        .expect("a preview");

        let launches = plan["launches"].as_array().unwrap();
        assert_eq!(launches[0]["role"], "server");
        assert_eq!(launches[1]["role"], "client");
        for launch in launches {
            let args: Vec<&str> = launch["args"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert!(args.contains(&"-mod=@FolderA;@FolderB;@mod"), "{args:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_server_preview_stages_the_named_sets_config() {
        let root = tmp("server-set");
        let server = root.join("server-install");
        let source = root.join("mod");
        let own = source.join("builds/default/@mod");
        let folder = root.join("mods/@FolderA");
        let cfg = "hostname = \"expansion\";\n";
        for dir in [&server, &source.join("server"), &own, &folder] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(server.join("DayZServer"), b"").unwrap();
        std::fs::write(
            server.join(crate::run_ops::SERVER_CONFIG),
            "hostname = \"install\";\n",
        )
        .unwrap();
        std::fs::write(source.join("server/serverDZ_expansion.cfg"), cfg).unwrap();
        std::fs::write(
            source.join("server/launch-sets.json"),
            json!({"sets": {"session": {
                "mods": [{"folder": folder}, {"local": "."}],
                "config": "serverDZ_expansion.cfg",
            }}})
            .to_string(),
        )
        .unwrap();

        let plan = DiagRun {
            control: RunControl::new(),
            db: None,
            settings: None,
            actor: "user".to_owned(),
        }
        .plan(json!({
            "mod_dir": own,
            "source_root": source,
            "server_dir": server,
            "profiles_dir": root.join("profiles"),
            "set": "session",
        }))
        .unwrap()
        .expect("a preview");

        let args: Vec<&str> = plan["launches"][0]["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            args.contains(&"-config=serverDZ_modfoundry.cfg"),
            "{args:?}"
        );
        assert!(args.contains(&"-mod=@FolderA;@mod"), "{args:?}");
        let staged = server.join(crate::diag_stage::DIAG_CONFIG_NAME);
        assert!(
            plan["steps"].as_array().unwrap().iter().any(|step| {
                step["op"] == "write" && step["path"] == json!(staged) && step["bytes"] == cfg.len()
            }),
            "{}",
            plan["steps"]
        );
        assert!(
            !source
                .join("server")
                .join(crate::run_ops::SERVER_CONFIG)
                .exists(),
            "the default template is not made"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_detached_run_without_a_limit_waits_on_no_timer() {
        let cases = [
            (false, json!({ "mod_dir": "/m" }), NO_TIMER_SECS, None),
            (
                false,
                json!({ "mod_dir": "/m", "timeout_secs": 1 }),
                1,
                Some(1),
            ),
            (
                true,
                json!({ "mod_dir": "/m" }),
                crate::run_ops::DEFAULT_RUN_TIMEOUT_SECS,
                None,
            ),
            (
                true,
                json!({ "mod_dir": "/m", "timeout_secs": 30 }),
                30,
                None,
            ),
        ];
        for (wait, args, window, limit) in cases {
            let (parsed, named) = diag_run_args(wait, args).unwrap();
            assert_eq!(parsed.timeout_secs, window);
            assert_eq!(named, limit);
        }
        assert!(
            diag_run_args(false, json!({ "mod_dir": "/m", "timeout_secs": null })).is_err(),
            "a null timeout is refused, not read as no limit"
        );
    }

    #[test]
    fn run_diag_advertises_an_optional_timeout() {
        let schema = DiagRunDiag {
            control: RunControl::new(),
            db: None,
            settings: None,
            actor: "user".to_owned(),
        }
        .args_schema();

        assert_eq!(
            schema["properties"]["timeout_secs"]["description"],
            "optional; omit to keep running until stopped"
        );
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "timeout_secs"));
    }

    #[test]
    fn the_three_run_commands_advertise_their_verdict_lists_and_set() {
        let schemas = [
            DiagRun {
                control: RunControl::new(),
                db: None,
                settings: None,
                actor: "user".to_owned(),
            }
            .args_schema(),
            DiagRunClient {
                control: RunControl::new(),
                db: None,
                settings: None,
                actor: "user".to_owned(),
            }
            .args_schema(),
            DiagRunDiag {
                control: RunControl::new(),
                db: None,
                settings: None,
                actor: "user".to_owned(),
            }
            .args_schema(),
        ];

        for schema in &schemas {
            for key in ["expect_lines", "fail_lines", "allow_errors"] {
                assert_eq!(schema["properties"][key]["type"], "array", "{schema}");
                assert_eq!(
                    schema["properties"][key]["items"]["type"], "string",
                    "{schema}"
                );
            }
            assert_eq!(schema["properties"]["set"]["type"], "string", "{schema}");
        }
        assert_eq!(schemas[2]["properties"]["config"]["type"], "string");
    }

    #[test]
    fn run_diag_args_without_a_timeout_round_trip_as_none() {
        let base = json!({ "mod_dir": "/mods/@X", "source_root": "/src" });

        let absent: RunDiagArgs = parse_args(base.clone()).unwrap();
        assert_eq!(absent.timeout_secs, None);
        let round_tripped: RunDiagArgs =
            parse_args(serde_json::to_value(&absent).unwrap()).unwrap();
        assert_eq!(round_tripped.timeout_secs, None);

        let mut null = base.clone();
        null["timeout_secs"] = json!(null);
        assert_eq!(parse_args::<RunDiagArgs>(null).unwrap().timeout_secs, None);

        let mut bounded = base;
        bounded["timeout_secs"] = json!(42);
        assert_eq!(
            parse_args::<RunDiagArgs>(bounded).unwrap().timeout_secs,
            Some(42)
        );
    }
}
