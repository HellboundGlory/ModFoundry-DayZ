//! `diag.run_diag`: one bounded run of the Diag server and, by default, the
//! Diag client that joins it. Everything the pair needs inside the client
//! install is staged by [`crate::diag_stage`] and removed when the run ends.

use crate::diag_stage::{plan_diag_stage, DiagStage, DiagStageArgs, TestModArg};
use crate::known_errors::KnownErrors;
use crate::launch_plan::{LaunchRole, PlannedLaunch, RunPlan, StageStep};
use crate::run_ops::{
    apply_project, clear_dead, client_log_dir, client_log_dirs, errors_from_logs, judge_logs,
    proton_command, resolve_game_dir, resolve_server_dir, resolve_steam_root, verdict_mods,
    verdict_rules, wait_for_client_end, window_args, ClientEnd, ClientRunOutcome, VerdictWatch,
    CLIENT_POLL_INTERVAL,
};
use modfoundry_diag::{
    capture_client, capture_command, count_storage_errors, find_logs, storage_flood_hint,
    wait_for_log_marker, ClientProfile, ClientWait, Connect, LaunchCommand, LaunchKind, LogFlags,
    LogStates, MappedError, MarkerWait, RunReport, ServerProfile, StopSignal, Verdict,
};
use modfoundry_platform::stage::{dead_run_leftovers, Staged};
use modfoundry_vfs::{root_name, write::WriteTxn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The client install's Diag executable.
pub const DIAG_EXE: &str = "DayZDiag_x64.exe";

/// The line the Diag server writes once it accepts players; the client is
/// started only after it appears.
pub const SERVER_READY_MARKER: &str = "Player connect enabled";

/// The Diag server port when the caller does not pin one.
pub const DEFAULT_DIAG_PORT: u16 = 2302;

/// Window handed to the lower layers when the caller sets none: long enough
/// never to elapse, so their `u64`-second signatures stay unchanged.
pub(crate) const NO_TIMER_SECS: u64 = 10 * 365 * 24 * 60 * 60;

/// Bound for the ready-marker wait, independent of the run window: a server
/// that never starts accepting players must still give up.
pub const SERVER_READY_TIMEOUT_SECS: u64 = crate::run_ops::DEFAULT_RUN_TIMEOUT_SECS;

/// The line a run that never reported ready ends with.
pub fn ready_timeout_message() -> String {
    format!("the server never reported ready within {SERVER_READY_TIMEOUT_SECS} s and was stopped")
}

/// The host interactions `run_diag` needs, so tests drive it without reading the
/// real process table, sleeping, or touching an installed game.
pub trait DiagHost {
    fn client_running(&self) -> bool;
    fn server_running(&self) -> bool;
    fn stop_client(&self) -> std::io::Result<usize>;
    fn stop_server(&self) -> std::io::Result<usize>;
    fn sleep(&self, duration: Duration);
    /// Quiet period `capture_client` treats as a complete client log.
    fn client_settle_secs(&self) -> u64 {
        crate::run_ops::CLIENT_SETTLE_SECS
    }
    /// How long the ready-marker wait gives a server that never reports ready.
    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(SERVER_READY_TIMEOUT_SECS)
    }
    /// Called with the launch window the captures get, so a test host records
    /// that an unbounded run passes them no deadline that can elapse.
    fn launch_window_secs(&self, _secs: u64) {}
    /// Called with the ready-marker wait's result; a test host records it.
    fn ready_wait_result(&self, _result: MarkerWait) {}
}

/// Arguments for the `diag.run_diag` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDiagArgs {
    /// Built `@Mod` dir loaded via `-mod=`.
    pub mod_dir: PathBuf,
    /// Loose mod source root (wins over the packed build under `-filePatching`).
    pub source_root: PathBuf,
    /// The server config template under the project's `server/` folder (default `serverDZ.cfg`).
    #[serde(default)]
    pub config: Option<PathBuf>,
    /// A named launch set whose mods and config this run takes.
    #[serde(default)]
    pub set: Option<String>,
    /// PBO prefix the loose source is staged as; default: the source dir name.
    #[serde(default)]
    pub mod_prefix: Option<String>,
    /// DayZ client install dir override (default: platform discovery).
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
    /// DayZServer install dir the `mpmissions`/config come from.
    #[serde(default)]
    pub server_dir: Option<PathBuf>,
    /// Steam library root override (default: platform discovery).
    #[serde(default)]
    pub steam_dir: Option<PathBuf>,
    /// Real profiles dir the staged link points at (default: `<source_root>/server/profiles`).
    #[serde(default)]
    pub profiles_dir: Option<PathBuf>,
    /// Diag server port (default: [`DEFAULT_DIAG_PORT`]).
    #[serde(default)]
    pub port: Option<u16>,
    /// Whether to also start the Diag client that joins the server.
    #[serde(default = "default_client")]
    pub client: bool,
    /// `-name=<profile>` game profile name for the client.
    #[serde(default)]
    pub connect_name: Option<String>,
    /// Windowed client size; `None` opens the default 1280x720 window.
    #[serde(default)]
    pub window: Option<crate::run_ops::WindowSize>,
    /// Whether the client opens in a window; false runs it full screen.
    #[serde(default = "default_windowed")]
    pub windowed: bool,
    /// Bounded launch window in seconds; `None` keeps running until stopped.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Optional id under which the caller can stop this run via `diag.stop`.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Extra mods loaded only for this run; the caller resolves workshop ids.
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

fn default_client() -> bool {
    true
}

pub(crate) fn default_windowed() -> bool {
    true
}

/// Result of a `diag.run_diag` launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagOutcome {
    pub profiles_dir: PathBuf,
    pub server: RunReport,
    /// The client half; `None` when `client` was false or the server never
    /// reported ready.
    pub client: Option<ClientRunOutcome>,
    /// Errors mapped from both log sets.
    pub errors: Vec<MappedError>,
    /// Whether the run ended because the server never reported ready and was
    /// stopped.
    #[serde(default)]
    pub ready_timed_out: bool,
    /// A likely cause for a saved world the server could not load.
    #[serde(default)]
    pub hint: Option<String>,
    /// What a run that died left in either install and this one removed.
    #[serde(default)]
    pub cleared: Vec<PathBuf>,
    /// Whether the run ended because its client closed and its server was stopped.
    #[serde(default)]
    pub client_closed: bool,
    /// How the run's logs judged against the caller's verdict lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
}

/// How often the ready-marker wait re-checks the server's log.
pub(crate) const MARKER_POLL: Duration = Duration::from_millis(250);

pub(crate) fn diag_log_dirs(args: &RunDiagArgs) -> Vec<PathBuf> {
    let profiles = diag_profiles_dir(args);
    let mut dirs = vec![std::path::absolute(&profiles).unwrap_or(profiles)];
    if args.client {
        dirs.extend(client_log_dirs(args.steam_dir.as_deref()));
    }
    dirs
}

/// Where a diag run keeps its profiles and logs: the caller's folder, else the project's.
fn diag_profiles_dir(args: &RunDiagArgs) -> PathBuf {
    args.profiles_dir
        .clone()
        .unwrap_or_else(|| project_profiles_dir(&args.source_root))
}

/// The installs and folders a diag run works in, resolved once.
#[derive(Debug, Clone)]
pub(crate) struct DiagDirs {
    pub(crate) game: PathBuf,
    pub(crate) exe: PathBuf,
    pub(crate) server: PathBuf,
    pub(crate) steam: PathBuf,
}

impl DiagDirs {
    /// What dead runs left staged in the game and server installs.
    fn dead_run_leftovers(&self) -> Vec<PathBuf> {
        let mut found = dead_run_leftovers(&self.game);
        found.extend(dead_run_leftovers(&self.server));
        found
    }
}

/// Resolves the game dir and its Diag exe, the server dir and the Steam root.
fn resolve_dirs(args: &RunDiagArgs) -> anyhow::Result<DiagDirs> {
    let game = resolve_game_dir(args.game_dir.as_deref())?;
    let exe = game.join(DIAG_EXE);
    if !exe.is_file() {
        anyhow::bail!("{DIAG_EXE} not found in {}", game.display());
    }
    Ok(DiagDirs {
        game,
        exe,
        server: resolve_server_dir(args.server_dir.as_deref())?,
        steam: resolve_steam_root(args.steam_dir.as_deref())?,
    })
}

/// Works out every check, path, argument and file a diag preview shows, without writing anything.
/// It resolves the dirs and treats what dead runs left in either install as absent, as a run does.
pub(crate) fn plan_diag(args: &RunDiagArgs) -> anyhow::Result<RunPlan> {
    let dirs = resolve_dirs(args)?;
    let gone = dirs.dead_run_leftovers();
    plan_diag_in(args, &dirs, &gone)
}

/// The launch plan itself, over dirs already resolved. Paths in `gone` (in the form
/// `dead_run_leftovers` returns) are treated as absent: they are a dead run's staging, which a run
/// clears before it plans.
pub(crate) fn plan_diag_in(
    args: &RunDiagArgs,
    dirs: &DiagDirs,
    gone: &[PathBuf],
) -> anyhow::Result<RunPlan> {
    let port = args.port.unwrap_or(DEFAULT_DIAG_PORT);
    if !args.source_root.is_dir() {
        anyhow::bail!("source root {} is not a folder", args.source_root.display());
    }
    let profiles_dir = diag_profiles_dir(args);
    let mod_prefix = match &args.mod_prefix {
        Some(prefix) => prefix.clone(),
        None => default_mod_prefix(&args.mod_dir, &args.source_root)?,
    };
    let mut project = Vec::new();
    let (stage, steps, stale) = plan_diag_stage(
        &DiagStageArgs {
            game_dir: dirs.game.clone(),
            server_dir: dirs.server.clone(),
            config: args.config.clone(),
            built_mod: args.mod_dir.clone(),
            source_root: args.source_root.clone(),
            mod_prefix,
            profiles_dir: profiles_dir.clone(),
            test_mods: args.test_mods.clone(),
        },
        gone,
        &mut project,
    )?;
    let mut launches = vec![PlannedLaunch {
        role: LaunchRole::Server,
        command: proton_command(&dirs.steam, &dirs.exe, &diag_server_argv(&stage, port))?,
    }];
    if args.client {
        launches.push(PlannedLaunch {
            role: LaunchRole::Client,
            command: client_launch(&dirs.steam, &dirs.exe, &stage, args, port)?,
        });
    }
    let mut leftovers = stale;
    leftovers.extend(dirs.dead_run_leftovers());
    Ok(RunPlan {
        launches,
        steps,
        project,
        leftovers,
    })
}

/// Applies a plan's project files and install staging, keeping a guard that removes the staging
/// again when the run ends.
pub(crate) fn stage_steps(
    game_dir: &Path,
    project: &[StageStep],
    steps: &[StageStep],
) -> anyhow::Result<Staged> {
    apply_project(project)?;
    let mut staged = Staged::in_install(&std::path::absolute(game_dir)?);
    crate::launch_plan::apply(steps, &mut staged)?;
    Ok(staged)
}

/// Run the Diag server (+ client) for `args`, returning the captured reports.
pub fn run_diag(args: &RunDiagArgs, stop: &StopSignal) -> anyhow::Result<DiagOutcome> {
    run_diag_with(args, stop, &RealHost)
}

/// [`run_diag`] over an injected host, so tests control the process checks, the
/// delays and the client settle period.
pub fn run_diag_with<H: DiagHost + ?Sized>(
    args: &RunDiagArgs,
    stop: &StopSignal,
    host: &H,
) -> anyhow::Result<DiagOutcome> {
    let known = KnownErrors::load(None, Some(&args.source_root))?;
    let rules = verdict_rules(
        &args.expect_lines,
        &args.fail_lines,
        &args.allow_errors,
        &known,
        || {
            let mut mods = verdict_mods(std::iter::once(args.mod_dir.as_path()), &args.test_mods);
            mods.extend(
                args.mod_prefix
                    .clone()
                    .or_else(|| default_mod_prefix(&args.mod_dir, &args.source_root).ok()),
            );
            mods
        },
    )?;
    let dirs = resolve_dirs(args)?;
    if host.client_running() {
        anyhow::bail!("a DayZ client is already running; stop it first");
    }
    if host.server_running() {
        anyhow::bail!("a DayZ Diag server is already running; stop it first");
    }
    let profiles_dir = diag_profiles_dir(args);
    let gone = dirs.dead_run_leftovers();
    plan_diag_in(args, &dirs, &gone)?;
    let mut cleared = clear_dead(&dirs.game)?;
    cleared.extend(clear_dead(&dirs.server)?);
    let plan = plan_diag_in(args, &dirs, &[])?;
    let _prune = PruneOnDrop(args.profiles_dir.is_none().then(|| profiles_dir.clone()));
    let _staged = stage_steps(&dirs.game, &plan.project, &plan.steps)?;

    let client_log = if args.client {
        Some(client_log_dir(&dirs.steam)?)
    } else {
        None
    };
    let mut launches = plan.launches.into_iter();
    let server_cmd = launches
        .next()
        .ok_or_else(|| anyhow::anyhow!("the diag plan names no server launch"))?
        .command;
    let client_cmd = launches.next().map(|launch| launch.command);

    let window_secs = args.timeout_secs.unwrap_or(NO_TIMER_SECS);
    let mut watch_dirs = vec![profiles_dir.clone()];
    watch_dirs.extend(client_log.iter().cloned());
    let watch = VerdictWatch::spawn(&rules, &watch_dirs, stop, Duration::from_secs(window_secs));

    let earlier_logs = modfoundry_diag::log_states(&profiles_dir);
    let server_stop = stop.clone();
    let server_profiles = profiles_dir.clone();
    host.launch_window_secs(window_secs);
    let server_thread = std::thread::spawn(move || {
        capture_command(&server_cmd, &server_profiles, window_secs, &server_stop)
    });

    let mut client = None;
    let mut ready_timed_out = false;
    let mut client_closed = false;
    let mut server_stopped = false;
    if let Some((cmd, log_dir)) = client_cmd.zip(client_log) {
        let run_over = || stop.is_stopped() || server_thread.is_finished();
        let give_up = StopSignal::new();
        let ready = wait_for_log_marker(
            &profiles_dir,
            &earlier_logs,
            SERVER_READY_MARKER,
            host.ready_timeout(),
            &give_up,
            || {
                if run_over() {
                    give_up.stop();
                }
                host.sleep(MARKER_POLL)
            },
        );
        host.ready_wait_result(ready);
        match ready {
            MarkerWait::Found if !run_over() => {
                let server_over = || server_thread.is_finished();
                match capture_client_run(args, window_secs, host, &cmd, &log_dir, stop, server_over)
                {
                    Ok(c) => {
                        // A client that ended by itself has no server to play on, and the run
                        // would otherwise wait on that server with no limit. A stop that lands
                        // as it ends is a stop, not the client closing.
                        let closed_by_itself = !c.report.stopped
                            && !c.report.timed_out
                            && !server_thread.is_finished()
                            && !stop.is_stopped();
                        if closed_by_itself {
                            client_closed = true;
                            server_stopped = true;
                            let _ = host.stop_client();
                            let _ = host.stop_server();
                        }
                        client = Some(ClientRunOutcome {
                            game_dir: dirs.game.clone(),
                            ..c
                        });
                    }
                    Err(e) => {
                        let _ = host.stop_client();
                        stop.stop();
                        let _ = host.stop_server();
                        let joined = server_thread.join();
                        let hint = joined.ok().and_then(Result::ok).and_then(|report| {
                            storage_flood_hint(count_storage_errors(&report.logs, &earlier_logs))
                        });
                        return Err(match hint {
                            Some(hint) => anyhow::anyhow!("{e:#}\nhint: {hint}"),
                            None => e,
                        });
                    }
                }
            }
            MarkerWait::TimedOut if !run_over() => {
                ready_timed_out = true;
                stop.stop();
                let _ = host.stop_server();
            }
            MarkerWait::Found | MarkerWait::TimedOut | MarkerWait::Stopped => {}
        }
    }

    let mut server = server_thread
        .join()
        .map_err(|_| anyhow::anyhow!("the diag server capture thread panicked"))??;

    // Joined before the stop state is read: a watcher that has not set it yet never will.
    drop(watch);
    if stop.is_stopped() {
        server.stopped = true;
        let stop_error = host.stop_client().err().map(|e| e.to_string());
        if let Some(c) = client.as_mut() {
            c.report.stopped = true;
            if c.client_stop_error.is_none() {
                c.client_stop_error = stop_error;
            }
        }
    }
    if !server_stopped {
        let _ = host.stop_server();
    }

    let hint = storage_flood_hint(count_storage_errors(&server.logs, &earlier_logs));
    let mut logs = server.logs.clone();
    if let Some(c) = &client {
        logs.extend(c.report.logs.iter().cloned());
    }
    let mut errors = errors_from_logs(&logs, &earlier_logs, Some(&args.source_root))?;
    known.mark(&mut errors);
    let verdict = judge_logs(&logs, &earlier_logs, &rules)?;
    Ok(DiagOutcome {
        profiles_dir,
        server,
        client,
        errors,
        ready_timed_out,
        hint,
        cleared,
        client_closed,
        verdict,
    })
}

/// Where a project's server launches keep their profiles and logs between runs.
pub fn project_profiles_dir(source_root: &Path) -> PathBuf {
    source_root.join("server").join("profiles")
}

/// Logs kept per kind in a project profiles folder; older ones are pruned.
const KEPT_LOGS_PER_KIND: usize = 10;

/// Keep the newest [`KEPT_LOGS_PER_KIND`] logs of each kind in `profiles_dir`.
fn prune_old_logs(profiles_dir: &Path) -> anyhow::Result<()> {
    let mut by_kind: HashMap<(String, String), Vec<PathBuf>> = HashMap::new();
    for log in find_logs(profiles_dir) {
        by_kind.entry(log_kind(&log)).or_default().push(log);
    }
    for mut logs in by_kind.into_values() {
        logs.sort_by_cached_key(|log| {
            std::cmp::Reverse(log.metadata().and_then(|m| m.modified()).ok())
        });
        // One at a time, so a log that cannot go does not keep the rest.
        for old in logs.iter().skip(KEPT_LOGS_PER_KIND) {
            if let Some(name) = old.file_name() {
                let mut txn =
                    WriteTxn::unrecorded(root_name("profiles", profiles_dir), profiles_dir);
                let _ = txn
                    .remove(name.to_string_lossy())
                    .and_then(|()| txn.finish().map(drop));
            }
        }
    }
    Ok(())
}

/// The part of a log's file name before its first `_`, and its extension,
/// lower-cased: one run writes one `DayZServer_*.RPT` and one `DayZServer_*.ADM`.
fn log_kind(log: &Path) -> (String, String) {
    let name = log.file_name().unwrap_or_default().to_string_lossy();
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('_').next().unwrap_or_default().to_owned();
    let ext = lower.rsplit_once('.').map_or("", |(_, ext)| ext).to_owned();
    (stem, ext)
}

/// Prunes the project's kept profiles folder when the run ends, however it ends.
pub(crate) struct PruneOnDrop(pub(crate) Option<PathBuf>);

impl Drop for PruneOnDrop {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            let _ = prune_old_logs(dir);
        }
    }
}

/// The prefix for the loose source: the built PBO's, else the source folder's name.
fn default_mod_prefix(mod_dir: &Path, source_root: &Path) -> anyhow::Result<String> {
    if let Some(prefix) = built_pbo_prefix(mod_dir) {
        if let Err(why) = pbo_prefix_fits_source(&prefix, source_root) {
            anyhow::bail!(
                "built PBO prefix \"{prefix}\" does not match the source: {why}. The build is \
                 out of date or used another --prefix; rebuild, or pass --mod-prefix"
            );
        }
        return Ok(prefix);
    }
    if !source_root.is_dir() {
        anyhow::bail!("source root {} is not a folder", source_root.display());
    }
    // Lexical, so a symlinked source root keeps its own name.
    let source = crate::script_host::normalized(&std::path::absolute(source_root)?);
    source
        .file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.strip_prefix('@').unwrap_or(name))
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "source root {} has no name; pass --mod-prefix",
                source_root.display()
            )
        })
}

/// Refuses a PBO `prefix` the source's own `files[]` entries do not bind: the engine would look
/// under `<prefix>/` and find nothing. A source with no `config.cpp`, or one the parser reported
/// issues for, has nothing to disagree with.
fn pbo_prefix_fits_source(prefix: &str, source_root: &Path) -> Result<(), String> {
    let Some(config) = crate::sources::find_child(source_root, "config.cpp", false) else {
        return Ok(());
    };
    let Ok(text) = std::fs::read_to_string(&config) else {
        return Ok(());
    };
    let parsed = modfoundry_config::parser::parse(&text);
    if !parsed.issues.is_empty() {
        return Ok(());
    }
    let dz = modfoundry_config::DayZConfig::from_file(&parsed.file);
    let wanted = format!("{}/", prefix.replace('\\', "/").to_lowercase());
    for module in dz.mods.iter().flat_map(|m| &m.script_modules) {
        for entry in &module.files {
            let normalized = entry
                .replace('\\', "/")
                .trim_start_matches('/')
                .trim_start_matches("./")
                .to_lowercase();
            if !normalized.starts_with(&wanted) {
                return Err(format!(
                    "files[] entry \"{entry}\" in module {} does not start with \"{prefix}/\"",
                    module.defs_name
                ));
            }
        }
    }
    Ok(())
}

/// The prefix of the only PBO in `<mod_dir>/addons`, without separators at its ends;
/// `None` when there is not exactly one or its header is unreadable.
fn built_pbo_prefix(mod_dir: &Path) -> Option<String> {
    let [pbo] = addon_pbos(mod_dir).try_into().ok()?;
    pbo_prefix(&pbo)
}

/// The prefixes of every readable PBO in `<mod_dir>/addons`.
pub(crate) fn pbo_prefixes(mod_dir: &Path) -> Vec<String> {
    addon_pbos(mod_dir)
        .iter()
        .filter_map(|pbo| pbo_prefix(pbo))
        .collect()
}

fn addon_pbos(mod_dir: &Path) -> Vec<PathBuf> {
    crate::script_index::find_child_dir(mod_dir, "addons")
        .map(|addons| crate::script_index::list_ext(&addons, "pbo"))
        .unwrap_or_default()
}

fn pbo_prefix(pbo: &Path) -> Option<String> {
    let (header, _) = modfoundry_pbo::read_header_file(pbo).ok()?;
    header
        .property("prefix")
        .map(|prefix| prefix.trim_matches(['\\', '/']))
        .filter(|prefix| !prefix.is_empty())
        .map(str::to_owned)
}

/// The Diag server's argv: a `DiagServer` profile plus `-nolauncher`.
fn diag_server_argv(stage: &DiagStage, port: u16) -> Vec<String> {
    let profile = ServerProfile {
        kind: LaunchKind::DiagServer,
        config: Some(PathBuf::from(stage.config_name())),
        mission: None,
        port: Some(port),
        profiles: Some(PathBuf::from(stage.profiles_arg())),
        mods: stage
            .mod_args_both()
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        server_mods: stage
            .mod_args_server()
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        ip: None,
        flags: LogFlags {
            do_logs: true,
            admin_log: true,
            freeze_check: true,
            file_patching: true,
            ..Default::default()
        },
    };
    let mut argv = profile.argv();
    argv.push("-nolauncher".into());
    argv
}

/// The Diag client's launch command: the staged `@Mod`, `-connect` to the local
/// server, the window args and `-nolauncher`.
fn client_launch(
    steam_root: &Path,
    diag_exe: &Path,
    stage: &DiagStage,
    args: &RunDiagArgs,
    port: u16,
) -> anyhow::Result<LaunchCommand> {
    let profile = ClientProfile {
        app_id: modfoundry_platform::DAYZ_APP_ID,
        mods: stage.mod_args_both(),
        connect: Some(Connect {
            host: "127.0.0.1".into(),
            game_port: port,
            query_port: None,
        }),
        profile_name: args.connect_name.clone(),
        extra: {
            let mut extra = if args.windowed {
                window_args(args.window, &[])
            } else {
                Vec::new()
            };
            extra.push("-nolauncher".into());
            extra
        },
        flags: LogFlags {
            do_logs: true,
            file_patching: true,
            ..Default::default()
        },
    };
    proton_command(steam_root, diag_exe, &profile.argv())
}

/// Capture the Diag client's log dir until it settles, then stay live until a
/// stop, the bounded window, or the client exits.
fn capture_client_run(
    args: &RunDiagArgs,
    window_secs: u64,
    host: &(impl DiagHost + ?Sized),
    cmd: &LaunchCommand,
    log_dir: &Path,
    stop: &StopSignal,
    server_over: impl Fn() -> bool,
) -> anyhow::Result<ClientRunOutcome> {
    let launched = Instant::now();
    let wait = ClientWait::new(window_secs, host.client_settle_secs());
    let mut report = capture_client(cmd, log_dir, &wait, stop)?;
    if !report.stopped && !report.timed_out {
        let deadline = launched + Duration::from_secs(window_secs);
        match wait_for_client_end(
            stop,
            deadline,
            || host.client_running() && !server_over(),
            || host.sleep(CLIENT_POLL_INTERVAL),
            || {},
        ) {
            ClientEnd::Stopped => report.stopped = true,
            ClientEnd::TimedOut => report.timed_out = true,
            ClientEnd::Exited => {
                // A client left behind has no server to play on, and would hold the run open.
                if host.client_running() {
                    let _ = host.stop_client();
                }
            }
        }
        report.logs = modfoundry_diag::find_logs(log_dir)
            .into_iter()
            .filter(|p| !report.before.contains(p))
            .collect();
    }
    let errors = errors_from_logs(&report.logs, &LogStates::new(), Some(&args.source_root))?;
    Ok(ClientRunOutcome {
        report,
        log_dir: log_dir.to_path_buf(),
        errors,
        client_stop_error: None,
        cleared: Vec::new(),
        links_kept: false,
        game_dir: PathBuf::new(),
        verdict: None,
    })
}

struct RealHost;

impl DiagHost for RealHost {
    fn client_running(&self) -> bool {
        modfoundry_platform::dayz_client_running()
    }

    fn server_running(&self) -> bool {
        modfoundry_platform::diag_server_running()
    }

    fn stop_client(&self) -> std::io::Result<usize> {
        modfoundry_platform::stop_diag_client()
    }

    fn stop_server(&self) -> std::io::Result<usize> {
        modfoundry_platform::stop_diag_server()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::diag_stage::{TestModRole, DIAG_CONFIG_NAME, DIAG_PROFILES_LINK};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-diagrun-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_exec(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    struct Fx {
        root: PathBuf,
        game: PathBuf,
        server: PathBuf,
        source: PathBuf,
        profiles: PathBuf,
        log_dir: PathBuf,
        record: PathBuf,
        built_mod: PathBuf,
    }

    fn stub_body(profiles: &Path, log_dir: &Path, record: &Path, rpt_extra: &str) -> String {
        format!(
            "#!/bin/sh\n\
             role=client\n\
             for a in \"$@\"; do [ \"$a\" = \"-server\" ] && role=server; done\n\
             {{\n\
             printf 'START\\n'\n\
             printf 'role:%s\\n' \"$role\"\n\
             for a in \"$@\"; do printf 'argv:%s\\n' \"$a\"; done\n\
             printf 'cwd:%s\\n' \"$(pwd)\"\n\
             printf 'SteamAppId:%s\\n' \"$SteamAppId\"\n\
             printf 'SteamGameId:%s\\n' \"$SteamGameId\"\n\
             printf 'STEAM_COMPAT_DATA_PATH:%s\\n' \"$STEAM_COMPAT_DATA_PATH\"\n\
             printf 'STEAM_COMPAT_CLIENT_INSTALL_PATH:%s\\n' \"$STEAM_COMPAT_CLIENT_INSTALL_PATH\"\n\
             printf 'LS_BEGIN\\n'\n\
             ls -1\n\
             printf 'LS_END\\n'\n\
             }} >> '{rec}'\n\
             if [ \"$role\" = server ]; then\n\
             printf '[CE][Hive] :: Init sequence finished.\\r\\nPlayer connect enabled\\r\\n{rpt_extra}' > '{p}/DayZDiag_x64_1.RPT'\n\
             printf ' SCRIPT    (E): @\"source/Scripts/3_Game/Broken.c,5\": boom\\r\\n' > '{p}/script_1.log'\n\
             else\n\
             printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{l}/DayZDiag_x64_1.RPT'\n\
             fi\n\
             sleep 3\n",
            rec = record.display(),
            p = profiles.display(),
            l = log_dir.display(),
        )
    }

    fn fixture(label: &str) -> Fx {
        fixture_with(label, "")
    }

    fn fixture_with(label: &str, rpt_extra: &str) -> Fx {
        let root = tmp(label);
        let game = root.join("steamapps/common/DayZ");
        let server = root.join("server");
        let source = root.join("source");
        let profiles = root.join("profiles");
        let built_mod = root.join("build/@X");
        let log_dir =
            root.join("steamapps/compatdata/221100/pfx/drive_c/users/steamuser/AppData/Local/DayZ");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join(DIAG_EXE), b"").unwrap();
        std::fs::create_dir_all(server.join("mpmissions")).unwrap();
        std::fs::write(
            server.join("serverDZ.cfg"),
            "BattlEye = 1;\nverifySignatures = 2;\n",
        )
        .unwrap();
        std::fs::create_dir_all(source.join("Scripts/3_Game")).unwrap();
        std::fs::write(source.join("Scripts/3_Game/Broken.c"), "class X {}\n").unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::create_dir_all(&built_mod).unwrap();
        std::fs::create_dir_all(&log_dir).unwrap();
        let record = root.join("record.txt");
        let body = stub_body(&profiles, &log_dir, &record, rpt_extra);
        let proton = root.join("steamapps/common/Proton - Experimental");
        std::fs::create_dir_all(&proton).unwrap();
        write_exec(&proton.join("proton"), &body);
        let runtime = root.join("steamapps/common/SteamLinuxRuntime_sniper");
        std::fs::create_dir_all(&runtime).unwrap();
        write_exec(&runtime.join("run"), &body);
        Fx {
            root,
            game,
            server,
            source,
            profiles,
            log_dir,
            record,
            built_mod,
        }
    }

    fn diag_args(fx: &Fx) -> RunDiagArgs {
        RunDiagArgs {
            mod_dir: fx.built_mod.clone(),
            source_root: fx.source.clone(),
            config: None,
            set: None,
            mod_prefix: None,
            game_dir: Some(fx.game.clone()),
            server_dir: Some(fx.server.clone()),
            steam_dir: Some(fx.root.clone()),
            profiles_dir: Some(fx.profiles.clone()),
            port: None,
            client: true,
            connect_name: None,
            window: None,
            timeout_secs: Some(2),
            run_id: None,
            test_mods: Vec::new(),
            windowed: true,
            expect_lines: Vec::new(),
            fail_lines: Vec::new(),
            allow_errors: Vec::new(),
        }
    }

    fn test_mod(fx: &Fx, name: &str, role: TestModRole) -> TestModArg {
        let path = fx.root.join("testmods").join(name);
        std::fs::create_dir_all(&path).unwrap();
        TestModArg {
            path,
            link_name: name.to_string(),
            role,
        }
    }

    fn dirs(fx: &Fx) -> DiagDirs {
        DiagDirs {
            game: fx.game.clone(),
            exe: fx.game.join(DIAG_EXE),
            server: fx.server.clone(),
            steam: fx.root.clone(),
        }
    }

    struct TestHost {
        client_running: AtomicBool,
        server_running: AtomicBool,
        stopped_client: AtomicUsize,
        stopped_server: AtomicUsize,
        window_secs: AtomicU64,
        ready_timeout_ms: AtomicU64,
        ready_result: Mutex<Option<MarkerWait>>,
        stop_on_ready: Option<StopSignal>,
        /// Set the stop signal the first time a poll finds no client, once the server is ready.
        stop_when_client_gone: Option<StopSignal>,
        client_up_on_ready: bool,
        client_settle_secs: u64,
        /// Written when the run stops the server, for a stub that waits on it.
        server_stop_file: Option<PathBuf>,
    }

    impl Default for TestHost {
        fn default() -> Self {
            Self {
                client_running: AtomicBool::new(false),
                server_running: AtomicBool::new(false),
                stopped_client: AtomicUsize::new(0),
                stopped_server: AtomicUsize::new(0),
                window_secs: AtomicU64::new(0),
                ready_timeout_ms: AtomicU64::new(SERVER_READY_TIMEOUT_SECS * 1000),
                ready_result: Mutex::new(None),
                stop_on_ready: None,
                stop_when_client_gone: None,
                client_up_on_ready: false,
                client_settle_secs: 1,
                server_stop_file: None,
            }
        }
    }

    impl TestHost {
        fn ready_result(&self) -> Option<MarkerWait> {
            *self.ready_result.lock().unwrap()
        }
    }

    impl DiagHost for TestHost {
        fn client_running(&self) -> bool {
            let running = self.client_running.load(Ordering::SeqCst);
            if !running && self.ready_result().is_some() {
                if let Some(stop) = &self.stop_when_client_gone {
                    stop.stop();
                }
            }
            running
        }

        fn server_running(&self) -> bool {
            self.server_running.load(Ordering::SeqCst)
        }

        fn stop_client(&self) -> std::io::Result<usize> {
            Ok(self.stopped_client.fetch_add(1, Ordering::SeqCst) + 1)
        }

        fn stop_server(&self) -> std::io::Result<usize> {
            if let Some(path) = &self.server_stop_file {
                std::fs::write(path, b"stop")?;
            }
            Ok(self.stopped_server.fetch_add(1, Ordering::SeqCst) + 1)
        }

        fn sleep(&self, _: Duration) {}

        fn client_settle_secs(&self) -> u64 {
            self.client_settle_secs
        }

        fn ready_timeout(&self) -> Duration {
            Duration::from_millis(self.ready_timeout_ms.load(Ordering::SeqCst))
        }

        fn launch_window_secs(&self, secs: u64) {
            self.window_secs.store(secs, Ordering::SeqCst);
        }

        fn ready_wait_result(&self, result: MarkerWait) {
            if result == MarkerWait::Found {
                if let Some(stop) = &self.stop_on_ready {
                    stop.stop();
                }
                if self.client_up_on_ready {
                    self.client_running.store(true, Ordering::SeqCst);
                }
            }
            *self.ready_result.lock().unwrap() = Some(result);
        }
    }

    fn record(fx: &Fx) -> String {
        std::fs::read_to_string(&fx.record).unwrap_or_default()
    }

    fn blocks(record: &str) -> Vec<&str> {
        record
            .split("START")
            .filter(|b| !b.trim().is_empty())
            .collect()
    }

    fn roles(record: &str) -> Vec<String> {
        record
            .lines()
            .filter_map(|l| l.strip_prefix("role:"))
            .map(str::to_owned)
            .collect()
    }

    fn block_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
        block.lines().find_map(|l| l.strip_prefix(key))
    }

    fn block_argv(block: &str) -> Vec<String> {
        block
            .lines()
            .filter_map(|l| l.strip_prefix("argv:"))
            .map(str::to_owned)
            .collect()
    }

    fn block_ls(block: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut inside = false;
        for l in block.lines() {
            if l == "LS_BEGIN" {
                inside = true;
            } else if l == "LS_END" {
                inside = false;
            } else if inside {
                out.push(l.to_string());
            }
        }
        out
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_plan_writes_nothing_and_names_the_staging_the_run_makes() {
        use crate::launch_plan::{LaunchRole, StageStep};

        let fx = fixture("plan");
        let args = diag_args(&fx);
        let game_before = entries(&fx.game);
        let source_before = entries(&fx.source);

        let plan = plan_diag(&args).unwrap();

        assert_eq!(entries(&fx.game), game_before, "a plan writes nothing");
        assert_eq!(
            entries(&fx.source),
            source_before,
            "a plan makes no project file"
        );
        assert!(
            !fx.source.join("server").exists(),
            "a plan creates no project folders"
        );
        assert!(plan.leftovers.is_empty(), "{:?}", plan.leftovers);

        let roles: Vec<LaunchRole> = plan.launches.iter().map(|launch| launch.role).collect();
        assert_eq!(roles, [LaunchRole::Server, LaunchRole::Client]);

        let config = fx.game.join(DIAG_CONFIG_NAME);
        let patched = "BattlEye = 0;\nverifySignatures = 0;\nallowFilePatching = 1;\n";
        assert!(
            plan.steps.iter().any(|step| matches!(
                step,
                StageStep::Write { path, contents, bytes }
                    if path == &config
                        && contents.as_slice() == patched.as_bytes()
                        && *bytes == patched.len()
            )),
            "{:?}",
            plan.steps
        );
        for step in [
            StageStep::Link {
                link: fx.game.join("mpmissions"),
                target: fx.server.join("mpmissions"),
            },
            StageStep::Link {
                link: fx.game.join(DIAG_PROFILES_LINK),
                target: fx.profiles.clone(),
            },
            StageStep::Link {
                link: fx.game.join("source"),
                target: fx.source.clone(),
            },
            StageStep::Link {
                link: fx.game.join("@X"),
                target: fx.built_mod.clone(),
            },
        ] {
            assert!(
                plan.steps.contains(&step),
                "missing {step:?}: {:?}",
                plan.steps
            );
        }
        let kept = fx.source.join("server").join(crate::run_ops::SERVER_CONFIG);
        assert!(
            plan.project.iter().any(|step| matches!(
                step,
                StageStep::Write { path, contents, .. }
                    if path == &kept && contents.as_slice() == b"BattlEye = 1;\nverifySignatures = 2;\n"
            )),
            "{:?}",
            plan.project
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_fresh_project_plans_its_profiles_folder_and_its_config() {
        use crate::launch_plan::StageStep;

        let fx = fixture("plan-fresh");
        let mut args = diag_args(&fx);
        args.profiles_dir = None;
        let server = fx.source.join("server");

        let plan = plan_diag(&args).unwrap();

        assert!(
            plan.project.contains(&StageStep::CreateDir {
                path: server.join("profiles")
            }),
            "{:?}",
            plan.project
        );
        assert!(
            plan.project.iter().any(|step| matches!(
                step,
                StageStep::Write { path, .. } if path == &server.join(crate::run_ops::SERVER_CONFIG)
            )),
            "{:?}",
            plan.project
        );
        assert!(!server.exists(), "a plan creates no project folders");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn stale_staging_is_cleared_by_the_plan_and_a_gone_path_is_planned_as_free() {
        use crate::launch_plan::StageStep;
        use modfoundry_platform::stage::journal_path;

        let fx = fixture("plan-stale");
        let args = diag_args(&fx);
        let config = fx.game.join(DIAG_CONFIG_NAME);
        let link = fx.game.join(DIAG_PROFILES_LINK);
        std::fs::write(&config, "stale").unwrap();
        let elsewhere = fx.root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        let plan = plan_diag(&args).unwrap();

        assert_eq!(plan.leftovers, [config.clone(), link.clone()]);
        assert!(
            plan.steps.contains(&StageStep::Remove {
                path: config.clone()
            }),
            "{:?}",
            plan.steps
        );
        assert!(
            plan.steps.contains(&StageStep::Relink {
                link: link.clone(),
                target: fx.profiles.clone()
            }),
            "{:?}",
            plan.steps
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "stale",
            "a plan clears nothing"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), elsewhere);

        let gone: Vec<PathBuf> = [&config, &link]
            .into_iter()
            .map(|path| journal_path(path).unwrap())
            .collect();
        let plan = plan_diag_in(&args, &dirs(&fx), &gone).unwrap();

        assert!(plan.leftovers.is_empty(), "{:?}", plan.leftovers);
        assert!(
            !plan.steps.contains(&StageStep::Remove {
                path: config.clone()
            }),
            "a cleared path needs no removal: {:?}",
            plan.steps
        );
        assert!(
            plan.steps.contains(&StageStep::Link {
                link,
                target: fx.profiles.clone()
            }),
            "{:?}",
            plan.steps
        );
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, StageStep::Write { path, .. } if path == &config)),
            "{:?}",
            plan.steps
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_stale_file_where_the_profiles_link_goes_is_removed_before_the_link() {
        use crate::launch_plan::StageStep;

        let fx = fixture("plan-stale-file");
        let args = diag_args(&fx);
        let link = fx.game.join(DIAG_PROFILES_LINK);
        std::fs::write(&link, "stale").unwrap();

        let plan = plan_diag(&args).unwrap();

        let removed = plan
            .steps
            .iter()
            .position(|step| matches!(step, StageStep::Remove { path } if path == &link))
            .unwrap_or_else(|| panic!("the stale file is not removed: {:?}", plan.steps));
        let linked = plan
            .steps
            .iter()
            .position(|step| matches!(step, StageStep::Link { link: at, .. } if at == &link))
            .unwrap_or_else(|| panic!("the profiles link is not made: {:?}", plan.steps));
        assert!(
            removed < linked,
            "the removal comes first: {:?}",
            plan.steps
        );
        assert_eq!(plan.leftovers, std::slice::from_ref(&link));
        assert_eq!(
            std::fs::read_to_string(&link).unwrap(),
            "stale",
            "a plan clears nothing"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_gone_parent_folder_is_planned_again_for_a_nested_prefix() {
        use crate::launch_plan::StageStep;
        use modfoundry_platform::stage::journal_path;

        let fx = fixture("plan-gone-parent");
        let mut args = diag_args(&fx);
        args.mod_prefix = Some("JM/CF".into());
        let parent = fx.game.join("JM");
        std::fs::create_dir(&parent).unwrap();
        let gone = [journal_path(&parent).unwrap()];

        let plan = plan_diag_in(&args, &dirs(&fx), &gone).unwrap();

        assert!(
            plan.steps.contains(&StageStep::CreateDir {
                path: parent.clone()
            }),
            "{:?}",
            plan.steps
        );
        assert!(
            plan.steps.contains(&StageStep::Link {
                link: parent.join("CF"),
                target: fx.source.clone()
            }),
            "{:?}",
            plan.steps
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn the_plan_names_the_commands_the_run_starts() {
        let fx = fixture("plan-commands");
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(2);

        let plan = plan_diag(&args).unwrap();
        run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let rec = record(&fx);
        let started = blocks(&rec);
        assert_eq!(started.len(), plan.launches.len(), "{rec}");
        for (block, launch) in started.iter().zip(&plan.launches) {
            let planned: Vec<String> = launch
                .command
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                block_argv(block),
                planned,
                "the run must start what the plan names"
            );
        }

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_preview_lists_a_dead_runs_leftover_and_plans_its_path_as_free() {
        use crate::launch_plan::StageStep;

        let fx = fixture("plan-dead-preview");
        let config = dead_leftover(&fx.game, DIAG_CONFIG_NAME);

        let plan = plan_diag(&diag_args(&fx)).unwrap();

        assert_eq!(plan.leftovers, std::slice::from_ref(&config));
        assert!(
            !plan.steps.contains(&StageStep::Remove {
                path: config.clone()
            }),
            "a cleared path needs no removal: {:?}",
            plan.steps
        );
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, StageStep::Write { path, .. } if path == &config)),
            "{:?}",
            plan.steps
        );
        assert!(config.is_file(), "a plan clears nothing");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_relative_source_root_records_an_absolute_log_folder() {
        let fx = fixture("relative-log-dir");
        let mut args = diag_args(&fx);
        args.source_root = PathBuf::from("MyMod");
        args.profiles_dir = None;
        args.client = false;
        assert_eq!(
            diag_log_dirs(&args),
            [std::path::absolute("MyMod/server/profiles").unwrap()]
        );
    }

    #[test]
    fn old_run_logs_are_pruned_to_the_last_ten() {
        let profiles = tmp("prune");
        let base = std::time::SystemTime::now() - Duration::from_secs(100_000);
        let cases = [
            ("script", "log", 12, 10),
            ("DayZServer", "RPT", 12, 10),
            ("DayZServer", "ADM", 3, 3),
        ];
        for (kind, ext, written, _) in cases {
            for i in 0..written {
                let file = profiles.join(format!("{kind}_{i:02}.{ext}"));
                std::fs::write(&file, b"log\r\n").unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&file)
                    .unwrap()
                    .set_modified(base + Duration::from_secs(i))
                    .unwrap();
            }
        }

        prune_old_logs(&profiles).unwrap();

        for (kind, ext, written, kept) in cases {
            let remaining: Vec<String> = entries(&profiles)
                .into_iter()
                .filter(|n| n.starts_with(kind) && n.ends_with(ext))
                .collect();
            let newest: Vec<String> = (written - kept..written)
                .map(|i| format!("{kind}_{i:02}.{ext}"))
                .collect();
            assert_eq!(remaining, newest, "{kind} logs");
        }
        let _ = std::fs::remove_dir_all(&profiles);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn without_a_profiles_dir_the_run_uses_the_project_server_folder() {
        let fx = fixture("projectdir");
        let mut args = diag_args(&fx);
        args.profiles_dir = None;
        args.client = false;
        args.timeout_secs = Some(1);

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let project = fx.source.join("server");
        assert_eq!(outcome.profiles_dir, project.join("profiles"));
        assert!(project.join("profiles").is_dir());
        assert!(project.join("serverDZ.cfg").is_file());
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    fn process_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn wait_for_roles(fx: &Fx, want: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && roles(&record(fx)).len() < want {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            roles(&record(fx)).len(),
            want,
            "stubs did not reach {want} invocation(s): {}",
            record(fx)
        );
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn server_stub_gets_the_diag_flags_over_staged_artefacts_that_are_removed_after() {
        let fx = fixture("server");
        let mut args = diag_args(&fx);
        args.client = false;
        args.timeout_secs = Some(1);
        let host = TestHost::default();

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        let rec = record(&fx);
        let b = blocks(&rec);
        assert_eq!(
            b.len(),
            1,
            "no client stub may run with client:false: {rec}"
        );
        assert_eq!(block_field(b[0], "role:"), Some("server"));

        let argv = block_argv(b[0]);
        assert_eq!(argv[3], fx.game.join(DIAG_EXE).to_str().unwrap());
        assert_eq!(
            argv[4..],
            [
                "-config=serverDZ_modfoundry.cfg",
                "-mod=@X",
                "-serverMod=",
                "-profiles=mf-diag-profiles",
                "-port=2302",
                "-server",
                "-filePatching",
                "-freezecheck",
                "-adminlog",
                "-dologs",
                "-nolauncher",
            ]
        );

        let cwd = std::fs::canonicalize(&fx.game).unwrap();
        assert_eq!(
            block_field(b[0], "cwd:"),
            Some(cwd.to_str().unwrap()),
            "the diag server must run with cwd = the client install"
        );
        assert_eq!(block_field(b[0], "SteamAppId:"), Some("221100"));
        assert_eq!(block_field(b[0], "SteamGameId:"), Some("221100"));
        assert!(block_field(b[0], "STEAM_COMPAT_DATA_PATH:").is_some());
        assert!(block_field(b[0], "STEAM_COMPAT_CLIENT_INSTALL_PATH:").is_some());

        let ls = block_ls(b[0]);
        for name in [
            "mpmissions",
            "source",
            "@X",
            "mf-diag-profiles",
            "serverDZ_modfoundry.cfg",
        ] {
            assert!(
                ls.iter().any(|e| e == name),
                "staged {name} missing at server start: {ls:?}"
            );
        }

        assert_eq!(
            entries(&fx.game),
            vec![DIAG_EXE.to_string()],
            "the guard must remove every staged artefact"
        );
        assert_eq!(host.stopped_server.load(Ordering::SeqCst), 1);
        assert_eq!(host.stopped_client.load(Ordering::SeqCst), 0);

        let mapped = fx.source.join("Scripts/3_Game/Broken.c");
        assert!(
            outcome
                .errors
                .iter()
                .any(|e| e.file.as_deref() == Some(mapped.as_path())),
            "the planted SCRIPT (E) line must map to {mapped:?}: {:?}",
            outcome.errors
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn client_starts_after_the_server_marker_with_connect_and_window_args() {
        let fx = fixture("client");
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(2);
        let host = TestHost::default();

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        let rec = record(&fx);
        let b = blocks(&rec);
        assert_eq!(b.len(), 2, "server then client: {rec}");
        assert_eq!(block_field(b[0], "role:"), Some("server"));
        assert_eq!(block_field(b[1], "role:"), Some("client"));

        let argv = block_argv(b[1]);
        assert_eq!(argv[3], fx.game.join(DIAG_EXE).to_str().unwrap());
        assert_eq!(
            argv[4..],
            [
                "-mod=@X",
                "-connect=127.0.0.1:2302",
                "-window",
                "-x=1280",
                "-y=720",
                "-nolauncher",
                "-filePatching",
                "-dologs",
            ]
        );
        assert!(
            block_ls(b[1]).iter().any(|e| e == "mf-diag-profiles"),
            "staging must still exist when the client starts"
        );

        let client = outcome.client.expect("client:true must start the client");
        assert!(!client.report.logs.is_empty());
        assert_eq!(host.stopped_server.load(Ordering::SeqCst), 1);
        assert_eq!(host.stopped_client.load(Ordering::SeqCst), 0);
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);
        assert!(
            fx.log_dir.is_dir(),
            "the prefix log dir is not ours to remove"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_server_that_dies_before_the_marker_ends_the_run_at_once_without_a_client() {
        let fx = fixture("earlydeath");
        let dying = format!(
            "#!/bin/sh\nprintf 'START\\nrole:server\\n' >> '{rec}'\n\
             printf ' SCRIPT    (E): @\"source/Scripts/3_Game/Broken.c,5\": boom\\r\\n' > '{p}/script_1.log'\n",
            rec = fx.record.display(),
            p = fx.profiles.display(),
        );
        let runtime = fx
            .root
            .join("steamapps/common/SteamLinuxRuntime_sniper/run");
        write_exec(&runtime, &dying);
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(60);
        let started = Instant::now();

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "a dead server must not wait out the window: {:?}",
            started.elapsed()
        );
        assert!(outcome.client.is_none(), "no client for a dead server");
        assert!(
            outcome.errors.iter().any(|e| e.file.is_some()),
            "the server's error must still be mapped: {:?}",
            outcome.errors
        );
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_missing_diag_exe_fails_before_anything_is_staged_or_started() {
        let fx = fixture("noexe");
        std::fs::remove_file(fx.game.join(DIAG_EXE)).unwrap();
        let host = TestHost::default();

        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &host)
            .unwrap_err()
            .to_string();
        assert!(err.contains("DayZDiag_x64.exe"), "unhelpful: {err}");
        assert!(!fx.record.exists(), "no process may start: {err}");
        assert!(entries(&fx.game).is_empty());

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_staging_conflict_fails_with_nothing_staged_and_no_process() {
        let fx = fixture("conflict");
        std::fs::create_dir_all(fx.game.join("source")).unwrap();
        let host = TestHost::default();

        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &host)
            .unwrap_err()
            .to_string();
        assert!(err.contains("source"), "unhelpful: {err}");
        assert!(!fx.record.exists(), "no process may start: {err}");
        assert_eq!(
            entries(&fx.game),
            vec![DIAG_EXE.to_string(), "source".into()]
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_running_client_or_diag_server_is_refused_before_staging() {
        let fx = fixture("running");
        let client = TestHost::default();
        client.client_running.store(true, Ordering::SeqCst);
        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &client)
            .unwrap_err()
            .to_string();
        assert!(err.contains("stop it first"), "unhelpful: {err}");
        assert!(!fx.record.exists());

        let server = TestHost::default();
        server.server_running.store(true, Ordering::SeqCst);
        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &server)
            .unwrap_err()
            .to_string();
        assert!(err.contains("stop it first"), "unhelpful: {err}");
        assert!(!fx.record.exists());
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_stop_ends_both_captures_and_leaves_no_staging() {
        let fx = fixture("stop");
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(30);
        args.test_mods = vec![
            test_mod(&fx, "a", TestModRole::Both),
            test_mod(&fx, "s", TestModRole::ServerOnly),
        ];
        let host = Arc::new(TestHost::default());
        let stop = StopSignal::new();

        let h = Arc::clone(&host);
        let s = stop.clone();
        let handle = std::thread::spawn(move || run_diag_with(&args, &s, &*h));

        wait_for_roles(&fx, 2);
        stop.stop();
        let outcome = handle.join().unwrap().unwrap();

        assert!(outcome.server.stopped);
        assert!(outcome.client.as_ref().unwrap().report.stopped);
        assert_eq!(host.stopped_client.load(Ordering::SeqCst), 1);
        assert_eq!(host.stopped_server.load(Ordering::SeqCst), 1);
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    /// A Diag server stub whose client half exits 4 without a log, and whose server half
    /// prints the ready marker plus `rpt_extra` into the capture folder and sleeps 3 s.
    fn failing_client_server(fx: &Fx, rpt_extra: &str) -> String {
        format!(
            "#!/bin/sh\n\
             role=client\n\
             for a in \"$@\"; do [ \"$a\" = \"-server\" ] && role=server; done\n\
             if [ \"$role\" = client ]; then\n\
             printf 'wine: cannot find DayZDiag_x64.exe\\n' >&2\n\
             exit 4\n\
             fi\n\
             printf 'START\\nrole:server\\npid:%s\\n' \"$$\" >> '{rec}'\n\
             printf 'Player connect enabled\\r\\n{rpt_extra}' > '{p}/DayZDiag_x64_1.RPT'\n\
             sleep 3\n",
            rec = fx.record.display(),
            p = fx.profiles.display(),
        )
    }

    #[test]
    fn a_failing_client_stops_and_joins_the_server_before_returning() {
        let fx = fixture("clientfail");
        let server_body = failing_client_server(&fx, "");
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &server_body,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(60);
        let host = TestHost::default();

        let err = run_diag_with(&args, &StopSignal::new(), &host).unwrap_err();

        assert!(
            err.to_string()
                .contains("exited with code 4 before writing a log"),
            "the client capture must fail on its launcher: {err}"
        );
        assert_eq!(
            host.stopped_server.load(Ordering::SeqCst),
            1,
            "a failing client must stop the server"
        );
        assert_eq!(
            host.stopped_client.load(Ordering::SeqCst),
            1,
            "the client processes must be stopped when its capture fails"
        );
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);
        let rec = record(&fx);
        let server = blocks(&rec)[0];
        let pid = block_field(server, "pid:")
            .expect("the server stub records its own pid")
            .parse::<u32>()
            .unwrap();
        assert!(
            !process_alive(pid),
            "the server must be joined before the run returns: {rec}"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_stop_as_the_marker_appears_launches_no_client() {
        let fx = fixture("stopatmarker");
        let args = diag_args(&fx);
        let stop = StopSignal::new();
        let host = TestHost {
            stop_on_ready: Some(stop.clone()),
            ..TestHost::default()
        };

        let outcome = run_diag_with(&args, &stop, &host).unwrap();

        assert!(outcome.client.is_none(), "a stop must not start the client");
        assert_eq!(roles(&record(&fx)), ["server"], "no client may run");
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_failing_client_still_reports_the_storage_hint() {
        let fx = fixture("clientfailhint");
        let server_body =
            failing_client_server(&fx, &"Scripted variables corrupted\\r\\n".repeat(100));
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &server_body,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(60);

        let err = run_diag_with(&args, &StopSignal::new(), &TestHost::default())
            .unwrap_err()
            .to_string();

        for text in [
            "exited with code 4 before writing a log",
            "hint: the server logged 100",
        ] {
            assert!(err.contains(text), "{text} missing: {err}");
        }
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn a_diag_run_with_a_flood_of_storage_errors_gives_the_hint() {
        let fx = fixture_with(
            "storagehint",
            &"Scripted variables corrupted\\r\\n".repeat(100),
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(2);

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let hint = outcome.hint.clone().expect("a flood of storage errors");
        assert!(hint.contains("100"), "{hint}");
        assert!(hint.contains("Scripted variables corrupted"), "{hint}");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    /// A Diag stub whose client writes its log and exits at once, and whose server reports the
    /// marker, records its pid, then waits for `stop_file` (or 60 s) before exiting.
    fn closing_client_server(fx: &Fx, stop_file: &Path) -> String {
        format!(
            "#!/bin/sh\n\
             role=client\n\
             for a in \"$@\"; do [ \"$a\" = \"-server\" ] && role=server; done\n\
             printf 'START\\nrole:%s\\npid:%s\\n' \"$role\" \"$$\" >> '{rec}'\n\
             if [ \"$role\" = server ]; then\n\
             printf 'Player connect enabled\\r\\n' > '{p}/DayZDiag_x64_1.RPT'\n\
             n=0\n\
             while [ \"$n\" -lt 1200 ] && [ ! -f '{stop}' ]; do sleep 0.05; n=$((n + 1)); done\n\
             else\n\
             printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{l}/DayZDiag_x64_1.RPT'\n\
             fi\n",
            rec = fx.record.display(),
            p = fx.profiles.display(),
            l = fx.log_dir.display(),
            stop = stop_file.display(),
        )
    }

    #[test]
    fn a_client_that_closes_ends_the_run_and_stops_the_server() {
        let fx = fixture("clientclosed");
        let stop_file = fx.root.join("stop-server");
        let body = closing_client_server(&fx, &stop_file);
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &body,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = None;
        let host = Arc::new(TestHost {
            server_stop_file: Some(stop_file.clone()),
            ..TestHost::default()
        });

        let (send, end) = std::sync::mpsc::channel();
        let h = Arc::clone(&host);
        std::thread::spawn(move || {
            let outcome = run_diag_with(&args, &StopSignal::new(), &*h);
            let _ = send.send(outcome.map(|outcome| outcome.client_closed));
        });
        let bound = Duration::from_secs(20);
        let client_closed = match end.recv_timeout(bound) {
            Ok(outcome) => outcome.unwrap(),
            Err(_) => {
                // Release the stub so a failing run leaves no process behind, then fail.
                let _ = std::fs::write(&stop_file, b"stop");
                panic!(
                    "the run did not end within {} s of the client closing",
                    bound.as_secs()
                );
            }
        };

        assert!(
            client_closed,
            "the outcome must say the client closed on its own"
        );
        assert_eq!(
            host.stopped_server.load(Ordering::SeqCst),
            1,
            "the server must be stopped once the client closed"
        );
        assert_eq!(
            host.stopped_client.load(Ordering::SeqCst),
            1,
            "the client's Proton wrappers outlive the game and must be stopped too"
        );
        let rec = record(&fx);
        let pid = block_field(blocks(&rec)[0], "pid:")
            .expect("the server stub records its own pid")
            .parse::<u32>()
            .unwrap();
        assert!(
            !process_alive(pid),
            "the server must be joined before the run returns: {rec}"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_stop_that_lands_as_the_client_ends_is_reported_as_a_stop() {
        let fx = fixture("clientstop");
        let stop_file = fx.root.join("stop-server");
        let body = closing_client_server(&fx, &stop_file);
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &body,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = None;
        let stop = StopSignal::new();
        let host = Arc::new(TestHost {
            server_stop_file: Some(stop_file.clone()),
            stop_when_client_gone: Some(stop.clone()),
            ..TestHost::default()
        });

        let (send, end) = std::sync::mpsc::channel();
        let h = Arc::clone(&host);
        let s = stop.clone();
        std::thread::spawn(move || {
            let outcome = run_diag_with(&args, &s, &*h);
            let _ = send.send(outcome.map(|outcome| outcome.client_closed));
        });
        let bound = Duration::from_secs(20);
        let client_closed = match end.recv_timeout(bound) {
            Ok(outcome) => outcome.unwrap(),
            Err(_) => {
                // Release the stub so a failing run leaves no process behind, then fail.
                let _ = std::fs::write(&stop_file, b"stop");
                panic!(
                    "the run did not end within {} s of the stop",
                    bound.as_secs()
                );
            }
        };

        assert!(
            !client_closed,
            "a stop that lands as the client ends must not read as the client closing itself"
        );
        assert_eq!(
            host.stopped_server.load(Ordering::SeqCst),
            1,
            "the server must be stopped once by the run"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_server_that_dies_after_the_marker_ends_the_run_and_closes_the_client() {
        let fx = fixture("diesaftermarker");
        let run = fx
            .root
            .join("steamapps/common/SteamLinuxRuntime_sniper/run");
        let body = std::fs::read_to_string(&run)
            .unwrap()
            .replace("sleep 3", "sleep 0.2");
        write_exec(&run, &body);
        let mut args = diag_args(&fx);
        args.timeout_secs = None;
        let host = TestHost {
            client_up_on_ready: true,
            ..TestHost::default()
        };

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        assert!(outcome.client.is_some());
        assert!(
            !outcome.client_closed,
            "a server that died first must not read as the client closing"
        );
        assert_eq!(host.stopped_client.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn the_server_loads_both_test_mods_then_the_project_mod_and_the_server_only_ones() {
        let fx = fixture("testmods-server");
        let mut args = diag_args(&fx);
        args.client = false;
        args.timeout_secs = Some(1);
        args.test_mods = vec![
            test_mod(&fx, "a", TestModRole::Both),
            test_mod(&fx, "b", TestModRole::Both),
            test_mod(&fx, "s", TestModRole::ServerOnly),
        ];

        run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let rec = record(&fx);
        let b = blocks(&rec);
        assert_eq!(b.len(), 1, "{rec}");
        let argv = block_argv(b[0]);
        assert!(argv.iter().any(|a| a == "-mod=@a;@b;@X"), "{argv:?}");
        assert!(argv.iter().any(|a| a == "-serverMod=@s"), "{argv:?}");
        let ls = block_ls(b[0]);
        for name in ["@a", "@b", "@s"] {
            assert!(ls.iter().any(|e| e == name), "{name} not staged: {ls:?}");
        }
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn the_client_loads_both_test_mods_but_no_server_only_one() {
        let fx = fixture("testmods-client");
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(2);
        args.test_mods = vec![
            test_mod(&fx, "a", TestModRole::Both),
            test_mod(&fx, "b", TestModRole::Both),
            test_mod(&fx, "s", TestModRole::ServerOnly),
        ];

        run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let rec = record(&fx);
        let b = blocks(&rec);
        assert_eq!(b.len(), 2);
        let argv = block_argv(b[1]);
        assert_eq!(block_field(b[1], "role:"), Some("client"));
        assert!(argv.iter().any(|a| a == "-mod=@a;@b;@X"), "{argv:?}");
        assert!(
            !argv.iter().any(|a| a.starts_with("-serverMod")),
            "no server-only mod may reach the client: {argv:?}"
        );
        assert!(block_ls(b[1]).iter().any(|e| e == "@a"));
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn an_unbounded_run_ends_on_stop_and_reports_no_timeout() {
        let fx = fixture("unbounded");
        let mut args = diag_args(&fx);
        args.timeout_secs = None;
        let host = Arc::new(TestHost::default());
        let stop = StopSignal::new();

        let h = Arc::clone(&host);
        let s = stop.clone();
        let handle = std::thread::spawn(move || run_diag_with(&args, &s, &*h));

        wait_for_roles(&fx, 2);
        stop.stop();
        let outcome = handle.join().unwrap().unwrap();

        assert!(outcome.server.stopped, "{:?}", outcome.server);
        assert!(!outcome.server.timed_out, "{:?}", outcome.server);
        let client = outcome.client.expect("the client starts before the stop");
        assert!(client.report.stopped);
        assert!(!client.report.timed_out);
        assert_eq!(host.stopped_client.load(Ordering::SeqCst), 1);
        assert_eq!(host.stopped_server.load(Ordering::SeqCst), 1);
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);
        assert_eq!(host.window_secs.load(Ordering::SeqCst), NO_TIMER_SECS);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn an_unbounded_run_hands_the_captures_no_deadline_that_can_elapse() {
        let fx = fixture("nowindow");
        let mut args = diag_args(&fx);
        args.client = false;
        args.timeout_secs = None;
        let host = TestHost::default();

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        assert!(
            !outcome.server.timed_out,
            "the stub exits by itself; no timer may cut it short: {:?}",
            outcome.server
        );
        let window = host.window_secs.load(Ordering::SeqCst);
        assert_eq!(window, NO_TIMER_SECS);
        assert!(
            window > crate::run_ops::DEFAULT_RUN_TIMEOUT_SECS,
            "unbounded must outlast the old bounded window: {window}"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn a_bounded_run_still_times_out() {
        let fx = fixture("bounded");
        let mut args = diag_args(&fx);
        args.client = false;
        args.timeout_secs = Some(1);
        let host = TestHost::default();

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        assert!(outcome.server.timed_out, "{:?}", outcome.server);
        assert!(!outcome.server.stopped, "{:?}", outcome.server);
        assert_eq!(host.window_secs.load(Ordering::SeqCst), 1);
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_stop_at_the_ready_deadline_is_not_a_ready_timeout() {
        let fx = fixture("stop-at-deadline");
        let args = diag_args(&fx);
        let host = TestHost::default();
        host.ready_timeout_ms.store(0, Ordering::SeqCst);
        let stop = StopSignal::new();
        stop.stop();

        let outcome = run_diag_with(&args, &stop, &host).unwrap();

        assert!(!outcome.ready_timed_out);
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn the_ready_wait_gives_up_on_a_server_that_never_reports_ready() {
        let fx = fixture("neverready");
        let alive = format!(
            "#!/bin/sh\nprintf 'START\\nrole:server\\n' >> '{rec}'\nsleep 1\n",
            rec = fx.record.display(),
        );
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &alive,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = None;
        let host = TestHost::default();
        host.ready_timeout_ms.store(200, Ordering::SeqCst);

        let outcome = run_diag_with(&args, &StopSignal::new(), &host).unwrap();

        assert!(
            outcome.ready_timed_out,
            "a server that never reports ready must end the run as a ready timeout"
        );
        assert!(outcome.server.stopped, "the server must be stopped");
        assert!(
            host.stopped_server.load(Ordering::SeqCst) >= 1,
            "the server processes must be stopped"
        );
        assert!(outcome.client.is_none(), "no client without a ready server");
        assert_eq!(
            host.ready_result(),
            Some(MarkerWait::TimedOut),
            "the ready wait must give up, not wait for the server to exit"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn fullscreen_drops_every_window_argument_from_the_client() {
        let fx = fixture("fullscreen");
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(2);
        args.windowed = false;

        run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let rec = record(&fx);
        let b = blocks(&rec);
        let argv = block_argv(b[1]);
        for flag in ["-window", "-x=1280", "-y=720"] {
            assert!(
                !argv.iter().any(|a| a == flag),
                "{flag} must be gone full screen: {argv:?}"
            );
        }
        assert!(argv.iter().any(|a| a == "-nolauncher"), "{argv:?}");
        assert!(argv.iter().any(|a| a == "-mod=@X"), "{argv:?}");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn the_expected_line_stops_the_run_and_the_verdict_passes() {
        let fx = fixture("verdict-stop");
        let done = format!(
            "#!/bin/sh\n\
             role=client\n\
             for a in \"$@\"; do [ \"$a\" = \"-server\" ] && role=server; done\n\
             if [ \"$role\" = server ]; then\n\
             printf 'Player connect enabled\\r\\n[SampleModTests] DONE side=server\\r\\n' > '{p}/DayZDiag_x64_1.RPT'\n\
             else\n\
             printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{l}/DayZDiag_x64_1.RPT'\n\
             fi\n\
             n=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done\n",
            p = fx.profiles.display(),
            l = fx.log_dir.display(),
        );
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &done,
        );
        let mut args = diag_args(&fx);
        args.timeout_secs = Some(5);
        args.expect_lines = vec!["[SampleModTests] DONE".into()];
        let started = Instant::now();

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        assert!(
            outcome.server.stopped && !outcome.server.timed_out,
            "the watcher must stop the run: {:?}",
            outcome.server
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the run waited for its window: {:?}",
            started.elapsed()
        );
        let verdict = outcome.verdict.expect("the run asked for a verdict");
        assert!(verdict.passed, "{verdict:?}");
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_verdict_catches_a_script_error_in_the_runs_own_staged_source() {
        for (label, mod_prefix, source) in [
            ("verdict-prefix", None, "source"),
            ("verdict-mod-prefix", Some("FS\\Core"), "FS/Core"),
        ] {
            let fx = fixture(label);
            let body = format!(
                "#!/bin/sh\n\
                 printf 'Player connect enabled\\r\\n SCRIPT    (E): @\"{source}/Scripts/3_Game/Broken.c,5\": boom\\r\\n' > '{p}/DayZDiag_x64_1.RPT'\n\
                 sleep 0.1\n",
                p = fx.profiles.display(),
            );
            write_exec(
                &fx.root
                    .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
                &body,
            );
            let mut args = diag_args(&fx);
            args.client = false;
            args.timeout_secs = Some(5);
            args.expect_lines = vec!["Player connect enabled".into()];
            args.mod_prefix = mod_prefix.map(str::to_owned);

            let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

            let verdict = outcome.verdict.expect("the run asked for a verdict");
            assert_eq!(verdict.mod_errors.len(), 1, "{label}: {verdict:?}");
            assert!(!verdict.passed, "{label}: {verdict:?}");
            let _ = std::fs::remove_dir_all(&fx.root);
        }
    }

    #[test]
    fn a_diag_run_marks_the_errors_its_projects_known_file_explains() {
        let fx = fixture("known-diag");
        let file = fx.source.join(crate::known_errors::KNOWN_ERRORS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            serde_json::json!([{
                "id": "broken00",
                "pattern": "boom",
                "meaning": "the staged script fails on load",
                "hint": "fix it",
            }])
            .to_string(),
        )
        .unwrap();
        let mut args = diag_args(&fx);
        args.client = false;
        args.timeout_secs = Some(5);
        let body = format!(
            "#!/bin/sh\n\
             printf 'Player connect enabled\\r\\n SCRIPT    (E): @\"source/Scripts/3_Game/Broken.c,5\": boom\\r\\n' > '{p}/DayZDiag_x64_1.RPT'\n\
             sleep 0.1\n",
            p = fx.profiles.display(),
        );
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &body,
        );

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        let error = outcome
            .errors
            .iter()
            .find(|e| e.message.contains("boom"))
            .expect("the stub's script error is mapped");
        assert_eq!(
            error.known.as_ref().map(|k| k.id.as_str()),
            Some("broken00")
        );
        assert!(
            outcome.verdict.is_none(),
            "a known-errors file alone asks for no verdict"
        );
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn the_default_prefix_comes_from_the_built_pbo_then_the_source_folder() {
        type Case<'a> = (&'a str, Vec<(&'a str, Vec<u8>)>, &'a str, &'a str);
        let cases: [Case; 5] = [
            (
                "one-built-pbo",
                vec![("cf.pbo", packed_pbo("JM\\CF\\"))],
                "FieldKit",
                "JM\\CF",
            ),
            ("no-addons-folder", vec![], "FieldKit/.", "FieldKit"),
            ("parent-of-source", vec![], "FieldKit/sub/..", "FieldKit"),
            (
                "two-built-pbos",
                vec![
                    ("cf.pbo", packed_pbo("JM\\CF")),
                    ("other.pbo", packed_pbo("JM\\Other")),
                ],
                "FieldKit",
                "FieldKit",
            ),
            (
                "unreadable-pbo",
                vec![("cf.pbo", b"truncated".to_vec())],
                "FieldKit",
                "FieldKit",
            ),
        ];
        for (label, pbos, source, want) in cases {
            let root = tmp(label);
            let mod_dir = root.join("@FieldKit");
            std::fs::create_dir_all(root.join("FieldKit/sub")).unwrap();
            for (name, bytes) in pbos {
                let addons = mod_dir.join("Addons");
                std::fs::create_dir_all(&addons).unwrap();
                std::fs::write(addons.join(name), bytes).unwrap();
            }

            let got = default_mod_prefix(&mod_dir, &root.join(source)).unwrap();

            assert_eq!(got, want, "{label}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A root `config.cpp` whose `CfgMods` `defs` module binds `files[]`.
    fn config_cpp(files: &[&str]) -> String {
        let entries: Vec<String> = files.iter().map(|f| format!("\"{f}\"")).collect();
        format!(
            "class CfgPatches {{ class FieldKit_Scripts {{ requiredAddons[] = {{\"DZ_Data\"}}; }}; }};\n\
             class CfgMods {{ class FieldKit {{ type = \"mod\"; class defs {{\n\
                 class gameScriptModule {{ files[] = {{ {} }}; }};\n\
             }}; }}; }};\n",
            entries.join(", ")
        )
    }

    fn packed_pbo(prefix: &str) -> Vec<u8> {
        let props = [("prefix".to_owned(), prefix.to_owned())];
        modfoundry_pbo::pack_pbo(&props, &[], &modfoundry_pbo::PackOptions::default()).unwrap()
    }

    /// A built `@FieldKit` holding one PBO, and a source root carrying `config`.
    fn prefix_fixture(
        label: &str,
        pbo_prefix: &str,
        config: Option<&str>,
    ) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmp(label);
        let mod_dir = root.join("build/@FieldKit");
        let addons = mod_dir.join("addons");
        std::fs::create_dir_all(&addons).unwrap();
        std::fs::write(addons.join("FieldKit.pbo"), packed_pbo(pbo_prefix)).unwrap();
        let source = root.join("FieldKit");
        std::fs::create_dir_all(source.join("Scripts/3_Game")).unwrap();
        if let Some(text) = config {
            std::fs::write(source.join("config.cpp"), text).unwrap();
        }
        (root, mod_dir, source)
    }

    #[test]
    fn a_pbo_prefix_is_checked_against_the_sources_files_entries() {
        struct Case<'a> {
            label: &'a str,
            pbo_prefix: &'a str,
            config: Option<&'a str>,
            want: Result<&'a str, &'a [&'a str]>,
        }
        let entries = config_cpp(&["FieldKit/Scripts/3_Game"]);
        let cases = [
            Case {
                label: "agreeing",
                pbo_prefix: "FieldKit",
                config: Some(&entries),
                want: Ok("FieldKit"),
            },
            Case {
                label: "stale",
                pbo_prefix: "Old",
                config: Some(&entries),
                want: Err(&["Old", "FieldKit/Scripts/3_Game", "rebuild"]),
            },
            Case {
                label: "case and slash style",
                pbo_prefix: "JM\\CF\\",
                config: Some(&config_cpp(&["jm/cf/Scripts/3_Game"])),
                want: Ok("JM\\CF"),
            },
            Case {
                label: "no config",
                pbo_prefix: "Old",
                config: None,
                want: Ok("Old"),
            },
            Case {
                label: "unparseable config",
                pbo_prefix: "Old",
                config: Some("class CfgMods { class FieldKit {"),
                want: Ok("Old"),
            },
            Case {
                label: "parse issue after a mismatching entry",
                pbo_prefix: "Old",
                config: Some(&format!("{entries}class Broken {{")),
                want: Ok("Old"),
            },
            Case {
                label: "entries with leading separators",
                pbo_prefix: "FieldKit",
                config: Some(&config_cpp(&[
                    "./FieldKit/Scripts/3_Game",
                    "/FieldKit/Scripts/4_World",
                ])),
                want: Ok("FieldKit"),
            },
            Case {
                label: "prefix with a leading separator",
                pbo_prefix: "\\FieldKit",
                config: Some(&entries),
                want: Ok("FieldKit"),
            },
        ];
        for Case {
            label,
            pbo_prefix,
            config,
            want,
        } in cases
        {
            let (root, mod_dir, source) = prefix_fixture(label, pbo_prefix, config);

            let got = default_mod_prefix(&mod_dir, &source);

            match want {
                Ok(prefix) => assert_eq!(got.unwrap(), prefix, "{label}"),
                Err(texts) => {
                    let err = got.expect_err(label).to_string();
                    for text in texts {
                        assert!(err.contains(text), "{label}: {text} missing from {err}");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn an_explicit_mod_prefix_skips_the_files_check() {
        let fx = fixture("explicit-prefix");
        std::fs::write(
            fx.source.join("config.cpp"),
            config_cpp(&["FieldKit/Scripts/3_Game"]),
        )
        .unwrap();
        let addons = fx.built_mod.join("addons");
        std::fs::create_dir_all(&addons).unwrap();
        std::fs::write(addons.join("FieldKit.pbo"), packed_pbo("Old")).unwrap();
        let mut args = diag_args(&fx);
        args.mod_prefix = Some("Old".into());

        let plan = plan_diag(&args).unwrap();

        assert!(
            plan.steps.iter().any(|step| matches!(
                step,
                crate::launch_plan::StageStep::Link { link, .. } if link == &fx.game.join("Old")
            )),
            "{:?}",
            plan.steps
        );
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn the_source_folder_fallback_drops_one_leading_at() {
        for (folder, want) in [("@FieldKit", "FieldKit"), ("FieldKit", "FieldKit")] {
            let root = tmp("at-source");
            let source = root.join(folder);
            std::fs::create_dir_all(&source).unwrap();

            let got = default_mod_prefix(&root.join("@FieldKit"), &source).unwrap();

            assert_eq!(got, want, "{folder}");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn a_stale_pbo_prefix_fails_the_run_before_anything_is_staged_or_started() {
        let fx = fixture("stale-prefix");
        std::fs::write(
            fx.source.join("config.cpp"),
            config_cpp(&["FieldKit/Scripts/3_Game"]),
        )
        .unwrap();
        let addons = fx.built_mod.join("addons");
        std::fs::create_dir_all(&addons).unwrap();
        std::fs::write(addons.join("FieldKit.pbo"), packed_pbo("Old")).unwrap();
        let host = TestHost::default();

        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &host)
            .unwrap_err()
            .to_string();

        for text in ["Old", "FieldKit/Scripts/3_Game", "rebuild"] {
            assert!(err.contains(text), "{text} missing: {err}");
        }
        assert!(!fx.record.exists(), "no process may start: {err}");
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    /// A file a dead run staged in `dir`, journaled so a clear would remove it.
    fn dead_leftover(dir: &Path, name: &str) -> PathBuf {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let path = dir.join(name);
        let contents = b"left by a dead run";
        std::fs::write(&path, contents).unwrap();
        let mut hasher = DefaultHasher::new();
        contents.hash(&mut hasher);
        let line = serde_json::json!({
            "kind": "file",
            "path": path,
            "len": contents.len(),
            "hash": hasher.finish(),
        });
        let journal = dir.join(modfoundry_platform::stage::JOURNAL_NAME);
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::write(
            journal.join(format!("{}-0.jsonl", dead_pid())),
            format!("{line}\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn a_bad_source_root_fails_the_run_and_leaves_a_dead_runs_staging_alone() {
        let fx = fixture("bad-source-before-clear");
        let leftover = dead_leftover(&fx.game, "staged-by-a-dead-run");
        let mut args = diag_args(&fx);
        args.source_root = fx.root.join("absent");

        let err = run_diag_with(&args, &StopSignal::new(), &TestHost::default())
            .unwrap_err()
            .to_string();

        assert!(err.contains("absent"), "unhelpful: {err}");
        assert!(
            leftover.is_file(),
            "the leftover must survive a plan that never ran: {err}"
        );
        assert!(!fx.record.exists(), "no process may start: {err}");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_valid_run_clears_a_dead_runs_staging_and_reports_it() {
        let fx = fixture("clear-then-run");
        let leftover = dead_leftover(&fx.game, "staged-by-a-dead-run");
        let quick = format!(
            "#!/bin/sh\n\
             printf 'START\\nrole:server\\nLS_BEGIN\\n' >> '{rec}'\n\
             ls -1 >> '{rec}'\n\
             printf 'LS_END\\n' >> '{rec}'\n\
             printf 'Player connect enabled\\r\\n' > '{p}/DayZDiag_x64_1.RPT'\n",
            rec = fx.record.display(),
            p = fx.profiles.display(),
        );
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &quick,
        );
        let mut args = diag_args(&fx);
        args.client = false;

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        assert!(
            !leftover.exists(),
            "the dead run's file must be cleared: {leftover:?}"
        );
        assert_eq!(outcome.cleared, vec![leftover]);
        let staged = block_ls(blocks(&record(&fx))[0]);
        for name in ["source", "@X"] {
            assert!(
                staged.iter().any(|e| e == name),
                "{name} missing from the staging after the clear: {staged:?}"
            );
        }
        assert_eq!(entries(&fx.game), vec![DIAG_EXE.to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_staging_conflict_leaves_a_dead_runs_staging_in_place() {
        let fx = fixture("conflict-before-clear");
        let leftover = dead_leftover(&fx.game, "staged-by-a-dead-run");
        std::fs::create_dir_all(fx.game.join("source")).unwrap();

        let err = run_diag_with(&diag_args(&fx), &StopSignal::new(), &TestHost::default())
            .unwrap_err()
            .to_string();

        assert!(err.contains("source"), "unhelpful: {err}");
        assert!(
            leftover.is_file(),
            "the leftover must survive a refused staging: {err}"
        );
        assert!(!fx.record.exists(), "no process may start: {err}");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_dead_runs_folder_where_the_source_link_goes_is_cleared_before_staging() {
        let fx = fixture("dead-folder-at-link");
        let folder = fx.game.join("source");
        std::fs::create_dir_all(&folder).unwrap();
        let journal = fx.game.join(modfoundry_platform::stage::JOURNAL_NAME);
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::write(
            journal.join(format!("{}-0.jsonl", dead_pid())),
            format!("{}\n", serde_json::json!({ "kind": "dir", "path": folder })),
        )
        .unwrap();
        write_exec(
            &fx.root
                .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
            &format!(
                "#!/bin/sh\nprintf 'Player connect enabled\\r\\n' > '{}/DayZDiag_x64_1.RPT'\n",
                fx.profiles.display()
            ),
        );
        let mut args = diag_args(&fx);
        args.client = false;

        let outcome = run_diag_with(&args, &StopSignal::new(), &TestHost::default()).unwrap();

        assert_eq!(outcome.cleared, vec![folder]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }
}
