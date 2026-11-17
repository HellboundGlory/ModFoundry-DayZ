//! Shared launch + diagnostics bodies (`mf run`, `mf errors`), the single
//! implementation CLI, MCP and automation bind. Launching drives the
//! engine process through `modfoundry-diag`; error parsing + file mapping live
//! there too — this crate only orchestrates and owns no parsing logic.

use crate::diag_run::{default_windowed, PruneOnDrop};
use crate::diag_stage::{check_test_mods, TestModArg, TestModRole, DIAG_CONFIG_NAME};
use crate::known_errors::KnownErrors;
use crate::launch_plan::{LaunchRole, PlannedLaunch, RunPlan, StageStep};
use modfoundry_diag::{
    count_storage_errors, map_logs, parse_log, storage_flood_hint, ClientProfile, ClientRunReport,
    Connect, LaunchCommand, LaunchKind, LinePattern, LogFlags, LogStates, MappedError, MarkerWait,
    Resolver, RunReport, ServerProfile, StopSignal, Verdict, VerdictRules,
};
use modfoundry_platform::stage::{clear_dead_runs, ensure_dir, Staged, JOURNAL_NAME};
use modfoundry_vfs::root_name;
use modfoundry_vfs::write::WriteTxn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The stop signals of in-flight launches, keyed by the caller's run id. Shared
/// by `diag.run`, `diag.run_client`, `diag.run_diag` and `diag.stop` so a
/// frontend can cancel a run it started.
#[derive(Clone, Default)]
pub struct RunControl(Arc<Mutex<HashMap<String, Vec<StopSignal>>>>);

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh signal under `run_id`, returning the handle the launch
    /// polls. Registrations under one id add up; with a run store, a second
    /// launch under a live id is refused before it gets here.
    fn begin(&self, run_id: &str) -> StopSignal {
        let signal = StopSignal::new();
        self.map()
            .entry(run_id.to_string())
            .or_default()
            .push(signal.clone());
        signal
    }

    /// Drop `signal`'s registration once its job ends; siblings stay stoppable.
    fn end(&self, run_id: &str, signal: &StopSignal) {
        let mut map = self.map();
        if let Some(signals) = map.get_mut(run_id) {
            signals.retain(|s| !s.same_flag(signal));
            if signals.is_empty() {
                map.remove(run_id);
            }
        }
    }

    /// Run `run` under `run_id`'s stop signal, registering and dropping it around the call.
    pub fn scoped<R>(&self, run_id: Option<&str>, run: impl FnOnce(&StopSignal) -> R) -> R {
        let Some(id) = run_id else {
            return run(&StopSignal::new());
        };
        let signal = self.begin(id);
        let _registration = Registration {
            control: self,
            run_id: id,
            signal: &signal,
        };
        run(&signal)
    }

    /// Set the stop signal of a live run; `false` when no run has that id.
    pub fn stop(&self, run_id: &str) -> bool {
        match self.map().get(run_id) {
            Some(signals) => {
                signals.iter().for_each(StopSignal::stop);
                true
            }
            None => false,
        }
    }

    /// Stop every live run and wait up to `timeout` for each to clean up and leave.
    pub fn stop_all_and_wait(&self, timeout: Duration) {
        self.map().values().flatten().for_each(StopSignal::stop);
        let deadline = Instant::now() + timeout;
        while !self.map().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A poisoned lock only means another thread panicked while holding it; the
    /// map is still consistent, so recover rather than propagate.
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<StopSignal>>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct Registration<'a> {
    control: &'a RunControl,
    run_id: &'a str,
    signal: &'a StopSignal,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.control.end(self.run_id, self.signal);
    }
}

/// Quiet period `mf run-client` waits out before treating the client log
/// capture as complete. Must exceed the client's longest boot gap (~27s
/// observed between the RPT header and world output).
pub const CLIENT_SETTLE_SECS: u64 = 30;

/// How often `mf run-client` polls for the client's exit while it stays live.
pub(crate) const CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A windowed client size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl WindowSize {
    /// Parse a `WxH` size (the `mf run-client --window` value).
    pub fn parse(s: &str) -> Option<Self> {
        let (w, h) = s.split_once('x')?;
        Some(Self {
            width: w.trim().parse().ok()?,
            height: h.trim().parse().ok()?,
        })
    }
}

/// The windowed size a client run opens at unless the caller overrides it.
pub const DEFAULT_WINDOW: WindowSize = WindowSize {
    width: 1280,
    height: 720,
};

/// The windowed-mode args to prepend to a client profile's extras: `window`
/// when given, else [`DEFAULT_WINDOW`]; empty when `extra` already carries a
/// `-window`/`-x=`/`-y=` arg (the caller's args win).
pub(crate) fn window_args(window: Option<WindowSize>, extra: &[String]) -> Vec<String> {
    if extra
        .iter()
        .any(|a| a == "-window" || a.starts_with("-x=") || a.starts_with("-y="))
    {
        return Vec::new();
    }
    let WindowSize { width, height } = window.unwrap_or(DEFAULT_WINDOW);
    vec![
        "-window".into(),
        format!("-x={width}"),
        format!("-y={height}"),
    ]
}

/// Spawn `exe` with `argv` (the resolved `mf run` profile) and capture the log
/// files it writes under `profiles_dir`, waiting up to `timeout_secs`.
pub fn run_profile(
    exe: &Path,
    argv: &[String],
    profiles_dir: &Path,
    timeout_secs: u64,
    stop: &StopSignal,
) -> anyhow::Result<RunReport> {
    ensure_dir(profiles_dir)?;
    modfoundry_diag::capture_run(exe, argv, profiles_dir, timeout_secs, stop)
}

/// Arguments for the `mf run` shared body (the native-server test launch).
/// Frontends hand this to the registry as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunServerArgs {
    /// The built `@Mod` dir to load via `-mod=` (content root).
    pub mod_dir: PathBuf,
    /// DayZServer install dir override; default: platform discovery.
    #[serde(default)]
    pub server_dir: Option<PathBuf>,
    /// `-config=…` server config file (relative to the server dir).
    #[serde(default)]
    pub config: Option<PathBuf>,
    /// A named launch set whose mods and config this run takes.
    #[serde(default)]
    pub set: Option<String>,
    /// The set's config file, staged from the project's server folder like the default one.
    #[serde(skip)]
    pub set_config: Option<PathBuf>,
    /// `-port=…`.
    #[serde(default)]
    pub port: Option<u16>,
    /// Where the server writes its profiles/logs (capture + then `mf errors`).
    /// `None` = `<source_root>/server/profiles` when a source root is given, else a
    /// fresh temp dir; reported back in [`RunOutcome::profiles_dir`].
    #[serde(default)]
    pub profiles_dir: Option<PathBuf>,
    /// Bounded launch window in seconds before we stop the server.
    #[serde(default = "default_run_timeout_secs")]
    pub timeout_secs: u64,
    /// Loose mod source root used to resolve logged paths to host files (the
    /// built `@Mod` under the server dir is packed; pass the project `Scripts`
    /// tree for file-mapped errors).
    #[serde(default)]
    pub source_root: Option<PathBuf>,
    /// Optional id under which the caller can stop this run via `diag.stop`.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Mods staged for this run only: `Both` load with `-mod=`, `ServerOnly` with `-serverMod=`.
    #[serde(default)]
    pub test_mods: Vec<TestModArg>,
    /// Lines the run's logs must show for its verdict to pass (`re:` prefix: a regular expression).
    #[serde(default)]
    pub expect_lines: Vec<String>,
    /// Lines that fail the run's verdict when they appear.
    #[serde(default)]
    pub fail_lines: Vec<String>,
    /// Known script errors the run's verdict ignores.
    #[serde(default)]
    pub allow_errors: Vec<String>,
}

/// Default bounded launch window (seconds) for a server test run.
pub const DEFAULT_RUN_TIMEOUT_SECS: u64 = 90;

/// Default bounded launch window for a server test run.
pub(crate) fn default_run_timeout_secs() -> u64 {
    DEFAULT_RUN_TIMEOUT_SECS
}

/// Default quiet period for a client capture.
pub(crate) fn default_settle_secs() -> u64 {
    CLIENT_SETTLE_SECS
}

/// Result of a `mf run` launch: where the capture landed, the raw capture
/// report, and any file-mapped errors already extracted from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub profiles_dir: PathBuf,
    pub report: RunReport,
    pub errors: Vec<MappedError>,
    /// A likely cause for a saved world the server could not load.
    #[serde(default)]
    pub hint: Option<String>,
    /// What a run that died left in the install and this one removed.
    #[serde(default)]
    pub cleared: Vec<PathBuf>,
    /// How the run's logs judged against the caller's verdict lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
}

/// The line a launch prints for a path an earlier run left behind.
pub fn cleared_line(path: &Path) -> String {
    format!("removed what an earlier run left: {}", path.display())
}

thread_local! {
    /// Set on the thread a detached launch runs on, whose caller never sees its summary.
    static DETACHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the calling thread as running a detached launch, whose stdout no caller prints.
pub(crate) fn mark_detached() {
    DETACHED.with(|detached| detached.set(true));
}

fn detached() -> bool {
    DETACHED.with(std::cell::Cell::get)
}

/// The lines a run with `detached` set prints for what it swept: a foreground
/// run's caller prints them from the outcome instead, so it prints none.
fn cleared_echo(detached: bool, cleared: &[PathBuf]) -> Vec<String> {
    if detached {
        cleared.iter().map(|path| cleared_line(path)).collect()
    } else {
        Vec::new()
    }
}

/// Removes what the runs that died left in `install`, saying so on stderr for a detached run.
pub(crate) fn clear_dead(install: &Path) -> anyhow::Result<Vec<PathBuf>> {
    match clear_dead_runs(install) {
        Ok(removed) => {
            report_cleared(std::io::stderr(), &removed);
            Ok(removed)
        }
        Err(failure) => {
            report_cleared(std::io::stderr(), &failure.removed);
            Err(anyhow::anyhow!(
                "clearing {}: {failure}",
                install.join(JOURNAL_NAME).display()
            ))
        }
    }
}

fn report_cleared(mut out: impl std::io::Write, paths: &[PathBuf]) {
    for line in cleared_echo(detached(), paths) {
        let _ = writeln!(out, "{line}");
    }
}

/// Resolve the DayZServer install dir: explicit override, else platform
/// discovery. Fails loudly when the box has no DayZServer install.
pub(crate) fn resolve_server_dir(server_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    match server_dir {
        Some(d) => Ok(d.to_path_buf()),
        None => modfoundry_platform::find_dayz_server().ok_or_else(|| {
            anyhow::anyhow!("DayZServer install not found; pass --server or run `mf doctor`")
        }),
    }
}

/// Resolve the Steam root: explicit override, else platform discovery. Fails
/// loudly when no Steam installation is found.
pub(crate) fn resolve_steam_root(steam_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    match steam_dir {
        Some(d) => Ok(d.to_path_buf()),
        None => modfoundry_platform::discover_steam_installations()
            .into_iter()
            .next()
            .map(|s| s.root)
            .ok_or_else(|| anyhow::anyhow!("no Steam installation found; pass --steam-dir")),
    }
}

/// Resolve the native server exe: explicit install dir, else platform
/// discovery. Fails loudly when the box has no DayZServer install.
pub fn resolve_server_exe(server_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let dir = resolve_server_dir(server_dir)?;
    let exe = if cfg!(windows) {
        dir.join("DayZServer_x64.exe")
    } else {
        dir.join("DayZServer")
    };
    if !exe.is_file() {
        anyhow::bail!("server binary not found at {}", exe.display());
    }
    Ok(exe)
}

/// A capture dir the caller did not pin: unique per process, instant and call so
/// repeated runs never read each other's logs.
pub(crate) fn scratch_profiles_dir() -> PathBuf {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mf-run-{}-{secs}-{n}", std::process::id()))
}

/// The server config a DayZServer install ships, relative to the install dir.
pub(crate) const SERVER_CONFIG: &str = "serverDZ.cfg";

/// `args` with a throwaway capture folder pinned when the caller gave neither one nor a source
/// root, so a plan and the run that follows it name one folder.
fn pin_capture_dir(args: &RunServerArgs) -> RunServerArgs {
    let mut args = args.clone();
    if args.profiles_dir.is_none() && args.source_root.is_none() {
        args.profiles_dir = Some(scratch_profiles_dir());
    }
    args
}

/// Where a run captures its logs: the caller's folder, else the project's `server/profiles`.
fn capture_dir(args: &RunServerArgs) -> anyhow::Result<PathBuf> {
    match (&args.profiles_dir, args.source_root.as_deref()) {
        (Some(dir), _) => Ok(dir.clone()),
        (None, Some(root)) => Ok(crate::diag_run::project_profiles_dir(&std::path::absolute(
            root,
        )?)),
        (None, None) => Ok(scratch_profiles_dir()),
    }
}

/// The DayZ engine resolves `-mod` paths relative to its install dir and
/// *ignores absolute paths on Linux* (verified empirically against the native
/// server), so a build outside the install is staged as an `@name` symlink the
/// way the client launch does, and the link is removed after the run so the
/// user's install is left as it was.
fn server_mod(
    mod_dir: &Path,
    server_dir: &Path,
    gone: &[PathBuf],
) -> anyhow::Result<(PathBuf, Option<StageStep>)> {
    match mod_dir.strip_prefix(server_dir) {
        Ok(rel) => Ok((rel.to_path_buf(), None)),
        Err(_) => {
            let link = mod_link(server_dir, mod_dir)?;
            let step = link_step(&link.link, &link.target, gone)?;
            Ok((link.arg, step))
        }
    }
}

/// The `.pbo` files in the mod's `addons` folder with no `.bisign` beside them,
/// sorted. A mod without an `addons` folder has none.
pub fn unsigned_pbos(mod_dir: &Path) -> Vec<String> {
    let files: Vec<String> = std::fs::read_dir(mod_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|dir| {
            dir.file_type().is_ok_and(|kind| kind.is_dir())
                && dir.file_name().eq_ignore_ascii_case("addons")
        })
        .flat_map(|dir| {
            let folder = dir.file_name().to_string_lossy().into_owned();
            std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter(|file| file.file_type().is_ok_and(|kind| kind.is_file()))
                .map(move |file| format!("{folder}/{}", file.file_name().to_string_lossy()))
        })
        .collect();
    let mut unsigned: Vec<String> = modfoundry_build::publish::checks::unsigned_pbos(&files)
        .into_iter()
        .map(|file| {
            file.rsplit_once('/')
                .map_or(file.clone(), |(_, name)| name.to_owned())
        })
        .collect();
    unsigned.sort();
    unsigned
}

fn require_signed(mod_dir: &Path) -> anyhow::Result<()> {
    let unsigned = unsigned_pbos(mod_dir);
    if unsigned.is_empty() {
        return Ok(());
    }
    let name = mod_dir
        .file_name()
        .unwrap_or(mod_dir.as_os_str())
        .to_string_lossy();
    anyhow::bail!(
        "{name} is not signed ({}); a retail server refuses unsigned mods. Pick or create a key in Settings › Signing, or give the build an authority",
        unsigned.join(", ")
    )
}

/// The `keys/` folder the mods' public keys are staged into, and the copies that put them
/// there, so a server running `verifySignatures = 2` accepts the signed mod. A key already
/// present with identical bytes is the user's own and is left alone; one with different
/// bytes fails the run rather than clobbering it.
fn key_steps(
    mod_dirs: &[&Path],
    server_dir: &Path,
    gone: &[PathBuf],
) -> anyhow::Result<Vec<StageStep>> {
    let keys_dir = server_dir.join("keys");
    let mut has_dir = keys_dir.is_dir() && !absent(gone, &keys_dir);
    let mut steps = Vec::new();
    // Two mods may ship one key, so a destination this plan already fills counts as taken.
    let mut planned: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for src in mod_dirs.iter().flat_map(|mod_dir| key_folders(mod_dir)) {
        for entry in std::fs::read_dir(&src)? {
            let path = entry?.path();
            let is_key = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bikey"));
            if !is_key || !path.is_file() {
                continue;
            }
            let dest = keys_dir.join(path.file_name().unwrap());
            let bytes = std::fs::read(&path)?;
            let installed = match planned.get(&dest) {
                Some(there) => Some(there.clone()),
                None if dest.is_file() && !absent(gone, &dest) => Some(std::fs::read(&dest)?),
                None => None,
            };
            if let Some(installed) = installed {
                if installed == bytes {
                    continue;
                }
                anyhow::bail!(
                    "server key conflict at {}: it differs from the build's {}; \
                     move the existing key aside and run again",
                    dest.display(),
                    path.display()
                );
            }
            if !has_dir {
                steps.push(StageStep::CreateDir {
                    path: keys_dir.clone(),
                });
                has_dir = true;
            }
            planned.insert(dest.clone(), bytes);
            steps.push(StageStep::Copy {
                from: path,
                to: dest,
            });
        }
    }
    Ok(steps)
}

/// The mod's key folders, sorted: workshop mods also spell them `Keys`, `Key` and `key`.
fn key_folders(mod_dir: &Path) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(mod_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            entry.path().is_dir()
                && (name.eq_ignore_ascii_case("keys") || name.eq_ignore_ascii_case("key"))
        })
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    folders
}

pub(crate) fn server_log_dirs(args: &RunServerArgs) -> Vec<PathBuf> {
    let kept = args
        .source_root
        .as_deref()
        .and_then(|root| project_server_dir(root).ok())
        .map(|server| server.join("profiles"));
    args.profiles_dir.clone().or(kept).into_iter().collect()
}

/// Launch the native server for `args` (bounded), capture its logs, then map
/// the resulting errors back to the mod's sources. This is the body `mf run`
/// binds: a build → launch → capture → map round trip in one call.
pub fn run_server(args: &RunServerArgs, stop: &StopSignal) -> anyhow::Result<RunOutcome> {
    let known = KnownErrors::load(None, args.source_root.as_deref())?;
    let rules = verdict_rules(
        &args.expect_lines,
        &args.fail_lines,
        &args.allow_errors,
        &known,
        || verdict_mods(std::iter::once(args.mod_dir.as_path()), &args.test_mods),
    )?;
    let args = &pin_capture_dir(args);
    let server_dir = resolve_server_dir(args.server_dir.as_deref())?;
    let cleared = clear_dead(&server_dir)?;
    let plan = plan_server(args, &[])?;
    let profiles_dir = capture_dir(args)?;
    let _prune = PruneOnDrop(args.profiles_dir.is_none().then(|| profiles_dir.clone()));
    apply_project(&plan.project)?;
    let mut staged = Staged::in_install(&server_dir);
    crate::launch_plan::apply(&plan.steps, &mut staged)?;
    let command = plan
        .launches
        .first()
        .ok_or_else(|| anyhow::anyhow!("the plan has no launch"))?;
    let watch = VerdictWatch::spawn(
        &rules,
        std::slice::from_ref(&profiles_dir),
        stop,
        Duration::from_secs(args.timeout_secs),
    );
    let earlier = modfoundry_diag::log_states(&profiles_dir);
    let report =
        modfoundry_diag::capture_command(&command.command, &profiles_dir, args.timeout_secs, stop)?;
    drop(watch);
    // Map against the loose source root when given (resolves packed-mod log
    // paths to host files); otherwise fall back to the built @Mod dir, which
    // only matches when the @Mod is unpacked/filepatched.
    let map_root = args.source_root.as_deref().unwrap_or(&args.mod_dir);
    let mut errors = errors_from_logs(&report.logs, &earlier, Some(map_root))?;
    known.mark(&mut errors);
    let hint = storage_flood_hint(count_storage_errors(&report.logs, &earlier));
    let verdict = judge_logs(&report.logs, &earlier, &rules)?;
    Ok(RunOutcome {
        profiles_dir,
        report,
        errors,
        hint,
        cleared,
        verdict,
    })
}

/// Works out every check, path, argument and file a server run uses, without writing
/// anything. Paths in `gone` are treated as absent: they are a dead run's leftovers,
/// which the launch removes first.
pub(crate) fn plan_server(args: &RunServerArgs, gone: &[PathBuf]) -> anyhow::Result<RunPlan> {
    let args = &pin_capture_dir(args);
    check_server(args)?;
    let exe = resolve_server_exe(args.server_dir.as_deref())?;
    server_plan(args, &exe, gone)
}

/// The refusals of a server run, which come before its server binary is resolved.
fn check_server(args: &RunServerArgs) -> anyhow::Result<()> {
    require_signed(&args.mod_dir)?;
    check_test_mods(&args.test_mods)?;
    refuse_build_names(std::slice::from_ref(&args.mod_dir), &args.test_mods)
}

fn server_plan(args: &RunServerArgs, exe: &Path, gone: &[PathBuf]) -> anyhow::Result<RunPlan> {
    let server_dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("server exe {} has no parent dir", exe.display()))?
        .to_path_buf();
    let mut steps = Vec::new();
    let (build_arg, build_link) = server_mod(&args.mod_dir, &server_dir, gone)?;
    steps.extend(build_link);
    let mod_dirs: Vec<&Path> = std::iter::once(args.mod_dir.as_path())
        .chain(
            args.test_mods
                .iter()
                .map(|test_mod| test_mod.path.as_path()),
        )
        .collect();
    steps.extend(key_steps(&mod_dirs, &server_dir, gone)?);
    let mut mods = Vec::new();
    let mut server_mods = Vec::new();
    for test_mod in &args.test_mods {
        let link = named_link(&server_dir, &test_mod.path, &test_mod.link_name)?;
        steps.extend(link_step(&link.link, &link.target, gone)?);
        match test_mod.role {
            TestModRole::Both => mods.push(link.arg),
            TestModRole::ServerOnly => server_mods.push(link.arg),
        }
    }
    mods.push(build_arg);
    let mut project = Vec::new();
    let profiles_dir = capture_dir(args)?;
    if !profiles_dir.is_dir() {
        project.push(StageStep::CreateDir {
            path: profiles_dir.clone(),
        });
    }
    let config = plan_config(args, &server_dir, gone, &mut steps, &mut project)?;
    let profile = ServerProfile {
        kind: LaunchKind::Server,
        config,
        mission: None,
        port: args.port,
        profiles: Some(profiles_dir),
        mods,
        server_mods,
        ip: None,
        flags: LogFlags {
            do_logs: true,
            admin_log: true,
            freeze_check: true,
            ..Default::default()
        },
    };
    let command = LaunchCommand {
        program: exe.to_path_buf(),
        args: profile.argv().into_iter().map(OsString::from).collect(),
        cwd: Some(server_dir),
        env: Vec::new(),
        env_remove: Vec::new(),
    };
    Ok(RunPlan {
        launches: vec![PlannedLaunch {
            role: LaunchRole::Server,
            command,
        }],
        steps,
        project,
        leftovers: gone.to_vec(),
    })
}

/// `<source_root>/server`, absolute: the server runs in its install dir, so a
/// relative `-profiles=` would land there.
fn project_server_dir(source_root: &Path) -> anyhow::Result<PathBuf> {
    if !source_root.is_dir() {
        anyhow::bail!("source root {} is not a folder", source_root.display());
    }
    Ok(std::path::absolute(source_root)?.join("server"))
}

/// The `-config=` choice of a server run, with the project config it keeps and the config it stages.
/// Without `-config` the server shuts itself down ten seconds after start, so the run defaults to
/// the project's copy, else the config every DayZServer install ships.
fn plan_config(
    args: &RunServerArgs,
    server_dir: &Path,
    gone: &[PathBuf],
    steps: &mut Vec<StageStep>,
    project: &mut Vec<StageStep>,
) -> anyhow::Result<Option<PathBuf>> {
    if let Some(config) = &args.config {
        return Ok(Some(config.clone()));
    }
    let Some(project_server) = args
        .source_root
        .as_deref()
        .map(project_server_dir)
        .transpose()?
    else {
        let shipped = server_dir.join(SERVER_CONFIG);
        return Ok(shipped.is_file().then(|| PathBuf::from(SERVER_CONFIG)));
    };
    let name = args
        .set_config
        .as_deref()
        .unwrap_or(Path::new(SERVER_CONFIG));
    let project_cfg = crate::diag_stage::project_config_path(&project_server, name);
    let bytes = if project_cfg.is_file() {
        std::fs::read(&project_cfg)
            .map_err(|error| anyhow::anyhow!("reading {}: {error}", project_cfg.display()))?
    } else if args.set_config.is_some() {
        anyhow::bail!("server config {} is missing", project_cfg.display());
    } else {
        let shipped = server_dir.join(SERVER_CONFIG);
        if !shipped.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&shipped)
            .map_err(|error| anyhow::anyhow!("reading {}: {error}", shipped.display()))?;
        project.push(StageStep::Write {
            path: project_cfg,
            contents: bytes.clone(),
            bytes: bytes.len(),
        });
        bytes
    };
    let staged = server_dir.join(DIAG_CONFIG_NAME);
    refuse_staged(&staged, gone)?;
    steps.push(StageStep::Write {
        path: staged,
        contents: bytes.clone(),
        bytes: bytes.len(),
    });
    Ok(Some(PathBuf::from(DIAG_CONFIG_NAME)))
}

/// Refuses to stage over a file at `path` unless nothing is there or a dead run left it: the
/// staged file is the proof a run is live, and replacing it would take the file from that run.
fn refuse_staged(path: &Path, gone: &[PathBuf]) -> anyhow::Result<()> {
    if absent(gone, path) {
        return Ok(());
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => anyhow::bail!(
            "{} is a directory; remove it to stage the run",
            path.display()
        ),
        Ok(_) => anyhow::bail!(
            "{} is already in the server folder: another run is using it, \
             or something else left it; stop that run or move the file aside and run again",
            path.display()
        ),
        Err(_) => Ok(()),
    }
}

/// Whether `path` is one a dead run left, which the launch removes before it stages. `gone` is
/// in the form a journal records, so `path` is matched in that form.
pub(crate) fn absent(gone: &[PathBuf], path: &Path) -> bool {
    !gone.is_empty()
        && modfoundry_platform::stage::journal_path(path).is_ok_and(|path| gone.contains(&path))
}

/// Applies the project files and folders a run makes and keeps.
pub fn apply_project(steps: &[StageStep]) -> anyhow::Result<()> {
    for step in steps {
        match step {
            StageStep::CreateDir { path } => ensure_dir(path)?,
            StageStep::Write { path, contents, .. } => write_project_file(path, contents)?,
            other => anyhow::bail!("{} is not a project step", other.path().display()),
        }
    }
    Ok(())
}

fn write_project_file(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no folder", path.display()))?;
    let mut txn = WriteTxn::unrecorded(root_name("project", dir), dir);
    txn.write(crate::file_rel(path)?, contents.to_vec())?;
    txn.finish()?;
    Ok(())
}

/// Parse every `.rpt`/`.log`/`.adm` file under `dir` and map the
/// resulting errors to source files under `source_root` (when given). Returns
/// file-attributable errors only (info lines are dropped).
pub fn errors_from_dir(dir: &Path, source_root: Option<&Path>) -> anyhow::Result<Vec<MappedError>> {
    errors_from_logs(
        &modfoundry_diag::find_logs(dir),
        &LogStates::new(),
        source_root,
    )
}

/// Parse an explicit set of log files and map their errors (the shared body
/// behind [`errors_from_dir`]; also used to map only the *new* logs a client
/// run produced, without re-reading earlier sessions' files). Only the part of
/// each log that this run wrote is read.
pub fn errors_from_logs(
    logs: &[PathBuf],
    earlier: &LogStates,
    source_root: Option<&Path>,
) -> anyhow::Result<Vec<MappedError>> {
    let resolver = match source_root {
        Some(root) if root.is_dir() => Resolver::scan(root),
        _ => Resolver::default(),
    };
    let mut all = Vec::new();
    for log in logs {
        let text = modfoundry_diag::run_log_text(log, earlier)?;
        let records = parse_log(&text);
        all.extend(map_logs(&records, &resolver));
    }
    // Log files are the source of truth for ordering: keep discovery order.
    Ok(modfoundry_diag::dedup_errors(all))
}

/// Whether any of a run's three verdict lists asks for a verdict.
pub(crate) fn wants_verdict(expect: &[String], fail: &[String], allow: &[String]) -> bool {
    !expect.is_empty() || !fail.is_empty() || !allow.is_empty()
}

/// The lines parsed into patterns; a bad one names itself.
pub(crate) fn parse_patterns(lines: &[String]) -> Result<Vec<LinePattern>, String> {
    lines.iter().map(|line| LinePattern::parse(line)).collect()
}

/// The rules a run's three lists, its project's known errors and mod names judge its logs
/// by; `None` when no list is set.
pub(crate) fn verdict_rules(
    expect: &[String],
    fail: &[String],
    allow: &[String],
    known: &KnownErrors,
    mods: impl FnOnce() -> Vec<String>,
) -> anyhow::Result<Option<VerdictRules>> {
    if !wants_verdict(expect, fail, allow) {
        return Ok(None);
    }
    let parse = |lines: &[String]| parse_patterns(lines).map_err(|e| anyhow::anyhow!("{e}"));
    let mut allow = parse(allow)?;
    allow.extend(known.patterns());
    Ok(Some(VerdictRules {
        expect: parse(expect)?,
        fail: parse(fail)?,
        allow,
        allow_fingerprints: known.fingerprints(),
        mods: mods(),
    }))
}

/// The names that tie a script error to this run: a mod's folder name without `@` and its PBO
/// prefixes, and every test mod's link name and readable PBO prefixes.
pub(crate) fn verdict_mods<'a>(
    mod_dirs: impl IntoIterator<Item = &'a Path>,
    test_mods: &[TestModArg],
) -> Vec<String> {
    let mut mods = Vec::new();
    for dir in mod_dirs {
        if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
            mods.push(name.trim_start_matches('@').to_owned());
        }
        mods.extend(crate::diag_run::pbo_prefixes(dir));
    }
    for test_mod in test_mods {
        mods.push(test_mod.link_name.clone());
        mods.extend(crate::diag_run::pbo_prefixes(&test_mod.path));
    }
    mods
}

/// The names that tie a client run's script errors to it: its own mod, the last of `mods`, and the test mods.
pub(crate) fn client_verdict_mods(args: &RunClientArgs) -> Vec<String> {
    verdict_mods(args.mods.last().map(PathBuf::as_path), &args.test_mods)
}

/// Judge a run's rules against the logs it wrote, reading only the part this run
/// wrote; `None` when the caller set no verdict list.
pub(crate) fn judge_logs(
    logs: &[PathBuf],
    earlier: &LogStates,
    rules: &Option<VerdictRules>,
) -> anyhow::Result<Option<Verdict>> {
    let Some(rules) = rules else {
        return Ok(None);
    };
    let texts: Vec<(PathBuf, String)> = logs
        .iter()
        .map(|log| {
            let text = modfoundry_diag::run_log_text(log, earlier)?;
            Ok((log.clone(), text))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(modfoundry_diag::judge(rules, &texts)))
}

/// How long a run goes on after every expected line has appeared, so its logs settle
/// before the verdict judges them.
#[cfg(test)]
const VERDICT_GRACE: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const VERDICT_GRACE: Duration = Duration::from_secs(5);

/// Follows a run's RPT logs for the lines it expects; once every one has appeared it
/// waits [`VERDICT_GRACE`] and stops the run.
pub(crate) struct VerdictWatch {
    cancel: StopSignal,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl VerdictWatch {
    /// The watcher for a run that expects lines, following `dirs` from before the run;
    /// `None` when the run expects none.
    pub(crate) fn spawn(
        rules: &Option<VerdictRules>,
        dirs: &[PathBuf],
        stop: &StopSignal,
        window: Duration,
    ) -> Option<Self> {
        let patterns = rules.as_ref().map(|rules| rules.expect.clone())?;
        if patterns.is_empty() {
            return None;
        }
        let watched: Vec<(PathBuf, LogStates)> = dirs
            .iter()
            .map(|dir| (dir.clone(), modfoundry_diag::log_states(dir)))
            .collect();
        let cancel = StopSignal::new();
        let watched_by_thread = cancel.clone();
        let stop = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut matched = vec![false; patterns.len()];
            let mut line_seen = |line: &str| {
                for (hit, pattern) in matched.iter_mut().zip(&patterns) {
                    *hit |= pattern.matches(line);
                }
                matched.iter().all(|hit| *hit)
            };
            let watched: Vec<(&Path, &LogStates)> = watched
                .iter()
                .map(|(dir, states)| (dir.as_path(), states))
                .collect();
            let found = modfoundry_diag::run::wait_for_log_lines(
                &watched,
                window,
                &watched_by_thread,
                &mut line_seen,
                || std::thread::sleep(crate::diag_run::MARKER_POLL),
            );
            if found == MarkerWait::Found {
                wait_grace(&watched_by_thread);
                if !watched_by_thread.is_stopped() {
                    stop.stop();
                }
            }
        });
        Some(Self {
            cancel,
            handle: Some(handle),
        })
    }
}

/// Waits [`VERDICT_GRACE`], ending early when `cancel` is set.
fn wait_grace(cancel: &StopSignal) {
    let deadline = Instant::now() + VERDICT_GRACE;
    while !cancel.is_stopped() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

impl Drop for VerdictWatch {
    fn drop(&mut self) {
        self.cancel.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Arguments for the client-under-Proton test launch (`mf run-client`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunClientArgs {
    /// Built `@Mod` dirs to stage into the client install and load (order preserved). The last
    /// entry is the run's own mod for the verdict.
    pub mods: Vec<PathBuf>,
    /// A named launch set whose shared mods this run loads.
    #[serde(default)]
    pub set: Option<String>,
    /// DayZ client install dir override (default: platform discovery).
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
    /// Steam library root override (default: platform discovery).
    #[serde(default)]
    pub steam_dir: Option<PathBuf>,
    /// Launcher executable override. When set, the client is started through
    /// `steam -applaunch` (the historical path, which opens the official
    /// launcher); otherwise it is launched directly under Proton.
    #[serde(default)]
    pub launcher: Option<PathBuf>,
    /// `-connect=…` server target.
    #[serde(skip)]
    pub connect: Option<Connect>,
    /// `-name=<profile>` game profile name.
    #[serde(default)]
    pub profile_name: Option<String>,
    /// Extra game args appended verbatim (e.g. `-nolauncher`, `-world=empty`).
    #[serde(default)]
    pub extra: Vec<String>,
    /// Windowed client size; `None` opens the [`DEFAULT_WINDOW`] size.
    #[serde(default)]
    pub window: Option<WindowSize>,
    /// Whether the client opens in a window; false runs it full screen.
    #[serde(default = "default_windowed")]
    pub windowed: bool,
    /// Bounded capture window in seconds.
    #[serde(default = "default_run_timeout_secs")]
    pub timeout_secs: u64,
    /// Quiet period (seconds) after which a stopped-growing log set is judged
    /// complete. Must exceed the client's longest boot gap (~27s observed).
    #[serde(default = "default_settle_secs")]
    pub settle_secs: u64,
    /// Loose mod source root for file-mapped errors.
    #[serde(default)]
    pub source_root: Option<PathBuf>,
    /// Mods staged for this run only and loaded before `mods`; a `ServerOnly` entry is refused.
    #[serde(default)]
    pub test_mods: Vec<TestModArg>,
    /// Lines the run's logs must show for its verdict to pass (`re:` prefix: a regular expression).
    #[serde(default)]
    pub expect_lines: Vec<String>,
    /// Lines that fail the run's verdict when they appear.
    #[serde(default)]
    pub fail_lines: Vec<String>,
    /// Known script errors the run's verdict ignores.
    #[serde(default)]
    pub allow_errors: Vec<String>,
}

/// Result of a client launch: the captured-new-logs report, the log dir that was
/// watched, and the file-mapped errors extracted from the new logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRunOutcome {
    pub report: ClientRunReport,
    pub log_dir: PathBuf,
    pub errors: Vec<MappedError>,
    /// Set when a stopped client could not be terminated; a warning, not a
    /// failure of the run.
    #[serde(default)]
    pub client_stop_error: Option<String>,
    /// What a run that died left in the install and this one removed.
    #[serde(default)]
    pub cleared: Vec<PathBuf>,
    /// Whether the links were left in the game dir because its game was still running.
    #[serde(default)]
    pub links_kept: bool,
    /// The game install the run staged into.
    #[serde(default)]
    pub game_dir: PathBuf,
    /// How the run's logs judged against the caller's verdict lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
}

/// Resolve the DayZ client install dir: explicit override, else platform
/// discovery. Fails loudly when the box has no DayZ client install.
pub fn resolve_game_dir(game_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    match game_dir {
        Some(d) => Ok(d.to_path_buf()),
        None => modfoundry_platform::find_dayz_installs()
            .0
            .ok_or_else(|| anyhow::anyhow!("DayZ client install not found; pass --game-dir")),
    }
}

/// A mod exposed to a launch: the `-mod=` argument, and the link and target that stage it.
struct ModLink {
    arg: PathBuf,
    link: PathBuf,
    target: PathBuf,
}

/// The link that exposes `mod_dir` under `install`, named after its folder. A game-dir-relative
/// `@name` is the one form both the server and the client load: the client runs under Proton, which
/// maps drives, and the official launcher only assembles Steam subscriptions.
#[cfg(unix)]
fn mod_link(install: &Path, mod_dir: &Path) -> anyhow::Result<ModLink> {
    let mod_dir = std::path::absolute(mod_dir)?;
    let name = mod_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("mod dir {} has no name", mod_dir.display()))?
        .to_string_lossy()
        .into_owned();
    named_link(install, &mod_dir, &name)
}

#[cfg(not(unix))]
fn mod_link(_install: &Path, _mod_dir: &Path) -> anyhow::Result<ModLink> {
    anyhow::bail!("linking a mod into the game install is only supported on unix hosts")
}

/// The `@<name>` link that exposes `mod_dir` under `install`.
#[cfg(unix)]
fn named_link(install: &Path, mod_dir: &Path, name: &str) -> anyhow::Result<ModLink> {
    let link_name = at_link_name(name);
    Ok(ModLink {
        arg: PathBuf::from(&link_name),
        link: install.join(&link_name),
        // A relative target resolves against the link's own directory (the
        // install), so the link must carry an absolute one.
        target: std::path::absolute(mod_dir)?,
    })
}

#[cfg(not(unix))]
fn named_link(_install: &Path, _mod_dir: &Path, _name: &str) -> anyhow::Result<ModLink> {
    anyhow::bail!("staging a mod outside the server install is only supported on unix hosts")
}

/// A test mod linked under a built mod's name would take that mod's link.
fn refuse_build_names(builds: &[PathBuf], test_mods: &[TestModArg]) -> anyhow::Result<()> {
    for build in builds {
        let Some(name) = build.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = name.trim_start_matches('@');
        if test_mods
            .iter()
            .any(|test_mod| test_mod.link_name.trim_start_matches('@') == name)
        {
            anyhow::bail!("test mod @{name} has the build's name; rename its link");
        }
    }
    Ok(())
}

/// `@name`, keeping an `@` the caller already wrote.
#[cfg(unix)]
pub(crate) fn at_link_name(base: &str) -> String {
    if base.starts_with('@') {
        base.to_string()
    } else {
        format!("@{base}")
    }
}

/// Whether the real folder at `link` is `target` itself, so the mod needs no link.
#[cfg(unix)]
pub(crate) fn in_place(link: &Path, target: &Path) -> bool {
    match (std::fs::canonicalize(link), std::fs::canonicalize(target)) {
        (Ok(link), Ok(target)) => link == target,
        _ => false,
    }
}

/// The step that makes `link -> target`, or none when the link already points there or the folder
/// is already in place.
#[cfg(unix)]
fn link_step(link: &Path, target: &Path, gone: &[PathBuf]) -> anyhow::Result<Option<StageStep>> {
    let step = |relink: bool| {
        let link = link.to_path_buf();
        let target = target.to_path_buf();
        Some(if relink {
            StageStep::Relink { link, target }
        } else {
            StageStep::Link { link, target }
        })
    };
    if absent(gone, link) {
        return Ok(step(false));
    }
    // `exists()` follows the link, so a dangling `@name` (target wiped) reads
    // false and a plain `symlink` would then fail with AlreadyExists. Inspect
    // the link itself and re-point a dangling one; a live link to another
    // directory is the user's.
    match std::fs::symlink_metadata(link) {
        Ok(meta) if !meta.file_type().is_symlink() => {
            if in_place(link, target) {
                return Ok(None);
            }
            anyhow::bail!(
                "{} exists and is not a symlink; remove it to stage {}",
                link.display(),
                target.display()
            )
        }
        Ok(_) if std::fs::read_link(link).ok().as_deref() == Some(target) => Ok(None),
        Ok(_) if link.exists() => anyhow::bail!(
            "{} already exists and does not point at {}; remove it to stage the mod",
            link.display(),
            target.display()
        ),
        Ok(_) => Ok(step(true)),
        Err(_) => Ok(step(false)),
    }
}

#[cfg(not(unix))]
fn link_step(_link: &Path, _target: &Path, _gone: &[PathBuf]) -> anyhow::Result<Option<StageStep>> {
    anyhow::bail!("staging a mod outside the server install is only supported on unix hosts")
}

/// Map a platform-resolved Proton launch into the diag crate's launch
/// description (keeps `modfoundry-diag` free of a platform dependency).
pub(crate) fn launch_command(pl: modfoundry_platform::ProtonLaunch) -> LaunchCommand {
    LaunchCommand {
        program: pl.program,
        args: pl.args,
        cwd: Some(pl.cwd),
        env: pl.env,
        env_remove: pl.env_remove,
    }
}

/// Describe the Proton command that starts `exe` with `argv`.
pub(crate) fn proton_command(
    steam_root: &Path,
    exe: &Path,
    argv: &[String],
) -> anyhow::Result<LaunchCommand> {
    modfoundry_platform::proton_launch(steam_root, exe, argv)
        .map(launch_command)
        .map_err(|e| anyhow::anyhow!("{e} (steam root {})", steam_root.display()))
}

/// The DayZ client's log dir inside the Proton prefix (where the client writes,
/// unlike the server's `-profiles`); the game creates it on its first run.
pub fn client_log_dir(steam_root: &Path) -> anyhow::Result<PathBuf> {
    let steam = modfoundry_platform::SteamInstall {
        root: steam_root.to_path_buf(),
        steamapps: steam_root.join("steamapps"),
    };
    let pfx = modfoundry_platform::proton_prefix(&steam, modfoundry_platform::DAYZ_APP_ID)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no Proton prefix for DayZ (app {}) under {}; run the client once under Proton first",
                modfoundry_platform::DAYZ_APP_ID,
                steam_root.display()
            )
        })?;
    modfoundry_platform::proton_client_log_dir(&pfx)
        .ok_or_else(|| anyhow::anyhow!("DayZ prefix {} is not a folder", pfx.display()))
}

pub(crate) fn client_log_dirs(steam_dir: Option<&Path>) -> Vec<PathBuf> {
    resolve_steam_root(steam_dir)
        .and_then(|steam| client_log_dir(&steam))
        .ok()
        .into_iter()
        .collect()
}

/// Why a live client run ended after its log capture settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientEnd {
    Stopped,
    TimedOut,
    Exited,
}

/// Wait until the stop signal is set (invoking `on_stop` once), the deadline
/// passes, or `running` turns false.
pub(crate) fn wait_for_client_end(
    stop: &StopSignal,
    deadline: Instant,
    running: impl Fn() -> bool,
    mut sleep: impl FnMut(),
    on_stop: impl FnOnce(),
) -> ClientEnd {
    loop {
        if stop.is_stopped() {
            on_stop();
            return ClientEnd::Stopped;
        }
        if Instant::now() >= deadline {
            return ClientEnd::TimedOut;
        }
        if !running() {
            return ClientEnd::Exited;
        }
        sleep();
    }
}

/// Launch the DayZ client under Proton with local mods staged and auto-connect,
/// capture the logs it writes into the Proton prefix, then map errors back to
/// the mod's sources. Mirrors [`run_server`] for the client half of the test
/// loop.
pub fn run_client(args: &RunClientArgs, stop: &StopSignal) -> anyhow::Result<ClientRunOutcome> {
    run_client_with(args, stop, &modfoundry_platform::dayz_client_running)
}

/// [`run_client`] with the "is a game already running" check supplied by the caller, so tests
/// never depend on the host's process table.
pub(crate) fn run_client_with(
    args: &RunClientArgs,
    stop: &StopSignal,
    client_running: &dyn Fn() -> bool,
) -> anyhow::Result<ClientRunOutcome> {
    let known = KnownErrors::load(None, args.source_root.as_deref())?;
    let rules = verdict_rules(
        &args.expect_lines,
        &args.fail_lines,
        &args.allow_errors,
        &known,
        || client_verdict_mods(args),
    )?;
    let game_dir = resolve_game_dir(args.game_dir.as_deref())?;
    let game_up = client_running();
    // Before clearing: a client a dead run started may still be using its links.
    if args.launcher.is_none() && game_up {
        anyhow::bail!("a DayZ client is already running; stop it first");
    }
    // A launcher's game may already be up, and it loads the dead run's links too.
    let cleared = if args.launcher.is_some() && game_up {
        Vec::new()
    } else {
        clear_dead(&game_dir)?
    };
    let plan = plan_client(args, &[])?;
    let mut staged = Staged::in_install(&game_dir);
    crate::launch_plan::apply(&plan.steps, &mut staged)?;
    let command = plan
        .launches
        .first()
        .ok_or_else(|| anyhow::anyhow!("the plan has no launch"))?;

    let steam_dir = resolve_steam_root(args.steam_dir.as_deref())?;
    let log_dir = client_log_dir(&steam_dir)?;

    let launched = Instant::now();
    let wait = modfoundry_diag::ClientWait::new(args.timeout_secs, args.settle_secs);
    let watch = VerdictWatch::spawn(
        &rules,
        std::slice::from_ref(&log_dir),
        stop,
        Duration::from_secs(args.timeout_secs),
    );
    let mut report = match modfoundry_diag::capture_client(&command.command, &log_dir, &wait, stop)
    {
        Ok(report) => report,
        Err(e) => {
            // Killing the captured child ends only the runtime wrapper. A launcher run
            // never checked for a running client, so the one out there may not be ours.
            if args.launcher.is_none() {
                let _ = stop_client_error();
            }
            return Err(e);
        }
    };

    // A stop closes the client the run launched; failing to do so is a warning,
    // not a failed capture.
    let mut client_stop_error = None;
    if !report.stopped && !report.timed_out && args.launcher.is_none() {
        // The capture settles while the game is still running, so keep the run
        // live until an explicit stop, the bounded window, or the game exits.
        // With a launcher override there is no game to poll.
        let deadline = launched + Duration::from_secs(args.timeout_secs);
        match wait_for_client_end(
            stop,
            deadline,
            client_running,
            || std::thread::sleep(CLIENT_POLL_INTERVAL),
            || client_stop_error = stop_client_error(),
        ) {
            ClientEnd::Stopped => report.stopped = true,
            ClientEnd::TimedOut => report.timed_out = true,
            ClientEnd::Exited => {}
        }
        let logs = modfoundry_diag::find_logs(&log_dir)
            .into_iter()
            .filter(|p| !report.before.contains(p))
            .collect();
        report.logs = logs;
    } else if report.stopped {
        client_stop_error = stop_client_error();
    }
    // A game still running at the window loads its mods from these links, so the sweep
    // takes them once the game has ended.
    let links_kept = report.timed_out && args.launcher.is_none() && client_running();
    if links_kept {
        staged.keep_for(modfoundry_platform::dayz_client_pid());
    }
    drop(watch);
    let mut errors =
        errors_from_logs(&report.logs, &LogStates::new(), args.source_root.as_deref())?;
    known.mark(&mut errors);
    let verdict = judge_logs(&report.logs, &LogStates::new(), &rules)?;
    Ok(ClientRunOutcome {
        report,
        log_dir,
        errors,
        client_stop_error,
        cleared,
        links_kept,
        game_dir,
        verdict,
    })
}

/// Works out every check, path and argument a client run uses, without writing anything. Paths in
/// `gone` are treated as absent: they are a dead run's leftovers, which the launch removes first.
pub(crate) fn plan_client(args: &RunClientArgs, gone: &[PathBuf]) -> anyhow::Result<RunPlan> {
    for m in &args.mods {
        require_signed(m)?;
    }
    check_test_mods(&args.test_mods)?;
    refuse_build_names(&args.mods, &args.test_mods)?;
    if let Some(test_mod) = args
        .test_mods
        .iter()
        .find(|test_mod| test_mod.role == TestModRole::ServerOnly)
    {
        anyhow::bail!(
            "test mod {} is server only and cannot load on the client",
            test_mod.link_name
        );
    }
    let game_dir = resolve_game_dir(args.game_dir.as_deref())?;
    let steam_dir = resolve_steam_root(args.steam_dir.as_deref())?;

    let mut links = Vec::new();
    for test_mod in &args.test_mods {
        links.push(named_link(&game_dir, &test_mod.path, &test_mod.link_name)?);
    }
    for m in &args.mods {
        links.push(mod_link(&game_dir, m)?);
    }
    let mut steps = Vec::new();
    let mut planned: HashMap<&Path, &Path> = HashMap::new();
    for link in &links {
        match planned.insert(&link.link, &link.target) {
            None => steps.extend(link_step(&link.link, &link.target, gone)?),
            Some(target) if target == link.target => {}
            Some(_) => anyhow::bail!(
                "{} already exists and does not point at {}; remove it to stage the mod",
                link.link.display(),
                link.target.display()
            ),
        }
    }
    let mods = links
        .iter()
        .map(|link| link.arg.to_string_lossy().into_owned())
        .collect();

    let mut extra = if args.windowed {
        window_args(args.window, &args.extra)
    } else {
        Vec::new()
    };
    extra.extend(args.extra.iter().cloned());
    let profile = ClientProfile {
        app_id: modfoundry_platform::DAYZ_APP_ID,
        mods,
        connect: args.connect.clone(),
        profile_name: args.profile_name.clone(),
        extra,
        flags: LogFlags {
            do_logs: true,
            ..Default::default()
        },
    };
    let command = match &args.launcher {
        Some(launcher) => LaunchCommand {
            program: launcher.clone(),
            args: profile
                .steam_argv()
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        },
        None => {
            let exe = modfoundry_platform::find_dayz_exe(&game_dir).ok_or_else(|| {
                anyhow::anyhow!(
                    "no DayZ client executable (DayZ_x64.exe or DayZ_BE.exe) in {}",
                    game_dir.display()
                )
            })?;
            proton_command(&steam_dir, &exe, &profile.argv())?
        }
    };
    Ok(RunPlan {
        launches: vec![PlannedLaunch {
            role: LaunchRole::Client,
            command,
        }],
        steps,
        project: Vec::new(),
        leftovers: gone.to_vec(),
    })
}

/// Stop the DayZ client a run launched, mapping a failure to its message.
fn stop_client_error() -> Option<String> {
    modfoundry_platform::stop_dayz_client()
        .err()
        .map(|e| e.to_string())
}

#[cfg(test)]
mod run_control_tests {
    use super::RunControl;

    #[test]
    fn a_stop_reaches_every_job_registered_under_one_run_id() {
        let control = RunControl::new();
        let server = control.begin("run-1");
        let client = control.begin("run-1");
        assert!(control.stop("run-1"));
        assert!(server.is_stopped());
        assert!(client.is_stopped());
    }

    #[test]
    fn a_finished_job_leaves_its_sibling_stoppable() {
        let control = RunControl::new();
        let server = control.begin("run-1");
        let client = control.begin("run-1");
        control.end("run-1", &client);
        assert!(control.stop("run-1"));
        assert!(server.is_stopped());
        assert!(!client.is_stopped());
    }

    #[test]
    fn a_panicking_body_leaves_nothing_to_stop() {
        let control = RunControl::new();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.scoped(Some("run-1"), |_| panic!("launch failed"));
        }));
        assert!(caught.is_err(), "the body's panic must reach the caller");
        assert!(!control.stop("run-1"));
    }

    #[test]
    fn a_run_with_no_live_jobs_cannot_be_stopped() {
        let control = RunControl::new();
        let only = control.begin("run-1");
        control.end("run-1", &only);
        assert!(!control.stop("run-1"));
    }
}

#[cfg(test)]
mod scratch_dir_tests {
    use super::scratch_profiles_dir;

    #[test]
    fn scratch_profiles_dirs_differ_within_a_second() {
        let first = scratch_profiles_dir();
        let second = scratch_profiles_dir();
        assert_ne!(
            first, second,
            "two runs in one second must not share a capture dir"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn report_cleared_writes_only_on_a_thread_marked_detached() {
        let swept = [PathBuf::from("/server/keys/Stale.bikey")];
        let expected = format!("{}\n", cleared_line(&swept[0]));

        let mut foreground: Vec<u8> = Vec::new();
        report_cleared(&mut foreground, &swept);
        assert!(
            foreground.is_empty(),
            "a foreground run's caller prints the outcome's cleared paths"
        );

        let written = std::thread::spawn(move || {
            mark_detached();
            let mut out: Vec<u8> = Vec::new();
            report_cleared(&mut out, &swept);
            String::from_utf8(out).unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(written, expected);
    }

    #[test]
    fn the_detached_mark_belongs_to_the_launch_thread_alone() {
        assert!(!detached(), "a launching thread runs foreground launches");

        let marked = std::thread::spawn(|| {
            mark_detached();
            detached()
        })
        .join()
        .unwrap();

        assert!(marked, "the spawned launch thread must be marked detached");
        assert!(
            !detached(),
            "the mark must not reach the caller's own thread"
        );
    }

    #[test]
    fn unsigned_pbos_lists_pbos_without_a_signature() {
        let mod_dir = tmp("unsigned-pbos");
        let addons = mod_dir.join("Addons");
        std::fs::create_dir_all(&addons).unwrap();
        for file in ["a.pbo", "b.pbo", "b.pbo.K.bisign", "notes.txt"] {
            std::fs::write(addons.join(file), b"").unwrap();
        }
        assert_eq!(unsigned_pbos(&mod_dir), ["a.pbo"]);
        assert!(unsigned_pbos(&tmp("unsigned-pbos-no-addons")).is_empty());
    }

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-runops-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn relative(path: &Path) -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components() {
            rel.push("..");
        }
        rel.push(path.strip_prefix("/").unwrap());
        rel
    }

    #[test]
    fn a_link_that_already_points_at_the_mod_needs_no_step() {
        let game_dir = tmp("game");
        let mod_dir = tmp("src").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();
        let link = mod_link(&game_dir, &mod_dir).unwrap();
        std::os::unix::fs::symlink(&link.target, &link.link).unwrap();

        assert_eq!(link_step(&link.link, &link.target, &[]).unwrap(), None);

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    }

    #[test]
    fn a_folder_already_at_the_link_path_needs_no_step() {
        let game_dir = tmp("game");
        let mod_dir = game_dir.join("@A");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(mod_dir.join("marker.txt"), b"here").unwrap();
        let link = named_link(&game_dir, &mod_dir, "A").unwrap();
        assert_eq!(link.link, mod_dir, "the link path is the mod's own folder");
        std::fs::create_dir_all(game_dir.join("sub")).unwrap();
        let detour = game_dir.join("sub/../@A");

        for target in [link.target.clone(), detour] {
            assert_eq!(link_step(&link.link, &target, &[]).unwrap(), None);
            assert!(
                mod_dir.join("marker.txt").is_file(),
                "the folder must be left alone"
            );
        }

        let _ = std::fs::remove_dir_all(&game_dir);
    }

    #[test]
    fn a_different_real_folder_at_the_link_path_is_still_refused_and_kept() {
        let game_dir = tmp("game");
        let occupied = game_dir.join("@A");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("marker.txt"), b"mine").unwrap();
        let mod_dir = tmp("src").join("A");
        std::fs::create_dir_all(&mod_dir).unwrap();

        let err = link_step(&occupied, &mod_dir, &[]).unwrap_err().to_string();

        assert!(
            err.contains(&occupied.display().to_string()),
            "unhelpful error: {err}"
        );
        assert!(
            err.contains(&mod_dir.display().to_string()),
            "the error must name the target too: {err}"
        );
        assert!(
            occupied.join("marker.txt").is_file(),
            "the user's folder must be left alone"
        );

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    }

    #[test]
    fn a_relative_mod_dir_is_linked_by_its_absolute_path() {
        let game_dir = tmp("game");
        let mod_dir = tmp("src").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();

        let link = mod_link(&game_dir, &relative(&mod_dir)).unwrap();

        assert_eq!(link.arg, PathBuf::from("@FieldKit"));
        assert!(
            link.target.is_absolute(),
            "relative link target: {:?}",
            link.target
        );
        assert_eq!(
            std::fs::canonicalize(&link.target).unwrap(),
            std::fs::canonicalize(&mod_dir).unwrap()
        );

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    }

    #[test]
    fn a_live_link_to_another_mod_is_refused_and_kept() {
        let game_dir = tmp("game");
        let mod_dir = tmp("src").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();
        let elsewhere = tmp("elsewhere").join("@FieldKit");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let link = game_dir.join("@FieldKit");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        let err = link_step(&link, &mod_dir, &[]).unwrap_err().to_string();

        assert!(
            err.contains(&link.display().to_string()),
            "unhelpful error: {err}"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), elsewhere);

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
    }

    #[test]
    fn a_dangling_link_is_repointed_and_removed_on_drop() {
        let game_dir = tmp("game");
        let mod_dir = tmp("src").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();
        let link = mod_link(&game_dir, &mod_dir).unwrap();
        std::os::unix::fs::symlink(game_dir.join("wiped"), &link.link).unwrap();

        let mut staged = Staged::in_install(&game_dir);
        let step = link_step(&link.link, &link.target, &[]).unwrap().unwrap();
        assert_eq!(
            step,
            StageStep::Relink {
                link: link.link.clone(),
                target: link.target.clone(),
            }
        );
        crate::launch_plan::apply(std::slice::from_ref(&step), &mut staged).unwrap();
        assert_eq!(std::fs::read_link(&link.link).unwrap(), link.target);

        drop(staged);
        assert!(
            link.link.symlink_metadata().is_err(),
            "the repointed link must be removed"
        );

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    }

    fn write_exec(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    /// A fake Steam root: a stub Proton + Steam Linux Runtime that record their
    /// argv/cwd/env, a Proton prefix with the client log dir, and a game dir.
    struct SteamFixture {
        root: PathBuf,
        game_dir: PathBuf,
        log_dir: PathBuf,
        record: PathBuf,
    }

    fn steam_fixture(label: &str) -> SteamFixture {
        let root = tmp(label);
        let common = root.join("steamapps/common");
        let game_dir = common.join("DayZ");
        let log_dir =
            root.join("steamapps/compatdata/221100/pfx/drive_c/users/steamuser/AppData/Local/DayZ");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::create_dir_all(&log_dir).unwrap();
        let record = root.join("record.txt");
        let body = format!(
            "#!/bin/sh\n{{\nfor a in \"$@\"; do printf 'argv:%s\\n' \"$a\"; done\n\
             printf 'cwd:%s\\n' \"$(pwd)\"\nprintf 'SteamAppId:%s\\n' \"$SteamAppId\"\n\
             }} > \"{rec}\"\nprintf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > \"{log}/DayZ_x64_x.RPT\"\n",
            rec = record.display(),
            log = log_dir.display(),
        );
        let proton = common.join("Proton - Experimental");
        std::fs::create_dir_all(&proton).unwrap();
        write_exec(&proton.join("proton"), &body);
        let runtime = common.join("SteamLinuxRuntime_sniper");
        std::fs::create_dir_all(&runtime).unwrap();
        write_exec(&runtime.join("run"), &body);
        SteamFixture {
            root,
            game_dir,
            log_dir,
            record,
        }
    }

    fn recorded_argv(record: &Path) -> Vec<String> {
        std::fs::read_to_string(record)
            .unwrap()
            .lines()
            .filter_map(|l| l.strip_prefix("argv:"))
            .map(str::to_string)
            .collect()
    }

    fn record_launcher(fx: &SteamFixture) -> PathBuf {
        let launcher = fx.root.join("launcher");
        let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf 'argv:%s\\n' \"$a\"; done > \"{}\"\n",
            fx.record.display()
        );
        write_exec(&launcher, &body);
        launcher
    }

    fn client_args(fx: &SteamFixture, mods: Vec<PathBuf>) -> RunClientArgs {
        RunClientArgs {
            mods,
            set: None,
            game_dir: Some(fx.game_dir.clone()),
            steam_dir: Some(fx.root.clone()),
            launcher: None,
            connect: Some(Connect {
                host: "10.0.0.5".into(),
                game_port: 2302,
                query_port: None,
            }),
            profile_name: None,
            extra: Vec::new(),
            window: None,
            windowed: true,
            timeout_secs: 15,
            settle_secs: 1,
            source_root: None,
            test_mods: Vec::new(),
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        }
    }

    #[test]
    fn client_window_args_default_to_1280x720_before_dologs() {
        let profile = ClientProfile {
            app_id: 221100,
            mods: Vec::new(),
            connect: Some(Connect {
                host: "10.0.0.5".into(),
                game_port: 2302,
                query_port: None,
            }),
            profile_name: None,
            extra: window_args(None, &[]),
            flags: LogFlags {
                do_logs: true,
                ..Default::default()
            },
        };
        let argv = profile.argv();
        let win = argv.iter().position(|a| a == "-window").unwrap();
        let x = argv.iter().position(|a| a == "-x=1280").unwrap();
        let y = argv.iter().position(|a| a == "-y=720").unwrap();
        let dl = argv.iter().position(|a| a == "-dologs").unwrap();
        assert!(win < x && x < y && y < dl, "wrong order: {argv:?}");
    }

    #[test]
    fn an_explicit_window_size_replaces_the_default() {
        let args = window_args(
            Some(WindowSize {
                width: 1920,
                height: 1080,
            }),
            &[],
        );
        assert_eq!(args, vec!["-window", "-x=1920", "-y=1080"]);
    }

    #[test]
    fn caller_supplied_window_args_suppress_the_default() {
        let extra = vec!["-window".to_string(), "-x=800".to_string()];
        assert!(window_args(None, &extra).is_empty());
        assert!(window_args(
            Some(WindowSize {
                width: 1920,
                height: 1080
            }),
            &extra
        )
        .is_empty());

        let mut combined = window_args(None, &extra);
        combined.extend(extra.iter().cloned());
        assert_eq!(
            combined.iter().filter(|a| *a == "-window").count(),
            1,
            "the caller's `-window` must not be duplicated: {combined:?}"
        );
    }

    #[test]
    fn wait_for_client_end_stops_and_calls_the_callback_once() {
        let stop = StopSignal::new();
        stop.stop();
        let calls = Cell::new(0);
        let end = wait_for_client_end(
            &stop,
            Instant::now(),
            || true,
            || {},
            || calls.set(calls.get() + 1),
        );
        assert_eq!(end, ClientEnd::Stopped);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn wait_for_client_end_times_out_without_stopping() {
        let calls = Cell::new(0);
        let end = wait_for_client_end(
            &StopSignal::new(),
            Instant::now(),
            || true,
            || {},
            || calls.set(calls.get() + 1),
        );
        assert_eq!(end, ClientEnd::TimedOut);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn wait_for_client_end_reports_an_exit() {
        let calls = Cell::new(0);
        let end = wait_for_client_end(
            &StopSignal::new(),
            Instant::now() + Duration::from_secs(60),
            || false,
            || {},
            || calls.set(calls.get() + 1),
        );
        assert_eq!(end, ClientEnd::Exited);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn run_client_without_a_launcher_override_runs_proton_directly() {
        let fx = steam_fixture("direct");
        std::fs::write(fx.game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let mod_root = tmp("direct-mod");
        let mod_dir = mod_root.join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();

        let outcome = run_client_with(
            &client_args(&fx, vec![mod_dir.clone()]),
            &StopSignal::new(),
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.log_dir, fx.log_dir);

        let argv = recorded_argv(&fx.record);
        let proton = fx
            .root
            .join("steamapps/common/Proton - Experimental/proton");
        let runtime = fx
            .root
            .join("steamapps/common/SteamLinuxRuntime_sniper/run");
        let exe = fx.game_dir.join("DayZ_x64.exe");
        assert_eq!(argv[0], "--");
        assert_eq!(argv[1], proton.to_str().unwrap());
        assert_eq!(argv[2], "run");
        assert_eq!(argv[3], exe.to_str().unwrap());
        assert_eq!(argv[4], "-mod=@FieldKit");
        assert_eq!(argv[5], "-connect=10.0.0.5:2302");
        assert_eq!(argv[6], "-window");
        assert_eq!(argv[7], "-x=1280");
        assert_eq!(argv[8], "-y=720");
        assert_eq!(argv[9], "-dologs");
        assert_eq!(
            argv.len(),
            10,
            "the runtime command carries `--` + proton + run + exe + game args, got {argv:?}"
        );
        // The runtime `run` stub is the executed program; the Proton binary is
        // only passed to it as an argument.
        assert!(runtime.is_file());

        let text = std::fs::read_to_string(&fx.record).unwrap();
        let cwd = std::fs::canonicalize(&fx.game_dir).unwrap();
        assert!(
            text.contains(&format!("cwd:{}", cwd.display())),
            "cwd was not the game dir: {text}"
        );
        assert!(text.contains("SteamAppId:221100"), "{text}");

        let _ = std::fs::remove_dir_all(&fx.root);
        let _ = std::fs::remove_dir_all(&mod_root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn a_running_client_refuses_a_direct_launch_but_not_a_launcher_override() {
        let fx = steam_fixture("running-guard");
        std::fs::write(fx.game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let mod_dir = tmp("running-guard-mod");
        let err = run_client_with(
            &client_args(&fx, vec![mod_dir.clone()]),
            &StopSignal::new(),
            &|| true,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already running"), "{err}");

        let mut with_override = client_args(&fx, vec![mod_dir]);
        with_override.launcher = Some(PathBuf::from("/bin/true"));
        with_override.timeout_secs = 1;
        with_override.settle_secs = 0;
        assert!(
            !run_client_with(&with_override, &StopSignal::new(), &|| true)
                .err()
                .is_some_and(|e| e.to_string().contains("already running"))
        );
    }

    #[test]
    fn run_client_without_a_dayz_exe_names_the_missing_binary() {
        let fx = steam_fixture("noexe");
        let err = run_client_with(&client_args(&fx, Vec::new()), &StopSignal::new(), &|| false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("DayZ_x64.exe"), "unhelpful error: {err}");
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn run_client_refuses_an_unsigned_mod_before_linking_it() {
        let fx = steam_fixture("unsigned-client");
        let mod_dir = fx.root.join("@Mod");
        std::fs::create_dir_all(mod_dir.join("addons")).unwrap();
        std::fs::write(mod_dir.join("addons/Mod.pbo"), b"").unwrap();
        let err = run_client_with(
            &client_args(&fx, vec![mod_dir]),
            &StopSignal::new(),
            &|| false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("Mod.pbo") && err.contains("Settings › Signing"),
            "unhelpful error: {err}"
        );
        assert!(!fx.game_dir.join("@Mod").exists(), "the mod was linked");
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn run_client_without_proton_names_the_missing_piece() {
        let root = tmp("noproton");
        let game_dir = root.join("steamapps/common/DayZ");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let args = RunClientArgs {
            game_dir: Some(game_dir),
            steam_dir: Some(root.clone()),
            ..client_args_no_steam()
        };
        let err = run_client_with(&args, &StopSignal::new(), &|| false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Proton"), "unhelpful error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_client_without_a_steam_runtime_names_the_missing_piece() {
        let root = tmp("noruntime");
        let common = root.join("steamapps/common");
        let game_dir = common.join("DayZ");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let proton = common.join("Proton - Experimental");
        std::fs::create_dir_all(&proton).unwrap();
        write_exec(&proton.join("proton"), "#!/bin/sh\nexit 0\n");
        let args = RunClientArgs {
            game_dir: Some(game_dir),
            steam_dir: Some(root.clone()),
            ..client_args_no_steam()
        };
        let err = run_client_with(&args, &StopSignal::new(), &|| false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Runtime"), "unhelpful error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn client_args_no_steam() -> RunClientArgs {
        RunClientArgs {
            mods: Vec::new(),
            set: None,
            game_dir: None,
            steam_dir: None,
            launcher: None,
            connect: None,
            profile_name: None,
            extra: Vec::new(),
            window: None,
            windowed: true,
            timeout_secs: 5,
            settle_secs: 1,
            source_root: None,
            test_mods: Vec::new(),
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        }
    }

    /// A fake DayZServer install whose stub records its argv into a file beside the install.
    fn server_stub(label: &str) -> (PathBuf, PathBuf) {
        let root = tmp(label);
        let server_dir = root.join("DayZServer");
        std::fs::create_dir_all(&server_dir).unwrap();
        let record = root.join("record.txt");
        let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf 'argv:%s\\n' \"$a\"; done > \"{}\"\nls keys 2>/dev/null | sed 's/^/key:/' >> \"{}\"\n",
            record.display(),
            record.display()
        );
        write_exec(&server_dir.join("DayZServer"), &body);
        (server_dir, record)
    }

    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn server_args(mod_dir: PathBuf, server_dir: &Path) -> RunServerArgs {
        RunServerArgs {
            mod_dir,
            server_dir: Some(server_dir.to_path_buf()),
            config: None,
            set: None,
            set_config: None,
            port: None,
            profiles_dir: None,
            timeout_secs: 5,
            source_root: None,
            run_id: None,
            test_mods: Vec::new(),
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        }
    }

    #[test]
    fn a_plan_names_what_a_run_stages_and_launches_the_same_command() {
        use crate::launch_plan::{LaunchRole, StageStep};

        let (server_dir, record) = server_stub("plan");
        let cfg = "hostname = \"x\";\n";
        std::fs::write(server_dir.join(SERVER_CONFIG), cfg).unwrap();
        let src = tmp("plan-src");
        let built = src.join("@Built");
        std::fs::create_dir_all(built.join("keys")).unwrap();
        std::fs::write(built.join("keys/Built.bikey"), b"key").unwrap();
        let test_mod = src.join("Other");
        std::fs::create_dir_all(test_mod.join("Keys")).unwrap();
        std::fs::write(test_mod.join("Keys/Other.bikey"), b"other").unwrap();
        let root = tmp("plan-root");
        let args = RunServerArgs {
            source_root: Some(root.clone()),
            test_mods: vec![TestModArg {
                path: test_mod.clone(),
                link_name: "Other".into(),
                role: TestModRole::ServerOnly,
            }],
            ..server_args(built.clone(), &server_dir)
        };
        let before = listing(&server_dir);

        let plan = plan_server(&args, &[]).unwrap();

        assert_eq!(listing(&server_dir), before, "a plan writes nothing");
        assert!(
            !root.join("server").exists(),
            "a plan creates no project folders"
        );
        assert_eq!(plan.launches.len(), 1);
        assert_eq!(plan.launches[0].role, LaunchRole::Server);
        let build_link = StageStep::Link {
            link: server_dir.join("@Built"),
            target: std::path::absolute(&built).unwrap(),
        };
        assert!(plan.steps.contains(&build_link), "{:?}", plan.steps);
        for (from, to) in [
            (
                built.join("keys/Built.bikey"),
                server_dir.join("keys/Built.bikey"),
            ),
            (
                test_mod.join("Keys/Other.bikey"),
                server_dir.join("keys/Other.bikey"),
            ),
        ] {
            assert!(
                plan.steps.contains(&StageStep::Copy { from, to }),
                "{:?}",
                plan.steps
            );
        }
        let staged_cfg = server_dir.join(DIAG_CONFIG_NAME);
        assert!(
            plan.steps.iter().any(|step| matches!(
                step,
                StageStep::Write { path, contents, bytes }
                    if path == &staged_cfg && contents.as_slice() == cfg.as_bytes() && *bytes == cfg.len()
            )),
            "{:?}",
            plan.steps
        );
        let kept_cfg = root.join("server").join(SERVER_CONFIG);
        assert!(
            plan.project.iter().any(|step| matches!(
                step,
                StageStep::Write { path, contents, .. }
                    if path == &kept_cfg && contents.as_slice() == cfg.as_bytes()
            )),
            "{:?}",
            plan.project
        );
        let profiles = root.join("server/profiles");
        assert!(
            plan.project.contains(&StageStep::CreateDir {
                path: profiles.clone()
            }),
            "{:?}",
            plan.project
        );

        let planned: Vec<String> = plan.launches[0]
            .command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        run_server(&args, &StopSignal::new()).unwrap();
        assert_eq!(recorded_argv(&record), planned);

        for dir in [server_dir.parent().unwrap(), &src, &root] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn two_mods_that_ship_the_same_key_plan_one_copy() {
        use crate::launch_plan::StageStep;

        let (server_dir, record) = server_stub("plan-same-key");
        let src = tmp("plan-same-key-src");
        let built = src.join("@Built");
        let test_mod = src.join("Other");
        for keys in [built.join("keys"), test_mod.join("keys")] {
            std::fs::create_dir_all(&keys).unwrap();
            std::fs::write(keys.join("Author.bikey"), b"key").unwrap();
        }
        let args = RunServerArgs {
            test_mods: vec![TestModArg {
                path: test_mod,
                link_name: "Other".into(),
                role: TestModRole::Both,
            }],
            ..server_args(built, &server_dir)
        };

        let plan = plan_server(&args, &[]).unwrap();
        let copies = plan
            .steps
            .iter()
            .filter(|step| matches!(step, StageStep::Copy { .. }))
            .count();
        assert_eq!(copies, 1, "{:?}", plan.steps);

        run_server(&args, &StopSignal::new()).unwrap();
        assert!(
            std::fs::read_to_string(&record)
                .unwrap()
                .contains("key:Author.bikey"),
            "the server must see the one key both mods ship"
        );

        let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn a_project_config_in_another_case_is_staged_without_a_new_copy() {
        use crate::launch_plan::StageStep;

        let (server_dir, _record) = server_stub("plan-cfg-case");
        std::fs::write(server_dir.join(SERVER_CONFIG), "hostname = \"retail\";\n").unwrap();
        let src = tmp("plan-cfg-case-src");
        std::fs::create_dir_all(src.join("server")).unwrap();
        std::fs::write(src.join("server/serverdz.cfg"), "hostname = \"lower\";\n").unwrap();
        let args = RunServerArgs {
            source_root: Some(src.clone()),
            ..server_args(src.join("@Built"), &server_dir)
        };

        let plan = plan_server(&args, &[]).unwrap();

        assert!(
            !plan
                .project
                .iter()
                .any(|step| matches!(step, StageStep::Write { .. })),
            "{:?}",
            plan.project
        );
        assert!(
            plan.steps.iter().any(|step| matches!(
                step,
                StageStep::Write { path, contents, .. }
                    if path.ends_with(DIAG_CONFIG_NAME) && contents == b"hostname = \"lower\";\n"
            )),
            "{:?}",
            plan.steps
        );
        let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn a_staged_config_in_the_install_refuses_the_run_unless_a_dead_run_left_it() {
        use crate::launch_plan::StageStep;
        use modfoundry_platform::stage::journal_path;

        #[derive(Clone, Copy)]
        enum State {
            Missing,
            File,
            FileFromADeadRun,
            Folder,
        }

        let cases = [
            ("nothing there", State::Missing, None),
            (
                "a file there",
                State::File,
                Some(["serverDZ_modfoundry.cfg", "another run"].as_slice()),
            ),
            ("a file a dead run left", State::FileFromADeadRun, None),
            (
                "a folder there",
                State::Folder,
                Some(["is a directory"].as_slice()),
            ),
        ];
        for (label, state, want) in cases {
            let (server_dir, _record) = server_stub("cfg-clash");
            std::fs::write(server_dir.join(SERVER_CONFIG), "hostname = \"retail\";\n").unwrap();
            let src = tmp("cfg-clash-src");
            let built = src.join("@Built");
            std::fs::create_dir_all(&built).unwrap();
            let root = tmp("cfg-clash-root");
            let staged = server_dir.join(DIAG_CONFIG_NAME);
            let gone: Vec<PathBuf> = match state {
                State::Missing => Vec::new(),
                State::Folder => {
                    std::fs::create_dir(&staged).unwrap();
                    Vec::new()
                }
                State::File | State::FileFromADeadRun => {
                    std::fs::write(&staged, b"other run").unwrap();
                    matches!(state, State::FileFromADeadRun)
                        .then(|| journal_path(&staged).unwrap())
                        .into_iter()
                        .collect()
                }
            };
            let args = RunServerArgs {
                source_root: Some(root.clone()),
                ..server_args(built, &server_dir)
            };

            match (want, plan_server(&args, &gone)) {
                (None, Ok(plan)) => assert!(
                    !plan
                        .steps
                        .iter()
                        .any(|step| matches!(step, StageStep::Remove { path } if path == &staged)),
                    "{label}: {:?}",
                    plan.steps
                ),
                (None, Err(error)) => panic!("{label}: {error}"),
                (Some(want), Err(error)) => {
                    let error = error.to_string();
                    for text in want {
                        assert!(error.contains(text), "{label}: {error}");
                    }
                }
                (Some(_), Ok(_)) => panic!("{label}: the plan went through"),
            }
            if matches!(state, State::File | State::FileFromADeadRun) {
                assert_eq!(
                    std::fs::read(&staged).unwrap(),
                    b"other run",
                    "{label}: the file must be left alone"
                );
            }

            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&src);
            let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
        }
    }

    #[test]
    fn two_mods_with_different_keys_of_one_name_are_refused() {
        let (server_dir, _record) = server_stub("plan-key-clash");
        let src = tmp("plan-key-clash-src");
        let built = src.join("@Built");
        let test_mod = src.join("Other");
        std::fs::create_dir_all(built.join("keys")).unwrap();
        std::fs::write(built.join("keys/Author.bikey"), b"one").unwrap();
        std::fs::create_dir_all(test_mod.join("keys")).unwrap();
        std::fs::write(test_mod.join("keys/Author.bikey"), b"other").unwrap();
        let args = RunServerArgs {
            test_mods: vec![TestModArg {
                path: test_mod,
                link_name: "Other".into(),
                role: TestModRole::Both,
            }],
            ..server_args(built, &server_dir)
        };

        let error = plan_server(&args, &[]).unwrap_err().to_string();
        assert!(
            error.contains("Author.bikey") && error.contains("conflict"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn a_leftover_listed_under_the_installs_canonical_path_is_recognised() {
        let root = tmp("canonical");
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let recorded = real.canonicalize().unwrap().join("@Mod");
        assert!(absent(std::slice::from_ref(&recorded), &alias.join("@Mod")));
        assert!(!absent(
            std::slice::from_ref(&recorded),
            &alias.join("@Other")
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_link_a_dead_run_left_is_planned_as_a_fresh_one() {
        use crate::launch_plan::StageStep;

        let (server_dir, _record) = server_stub("plan-leftover");
        let src = tmp("plan-leftover-src");
        let built = src.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let elsewhere = src.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let link = server_dir.join("@Built");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let args = server_args(built.clone(), &server_dir);

        assert!(plan_server(&args, &[]).is_err(), "a live link is refused");

        let plan = plan_server(&args, std::slice::from_ref(&link)).unwrap();
        assert_eq!(plan.leftovers, vec![link.clone()]);
        assert!(
            plan.steps.contains(&StageStep::Link {
                link,
                target: std::path::absolute(&built).unwrap()
            }),
            "{:?}",
            plan.steps
        );

        let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(&src);
    }

    /// Leaves `path` as a run that died left it, its journal under a dead pid's name.
    fn leave_staged_by_a_dead_run(install: &Path, path: &Path, contents: &[u8]) {
        let mut staged = Staged::in_install(install);
        staged.write(path, contents).unwrap();
        std::mem::forget(staged);
        let dir = install.join(JOURNAL_NAME);
        let ours = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::rename(ours, dir.join(format!("{}-0.jsonl", dead_pid()))).unwrap();
    }

    #[test]
    fn run_server_clears_the_config_a_dead_run_staged_and_runs() {
        let (server_dir, _record) = server_stub("dead-cfg");
        std::fs::write(server_dir.join(SERVER_CONFIG), "hostname = \"retail\";\n").unwrap();
        let staged = server_dir.join(DIAG_CONFIG_NAME);
        leave_staged_by_a_dead_run(&server_dir, &staged, b"left");
        let src = tmp("dead-cfg-src");
        let built = src.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let root = tmp("dead-cfg-root");
        let args = RunServerArgs {
            source_root: Some(root.clone()),
            ..server_args(built, &server_dir)
        };

        run_server(&args, &StopSignal::new()).unwrap();

        assert!(!staged.exists(), "the run removes what it staged");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
    }

    #[test]
    fn run_server_removes_a_key_a_dead_run_left_and_keeps_one_it_did_not_record() {
        for journalled in [true, false] {
            let (server_dir, _record) = server_stub("clear-dead");
            let keys = server_dir.join("keys");
            std::fs::create_dir(&keys).unwrap();
            let key = keys.join("Stale.bikey");
            if journalled {
                leave_staged_by_a_dead_run(&server_dir, &key, b"key");
            } else {
                std::fs::write(&key, b"key").unwrap();
            }
            let src = tmp("clear-dead-src");
            let built = src.join("@Built");
            std::fs::create_dir_all(&built).unwrap();
            let args = RunServerArgs {
                mod_dir: built,
                server_dir: Some(server_dir.clone()),
                config: None,
                set: None,
                set_config: None,
                port: None,
                profiles_dir: None,
                timeout_secs: 5,
                source_root: None,
                run_id: None,
                test_mods: Vec::new(),
                expect_lines: Vec::new(),
                fail_lines: Vec::new(),
                allow_errors: Vec::new(),
            };

            let outcome = run_server(&args, &StopSignal::new()).unwrap();

            assert_eq!(key.is_file(), !journalled, "journalled: {journalled}");
            let want = if journalled {
                vec![key.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(outcome.cleared, want, "journalled: {journalled}");
            assert!(
                !server_dir.join(JOURNAL_NAME).exists(),
                "a finished run leaves no journal: {journalled}"
            );

            let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
            let _ = std::fs::remove_dir_all(&src);
        }
    }

    #[test]
    fn run_server_loads_a_both_test_mod_by_mod_and_a_server_only_one_by_server_mod() {
        let (server_dir, record) = server_stub("server-test-mods");
        let src = tmp("server-test-mods-src");
        let built = src.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let both = src.join("A");
        let server_only = src.join("B");
        std::fs::create_dir_all(both.join("keys")).unwrap();
        std::fs::write(both.join("keys/A.bikey"), b"a").unwrap();
        std::fs::create_dir_all(&server_only).unwrap();
        let args = RunServerArgs {
            mod_dir: built,
            server_dir: Some(server_dir.clone()),
            config: None,
            set: None,
            set_config: None,
            port: None,
            profiles_dir: None,
            timeout_secs: 5,
            source_root: None,
            run_id: None,
            test_mods: vec![
                TestModArg {
                    path: both,
                    link_name: "A".into(),
                    role: TestModRole::Both,
                },
                TestModArg {
                    path: server_only,
                    link_name: "B".into(),
                    role: TestModRole::ServerOnly,
                },
            ],
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        };
        run_server(&args, &StopSignal::new()).unwrap();

        let argv = recorded_argv(&record);
        assert!(argv.contains(&"-mod=@A;@Built".to_string()), "{argv:?}");
        assert!(argv.contains(&"-serverMod=@B".to_string()), "{argv:?}");
        assert!(
            std::fs::read_to_string(&record)
                .unwrap()
                .contains("key:A.bikey"),
            "a test mod's key is in the server's keys/ while it runs"
        );
        assert!(!server_dir.join("keys/A.bikey").exists());
        for link in ["@A", "@B", "@Built"] {
            assert!(
                server_dir.join(link).symlink_metadata().is_err(),
                "{link} must be removed after the run"
            );
        }
    }

    #[test]
    fn run_client_stages_a_test_mod_before_the_built_mod() {
        let fx = steam_fixture("client-test-mods");
        let launcher = record_launcher(&fx);
        let src = tmp("client-test-mods-src");
        let built = src.join("@Built");
        let test_mod = src.join("A");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::create_dir_all(&test_mod).unwrap();
        let args = RunClientArgs {
            mods: vec![built],
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            test_mods: vec![TestModArg {
                path: test_mod,
                link_name: "A".into(),
                role: TestModRole::Both,
            }],
            ..client_args(&fx, Vec::new())
        };
        run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        let argv = recorded_argv(&fx.record);
        assert!(argv.contains(&"-mod=@A;@Built".to_string()), "{argv:?}");
        for link in ["@A", "@Built"] {
            assert!(
                fx.game_dir.join(link).symlink_metadata().is_err(),
                "{link} must be removed after the run"
            );
        }
    }

    /// A fixture with a game exe and one built mod, and client args with a 1 s window.
    fn built_client_run(name: &str, settle_secs: u64) -> (SteamFixture, PathBuf, RunClientArgs) {
        let fx = steam_fixture(name);
        std::fs::write(fx.game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let src = tmp(&format!("{name}-src"));
        let built = src.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let args = RunClientArgs {
            mods: vec![built],
            timeout_secs: 1,
            settle_secs,
            ..client_args(&fx, Vec::new())
        };
        (fx, src, args)
    }

    #[test]
    fn run_client_keeps_its_links_when_the_game_is_still_running_at_the_window() {
        let (fx, src, args) = built_client_run("client-kept", 0);
        // No game before the launch, then one that outlives the window.
        let calls = AtomicU64::new(0);
        let running = || calls.fetch_add(1, Ordering::Relaxed) > 0;

        let outcome = run_client_with(&args, &StopSignal::new(), &running).unwrap();

        assert!(
            outcome.links_kept,
            "a game still running at the window must keep its links"
        );
        assert_eq!(outcome.game_dir, fx.game_dir);
        assert!(
            fx.game_dir.join("@Built").symlink_metadata().is_ok(),
            "the link the running game loads from must survive the run"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn run_client_keeps_its_links_when_the_capture_itself_runs_out_of_window() {
        let (fx, src, args) = built_client_run("client-kept-capture", 30);
        let calls = AtomicU64::new(0);
        let running = || calls.fetch_add(1, Ordering::Relaxed) > 0;

        let outcome = run_client_with(&args, &StopSignal::new(), &running).unwrap();

        assert!(outcome.report.timed_out);
        assert!(outcome.links_kept, "{outcome:?}");
        assert!(fx.game_dir.join("@Built").symlink_metadata().is_ok());

        let _ = std::fs::remove_dir_all(&fx.root);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn run_client_removes_its_links_when_its_game_exits() {
        let (fx, src, args) = built_client_run("client-gone", 0);

        let outcome = run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        assert!(
            !outcome.links_kept,
            "a game that ended must leave nothing behind"
        );
        assert!(
            fx.game_dir.join("@Built").symlink_metadata().is_err(),
            "the link must be removed once the game has gone"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn run_client_uses_a_test_mod_that_already_sits_in_the_game_dir() {
        let fx = steam_fixture("client-mod-in-place");
        let launcher = record_launcher(&fx);
        let in_place = fx.game_dir.join("@A");
        std::fs::create_dir_all(&in_place).unwrap();
        std::fs::write(in_place.join("marker.txt"), b"here").unwrap();
        let src = tmp("client-mod-in-place-src");
        let built = src.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let args = RunClientArgs {
            mods: vec![built],
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            test_mods: vec![TestModArg {
                path: in_place.clone(),
                link_name: "A".into(),
                role: TestModRole::Both,
            }],
            ..client_args(&fx, Vec::new())
        };
        run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        let argv = recorded_argv(&fx.record);
        assert!(argv.contains(&"-mod=@A;@Built".to_string()), "{argv:?}");
        assert!(
            in_place.is_dir() && !in_place.is_symlink(),
            "@A must stay the real folder it was"
        );
        assert!(
            in_place.join("marker.txt").is_file(),
            "the folder's files must survive the run"
        );
        assert!(
            fx.game_dir.join("@Built").symlink_metadata().is_err(),
            "@Built must be removed after the run"
        );
    }

    #[test]
    fn run_client_removes_what_a_dead_run_left_in_the_game_dir() {
        let fx = steam_fixture("client-clear");
        let launcher = record_launcher(&fx);
        let stale = fx.game_dir.join("left-behind.txt");
        leave_staged_by_a_dead_run(&fx.game_dir, &stale, b"left");
        let args = RunClientArgs {
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            ..client_args(&fx, Vec::new())
        };

        let outcome = run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        assert!(
            !stale.exists(),
            "the game dir must lose what a dead run left"
        );
        assert_eq!(outcome.cleared, vec![stale]);
        assert!(!fx.game_dir.join(JOURNAL_NAME).exists());

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn run_client_with_a_launcher_leaves_a_dead_runs_leftover_while_a_game_runs() {
        let fx = steam_fixture("client-clear-running");
        let launcher = record_launcher(&fx);
        let stale = fx.game_dir.join("left-behind.txt");
        leave_staged_by_a_dead_run(&fx.game_dir, &stale, b"left");
        let args = RunClientArgs {
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            ..client_args(&fx, Vec::new())
        };

        let outcome = run_client_with(&args, &StopSignal::new(), &|| true).unwrap();

        assert!(
            stale.is_file(),
            "a game the launcher left running may still load what the dead run left"
        );
        assert!(outcome.cleared.is_empty(), "{:?}", outcome.cleared);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_client_plan_writes_nothing_and_names_the_command_a_run_launches() {
        for launcher in [false, true] {
            let label = if launcher {
                "plan-client-launcher"
            } else {
                "plan-client-direct"
            };
            let fx = steam_fixture(label);
            std::fs::write(fx.game_dir.join("DayZ_x64.exe"), b"").unwrap();
            let mod_root = tmp("plan-client-mod");
            let mod_dir = mod_root.join("@FieldKit");
            std::fs::create_dir_all(&mod_dir).unwrap();
            let mut args = client_args(&fx, vec![mod_dir.clone()]);
            args.launcher = launcher.then(|| record_launcher(&fx));
            args.timeout_secs = 1;
            args.settle_secs = 0;
            let before = listing(&fx.game_dir);

            let plan = plan_client(&args, &[]).unwrap();

            assert_eq!(
                listing(&fx.game_dir),
                before,
                "a plan writes nothing: launcher {launcher}"
            );
            assert_eq!(plan.launches.len(), 1);
            assert_eq!(plan.launches[0].role, LaunchRole::Client);
            assert!(
                plan.steps.contains(&StageStep::Link {
                    link: fx.game_dir.join("@FieldKit"),
                    target: std::path::absolute(&mod_dir).unwrap(),
                }),
                "launcher {launcher}: {:?}",
                plan.steps
            );
            let planned: Vec<String> = plan.launches[0]
                .command
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();

            run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

            assert_eq!(recorded_argv(&fx.record), planned, "launcher {launcher}");

            let _ = std::fs::remove_dir_all(&fx.root);
            let _ = std::fs::remove_dir_all(&mod_root);
        }
    }

    #[test]
    fn a_client_link_a_dead_run_left_is_planned_as_a_fresh_one() {
        let game_dir = tmp("game");
        let mod_dir = tmp("src").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();
        let elsewhere = tmp("elsewhere");
        let link = game_dir.join("@FieldKit");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let args = RunClientArgs {
            mods: vec![mod_dir.clone()],
            game_dir: Some(game_dir.clone()),
            steam_dir: Some(tmp("steam")),
            launcher: Some(PathBuf::from("/bin/true")),
            ..client_args_no_steam()
        };

        let refused = plan_client(&args, &[]).unwrap_err().to_string();
        assert!(refused.contains("@FieldKit"), "{refused}");

        let plan = plan_client(&args, std::slice::from_ref(&link)).unwrap();
        assert_eq!(plan.leftovers, vec![link.clone()]);
        assert!(
            plan.steps.contains(&StageStep::Link {
                link,
                target: std::path::absolute(&mod_dir).unwrap(),
            }),
            "{:?}",
            plan.steps
        );

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(elsewhere);
        let _ = std::fs::remove_dir_all(args.steam_dir.unwrap());
    }

    #[test]
    fn a_mod_listed_twice_is_linked_once_and_two_mods_sharing_a_name_are_refused() {
        let game_dir = tmp("game");
        let src = tmp("src");
        let mod_dir = src.join("a").join("@FieldKit");
        let other = src.join("b").join("@FieldKit");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let args = |mods: Vec<PathBuf>| RunClientArgs {
            mods,
            game_dir: Some(game_dir.clone()),
            steam_dir: Some(src.clone()),
            launcher: Some(PathBuf::from("/bin/true")),
            ..client_args_no_steam()
        };

        let plan = plan_client(&args(vec![mod_dir.clone(), mod_dir.clone()]), &[]).unwrap();
        assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps);
        let launch = format!("{:?}", plan.launches[0].command.args);
        assert_eq!(launch.matches("@FieldKit").count(), 2, "{launch}");

        let refused = plan_client(&args(vec![mod_dir, other]), &[])
            .unwrap_err()
            .to_string();
        assert!(refused.contains("does not point at"), "{refused}");

        let _ = std::fs::remove_dir_all(&game_dir);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn server_keys_stage_from_a_key_folder_in_any_spelling() {
        let root = tmp("server-key-spellings");
        let server = root.join("server");
        std::fs::create_dir_all(&server).unwrap();
        for (folder, file) in [("Keys", "Upper.bikey"), ("key", "Single.BIKEY")] {
            let keys = root.join("@Mod").join(folder);
            std::fs::create_dir_all(&keys).unwrap();
            std::fs::write(keys.join(file), file).unwrap();
        }
        let mod_dir = root.join("@Mod");
        let steps = key_steps(&[mod_dir.as_path()], &server, &[]).unwrap();
        let mut staged = Staged::in_install(&server);
        crate::launch_plan::apply(&steps, &mut staged).unwrap();
        assert!(server.join("keys/Upper.bikey").is_file());
        assert!(server.join("keys/Single.BIKEY").is_file());
        drop(staged);
        assert!(!server.join("keys").exists());
    }

    #[test]
    fn run_client_full_screen_drops_every_window_argument() {
        let fx = steam_fixture("client-fullscreen");
        let launcher = record_launcher(&fx);
        let args = RunClientArgs {
            launcher: Some(launcher),
            extra: vec!["-nolauncher".into()],
            window: Some(WindowSize {
                width: 800,
                height: 600,
            }),
            windowed: false,
            timeout_secs: 1,
            settle_secs: 0,
            ..client_args(&fx, Vec::new())
        };
        run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        let argv = recorded_argv(&fx.record);
        for flag in ["-window", "-x=", "-y="] {
            assert!(
                !argv.iter().any(|a| a.starts_with(flag)),
                "{flag} must be gone full screen: {argv:?}"
            );
        }
        assert!(
            argv.iter().any(|a| a == "-nolauncher"),
            "the caller's extra must still be appended: {argv:?}"
        );
    }

    #[test]
    fn run_server_refuses_a_test_mod_named_like_the_build_and_stages_nothing() {
        let (server_dir, record) = server_stub("server-clash-test-mod");
        let src = tmp("server-clash-test-mod-src");
        let built = src.join("@Built");
        let other = src.join("Other");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let args = RunServerArgs {
            mod_dir: built,
            server_dir: Some(server_dir.clone()),
            config: None,
            set: None,
            set_config: None,
            port: None,
            profiles_dir: None,
            timeout_secs: 5,
            source_root: None,
            run_id: None,
            test_mods: vec![TestModArg {
                path: other,
                link_name: "Built".into(),
                role: TestModRole::Both,
            }],
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        };
        let err = run_server(&args, &StopSignal::new())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("test mod @Built has the build's name"),
            "{err}"
        );
        assert!(
            !record.exists(),
            "the server must not launch after a refusal"
        );
        assert!(server_dir.join("@Built").symlink_metadata().is_err());
    }

    #[test]
    fn run_client_refuses_a_server_only_test_mod_before_linking_anything() {
        let src = tmp("client-server-only-src");
        let server_only = src.join("Srv");
        std::fs::create_dir_all(&server_only).unwrap();
        let args = RunClientArgs {
            test_mods: vec![TestModArg {
                path: server_only,
                link_name: "Srv".into(),
                role: TestModRole::ServerOnly,
            }],
            game_dir: Some(src.join("no-game")),
            ..client_args_no_steam()
        };
        let err = run_client_with(&args, &StopSignal::new(), &|| false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Srv") && err.contains("server only"), "{err}");
    }

    #[test]
    fn a_client_verdict_judges_the_logs_the_client_wrote() {
        let fx = steam_fixture("client-verdict");
        let launcher = fx.root.join("launcher");
        write_exec(
            &launcher,
            &format!(
                "#!/bin/sh\nprintf ' 1:00:00.000 SCRIPT       : [SampleModTests] FAIL Cfg\\r\\n' > '{}/DayZ_x64_x.RPT'\n",
                fx.log_dir.display()
            ),
        );
        let args = RunClientArgs {
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            fail_lines: vec!["[SampleModTests] FAIL".into()],
            ..client_args(&fx, Vec::new())
        };

        let outcome = run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        let verdict = outcome.verdict.expect("the run asked for a verdict");
        assert!(!verdict.passed, "{verdict:?}");
        assert_eq!(verdict.fail_lines.len(), 1, "{verdict:?}");
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    /// A fake DayZServer that writes `rpt` into its `-profiles` folder with `redirect`
    /// (`>` or `>>`) and exits.
    fn server_writing(label: &str, rpt: &str, redirect: &str) -> PathBuf {
        let root = tmp(label);
        let server_dir = root.join("DayZServer");
        std::fs::create_dir_all(&server_dir).unwrap();
        write_exec(
            &server_dir.join("DayZServer"),
            &format!(
                "#!/bin/sh\nprof=''; for a in \"$@\"; do case \"$a\" in -profiles=*) prof=\"${{a#-profiles=}}\";; esac; done\ncat {redirect} \"$prof/DayZServer_1.RPT\" <<'RPT'\n{rpt}\nRPT\n"
            ),
        );
        server_dir
    }

    /// A fake DayZServer that writes `rpt` into its `-profiles` folder and exits.
    fn logging_server(label: &str, rpt: &str) -> PathBuf {
        server_writing(label, rpt, ">")
    }

    #[test]
    fn a_server_verdict_ignores_lines_an_earlier_run_left_in_an_appended_log() {
        let cases = [
            (
                "an appended log",
                "[SampleModTests] FAIL old\r\n",
                "[SampleModTests] DONE\r\n",
                ">>",
            ),
            (
                "a rewritten longer log",
                "old\r\n",
                "[SampleModTests] DONE\r\nmore text\r\n",
                ">",
            ),
        ];
        for (label, old, written, redirect) in cases {
            let server_dir = server_writing("verdict-kept", written, redirect);
            let src = tmp("verdict-kept-src");
            let built = src.join("@Built");
            std::fs::create_dir_all(&built).unwrap();
            let profiles = tmp("verdict-kept-profiles");
            std::fs::write(profiles.join("DayZServer_1.RPT"), old).unwrap();
            let mut args = server_args(built, &server_dir);
            args.profiles_dir = Some(profiles.clone());
            args.expect_lines = vec!["[SampleModTests] DONE".into()];
            args.fail_lines = vec!["[SampleModTests] FAIL".into()];

            let outcome = run_server(&args, &StopSignal::new()).unwrap();

            let verdict = outcome.verdict.expect("a verdict list asks for a verdict");
            assert!(verdict.passed, "{label}: {verdict:?}");
            let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
            let _ = std::fs::remove_dir_all(&src);
            let _ = std::fs::remove_dir_all(&profiles);
        }
    }

    /// A project whose `server/known-errors.json` allows the CF `JSON ERROR` line by pattern.
    fn known_errors_project(root: &Path) {
        let file = root.join(crate::known_errors::KNOWN_ERRORS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            serde_json::json!([{
                "id": "cf000000",
                "pattern": "JSON ERROR JM/CF",
                "meaning": "CF logs one on every boot",
                "hint": "ignore it",
            }])
            .to_string(),
        )
        .unwrap();
    }

    const CF_AND_MOD_ERRORS: &str = concat!(
        " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46\r\n",
        " 3:45:52.000 SCRIPT    (E): @\"Built/scripts/3_Game/broken.c,5\": Can't find variable 'x'\r\n",
    );

    #[test]
    fn a_server_verdict_ignores_an_error_the_projects_known_file_explains() {
        let server_dir = logging_server("known-srv", CF_AND_MOD_ERRORS);
        let root = tmp("known-srv-root");
        known_errors_project(&root);
        let built = root.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let profiles = tmp("known-srv-profiles");
        let mut args = server_args(built, &server_dir);
        args.profiles_dir = Some(profiles.clone());
        args.source_root = Some(root.clone());
        args.fail_lines = vec!["never appears".into()];

        let outcome = run_server(&args, &StopSignal::new()).unwrap();

        let verdict = outcome.verdict.expect("a fail line asks for a verdict");
        assert_eq!(verdict.ignored, 1, "{verdict:?}");
        assert_eq!(
            verdict.mod_errors.len(),
            1,
            "the mod error still fails it: {verdict:?}"
        );
        assert!(!verdict.passed, "{verdict:?}");
        let cf = outcome
            .errors
            .iter()
            .find(|e| e.message.starts_with("JSON ERROR"))
            .unwrap();
        assert_eq!(cf.known.as_ref().map(|k| k.id.as_str()), Some("cf000000"));

        for dir in [server_dir.parent().unwrap(), &root, &profiles] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_run_without_a_verdict_list_still_marks_the_known_errors() {
        let server_dir = logging_server("known-silent", CF_AND_MOD_ERRORS);
        let root = tmp("known-silent-root");
        known_errors_project(&root);
        let built = root.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let profiles = tmp("known-silent-profiles");
        let mut args = server_args(built, &server_dir);
        args.profiles_dir = Some(profiles.clone());
        args.source_root = Some(root.clone());

        let outcome = run_server(&args, &StopSignal::new()).unwrap();

        assert!(
            outcome.verdict.is_none(),
            "a known-errors file asks for no verdict"
        );
        assert!(outcome
            .errors
            .iter()
            .any(|e| e.known.as_ref().is_some_and(|k| k.id == "cf000000")));

        for dir in [server_dir.parent().unwrap(), &root, &profiles] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_malformed_known_errors_file_refuses_the_run_before_it_launches() {
        let root = tmp("known-bad-run");
        let server_dir = logging_server("known-bad-run-srv", CF_AND_MOD_ERRORS);
        std::fs::create_dir_all(root.join("server")).unwrap();
        std::fs::write(
            root.join(crate::known_errors::KNOWN_ERRORS_FILE),
            "not json",
        )
        .unwrap();
        let built = root.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let profiles = tmp("known-bad-run-profiles");
        let mut args = server_args(built, &server_dir);
        args.profiles_dir = Some(profiles.clone());
        args.source_root = Some(root.clone());

        let err = run_server(&args, &StopSignal::new())
            .expect_err("a malformed known-errors file must refuse the run")
            .to_string();

        assert!(
            err.starts_with(
                &root
                    .join(crate::known_errors::KNOWN_ERRORS_FILE)
                    .display()
                    .to_string()
            ),
            "{err}"
        );
        assert!(
            modfoundry_diag::find_logs(&profiles).is_empty(),
            "a refused run writes no log"
        );

        for dir in [server_dir.parent().unwrap(), &root, &profiles] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_client_run_marks_the_errors_its_projects_known_file_explains() {
        let fx = steam_fixture("known-client");
        let launcher = fx.root.join("launcher");
        write_exec(
            &launcher,
            &format!(
                "#!/bin/sh\nprintf '{}' > '{}/DayZ_x64_x.RPT'\n",
                CF_AND_MOD_ERRORS.replace('\'', "'\\''"),
                fx.log_dir.display()
            ),
        );
        let root = tmp("known-client-root");
        known_errors_project(&root);
        let built = root.join("@Built");
        std::fs::create_dir_all(&built).unwrap();
        let args = RunClientArgs {
            mods: vec![built],
            launcher: Some(launcher),
            timeout_secs: 1,
            settle_secs: 0,
            source_root: Some(root.clone()),
            ..client_args(&fx, Vec::new())
        };

        let outcome = run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

        let cf = outcome
            .errors
            .iter()
            .find(|e| e.message.starts_with("JSON ERROR"))
            .expect("the CF line is mapped");
        assert_eq!(cf.known.as_ref().map(|k| k.id.as_str()), Some("cf000000"));
        assert!(
            outcome.verdict.is_none(),
            "a known-errors file alone asks for no verdict"
        );

        for dir in [&fx.root, &root] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn verdict_mods_name_every_pbo_prefix_of_a_mod() {
        use modfoundry_pbo::{pack_pbo, PackOptions};
        let mod_dir = tmp("verdict-mods").join("@Expansion-Core");
        let addons = mod_dir.join("addons");
        std::fs::create_dir_all(&addons).unwrap();
        for (file, prefix) in [
            ("core.pbo", "DayZExpansion\\Core\\"),
            ("ui.pbo", "DayZExpansion\\UI"),
        ] {
            let props = [("prefix".to_owned(), prefix.to_owned())];
            let bytes = pack_pbo(&props, &[], &PackOptions::default()).unwrap();
            std::fs::write(addons.join(file), bytes).unwrap();
        }
        std::fs::write(addons.join("broken.pbo"), b"truncated").unwrap();

        let mut mods = verdict_mods([mod_dir.as_path()], &[]);
        mods.sort();

        assert_eq!(
            mods,
            ["DayZExpansion\\Core", "DayZExpansion\\UI", "Expansion-Core"]
        );
    }

    #[test]
    fn a_client_run_fails_only_on_a_script_error_in_its_last_mod() {
        for (named, passed) in [("First", true), ("Last", false)] {
            let fx = steam_fixture(&format!("client-own-mod-{named}"));
            let mods = ["@First", "@Last"].map(|name| fx.root.join(name));
            for dir in &mods {
                std::fs::create_dir_all(dir).unwrap();
            }
            let launcher = fx.root.join("launcher");
            write_exec(
                &launcher,
                &format!(
                    "#!/bin/sh\nprintf ' SCRIPT    (E): @\"{named}/Scripts/3_Game/x.c,1\": boom\\r\\n' > '{}/DayZ_x64_x.RPT'\n",
                    fx.log_dir.display()
                ),
            );
            let args = RunClientArgs {
                launcher: Some(launcher),
                timeout_secs: 1,
                settle_secs: 0,
                fail_lines: vec!["[SampleModTests] FAIL".into()],
                ..client_args(&fx, mods.to_vec())
            };

            let outcome = run_client_with(&args, &StopSignal::new(), &|| false).unwrap();

            let verdict = outcome.verdict.expect("the run asked for a verdict");
            assert_eq!(verdict.passed, passed, "{named}: {verdict:?}");
            let _ = std::fs::remove_dir_all(&fx.root);
        }
    }

    #[test]
    fn a_client_verdict_names_only_its_last_mod_and_the_test_mods() {
        let root = tmp("verdict-last");
        let first = root.join("@First");
        let last = root.join("@Last");
        let extra = root.join("Extra");
        for dir in [&first, &last, &extra] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let args = RunClientArgs {
            mods: vec![first, last],
            test_mods: vec![TestModArg {
                path: extra,
                link_name: "Extra".to_owned(),
                role: TestModRole::Both,
            }],
            ..client_args_no_steam()
        };

        let mut mods = client_verdict_mods(&args);
        mods.sort();
        assert_eq!(mods, ["Extra", "Last"]);

        let mut mods = client_verdict_mods(&RunClientArgs {
            mods: Vec::new(),
            ..args
        });
        mods.sort();
        assert_eq!(mods, ["Extra"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_flood_of_storage_errors_gives_the_run_a_hint() {
        for (lines, wants_hint) in [(99, false), (100, true)] {
            let rpt = vec!["Scripted variables corrupted: 1"; lines].join("\n");
            let server_dir = logging_server("storage-hint", &rpt);
            let src = tmp("storage-hint-src");
            let built = src.join("@Built");
            std::fs::create_dir_all(&built).unwrap();
            let profiles = tmp("storage-hint-profiles");
            let mut args = server_args(built, &server_dir);
            args.profiles_dir = Some(profiles.clone());

            let outcome = run_server(&args, &StopSignal::new()).unwrap();

            match (wants_hint, &outcome.hint) {
                (false, None) => {}
                (true, Some(hint)) => {
                    assert!(hint.contains("100"), "{hint}");
                    assert!(hint.contains("Scripted variables corrupted"), "{hint}");
                }
                other => panic!("{lines} lines: {other:?}"),
            }
            let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
            let _ = std::fs::remove_dir_all(&src);
            let _ = std::fs::remove_dir_all(&profiles);
        }
    }

    #[test]
    fn a_verdict_fails_on_a_script_error_naming_the_runs_mod_unless_it_is_allowed() {
        let rpt = " SCRIPT    (E): @\"Built/Scripts/3_Game/x.c,1\": boom";
        for (allow, passed) in [(None, false), (Some("boom"), true)] {
            let server_dir = logging_server("verdict-error", rpt);
            let src = tmp("verdict-error-src");
            let built = src.join("@Built");
            std::fs::create_dir_all(&built).unwrap();
            let profiles = tmp("verdict-error-profiles");
            let mut args = server_args(built, &server_dir);
            args.profiles_dir = Some(profiles);
            args.fail_lines = vec!["[SampleModTests] FAIL".into()];
            args.allow_errors = allow.map(|text| vec![text.to_owned()]).unwrap_or_default();

            let outcome = run_server(&args, &StopSignal::new()).unwrap();

            let verdict = outcome.verdict.expect("a verdict list asks for a verdict");
            assert_eq!(verdict.passed, passed, "{verdict:?}");
            assert_eq!(verdict.mod_errors.len(), usize::from(allow.is_none()));
            assert_eq!(verdict.ignored, usize::from(allow.is_some()));
            let _ = std::fs::remove_dir_all(server_dir.parent().unwrap());
            let _ = std::fs::remove_dir_all(&src);
        }
    }
}
