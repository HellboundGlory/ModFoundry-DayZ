//! ModFoundry CLI (`mf`).
//!
//! Thin frontend: parses argv, dispatches to the core command registry
//! ([`modfoundry_commands::core`]) through the agent broker (so the
//! persisted policy authorizes the call and the audit trail records it),
//! and renders the shared outcome. No product logic lives here.

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use modfoundry_agent::{
    effect_from_str, effect_str, now_unix, open_db, resolve_db, session_only_warning, AuditOutcome,
    Broker, Decision, OpenedDb,
};
use modfoundry_commands::diag_stage::{TestModArg, TestModRole};
use modfoundry_commands::font_ops::FontArgs;
use modfoundry_commands::image_ops::ImageArgs;
use modfoundry_commands::imageset_ops::ImagesetArgs;
use modfoundry_commands::pbo_ops::PboUnpackArgs;
use modfoundry_commands::project_deps::{DependencyOrigin, SavedDependency};
use modfoundry_commands::project_settings::ModSource;
use modfoundry_commands::publish_upload::UploadState;
use modfoundry_commands::registry::Session as RegistrySession;
use modfoundry_commands::registry::{
    ConvertArgs, FindingReport, GpuReport, HostReport, KeygenArgs, PathArgs, ScaffoldArgs,
    SignCheckArgs,
};
use modfoundry_commands::run_ops::{cleared_line, WindowSize};
use modfoundry_commands::script_lint::{GameReport, ScriptLintReport};
use modfoundry_commands::update::{self, NOTICE_WAIT};
use modfoundry_commands::{
    ready_timeout_message, BuildReport, ClientRunOutcome, DiagOutcome, RunClientArgs,
    RunClientCommandArgs, RunDiagArgs, RunOutcome, RunServerArgs,
};
use modfoundry_core::{Actor, Core, Diagnostic, EffectKind, Outcome, Severity, PREVIEW_ARG};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `std`'s `println!`, `print!` and `eprintln!`, except that `mf` ends quietly once its reader has gone (`mf ... | head`).
macro_rules! checked_write {
    ($out:expr, $write:ident, $($arg:tt)*) => {{
        use std::io::Write as _;
        if let Err(error) = $write!($out, $($arg)*) {
            $crate::closed_output(error);
        }
    }};
}

macro_rules! println {
    ($($arg:tt)*) => { checked_write!(std::io::stdout(), writeln, $($arg)*) };
}

macro_rules! print {
    ($($arg:tt)*) => { checked_write!(std::io::stdout(), write, $($arg)*) };
}

macro_rules! eprintln {
    ($($arg:tt)*) => { checked_write!(std::io::stderr(), writeln, $($arg)*) };
}

/// Exits with the status a shell gives a program its pipe closed on; any other write error panics as `std` does.
fn closed_output(error: std::io::Error) {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(141);
    }
    panic!("failed printing: {error}");
}

#[derive(Parser)]
#[command(
    name = "mf",
    version,
    about = "ModFoundry — modern cross-platform DayZ modding environment"
)]
struct Cli {
    /// Agent database holding the policy rules and the audit trail (default:
    /// `$MODFOUNDRY_DB`, else the host data dir).
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Return the change-set a mutating verb would apply, without applying it.
    #[arg(long, global = true)]
    preview: bool,
    #[command(subcommand)]
    command: Commands,
}

/// The launch options `mf run` takes after the mod dir.
#[derive(clap::Args)]
struct RunFlags {
    /// DayZServer install dir override (default: platform discovery).
    #[arg(long)]
    server: Option<PathBuf>,
    /// `-config=…` server config file (relative to the server dir).
    #[arg(long)]
    config: Option<PathBuf>,
    /// A named launch set in the project's `server/launch-sets.json`; its mods and config fill
    /// what the flags leave out.
    #[arg(long)]
    set: Option<String>,
    /// `-port=…` listen port.
    #[arg(long)]
    port: Option<u16>,
    /// Bounded launch window in seconds before we stop the server (default: 90).
    #[arg(long)]
    timeout: Option<u64>,
    /// Loose mod source root to map logged errors to host files (the built
    /// @Mod is packed; pass the project dir for file-mapped errors).
    #[arg(long)]
    source_root: Option<PathBuf>,
    /// Extra mod loaded only for this run (repeatable; order preserved).
    /// `PATH` is the mod dir; append `:server` to load it on the server only.
    #[arg(long = "test-mod", value_parser = parse_test_mod)]
    test_mods: Vec<TestModArg>,
    /// A line the run's logs must show for its verdict to pass (repeatable;
    /// `re:` makes it a regular expression); the run stops 5 seconds after every
    /// expected line has appeared in an RPT log, with `--timeout` as the upper limit.
    #[arg(long = "expect-line", value_name = "TEXT")]
    expect_lines: Vec<String>,
    /// A line that fails the run's verdict when it appears (repeatable; `re:`
    /// makes it a regular expression).
    #[arg(long = "fail-line", value_name = "TEXT")]
    fail_lines: Vec<String>,
    /// A known script error the run's verdict ignores (repeatable; `re:` makes
    /// it a regular expression).
    #[arg(long = "allow-error", value_name = "TEXT")]
    allow_errors: Vec<String>,
}

/// The launch options `mf run-client` takes.
#[derive(clap::Args)]
struct RunClientFlags {
    /// Built `@Mod` dir(s) to stage into the client install and load (repeatable; order preserved).
    /// The last one is the run's own mod: the verdict ties script errors to it and to the test
    /// mods, not to the others.
    #[arg(long = "mod", required_unless_present = "set")]
    mods: Vec<PathBuf>,
    /// A named launch set in the project's `server/launch-sets.json`; its shared mods are the
    /// ones this run loads.
    #[arg(long)]
    set: Option<String>,
    /// DayZ client install dir override (default: platform discovery).
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// Steam library root override (default: platform discovery).
    #[arg(long)]
    steam_dir: Option<PathBuf>,
    /// Launcher executable override (default: launch DayZ directly under
    /// Proton).
    #[arg(long)]
    launcher: Option<PathBuf>,
    /// `-connect=host:gameport[:queryport]` auto-connect target.
    #[arg(long)]
    connect: Option<String>,
    /// `-name=<profile>` game profile name.
    #[arg(long)]
    name: Option<String>,
    /// Extra game args appended verbatim (e.g. `-nolauncher`, `-world=empty`).
    #[arg(long = "extra")]
    extra: Vec<String>,
    /// Windowed client size as `WxH` (default 1280x720).
    #[arg(long, value_parser = parse_window, conflicts_with = "fullscreen")]
    window: Option<WindowSize>,
    /// Open the client in a window (the default).
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,
    /// Run the client full screen instead of in the default window.
    #[arg(long, conflicts_with = "windowed")]
    fullscreen: bool,
    /// Bounded capture window in seconds (default: 90).
    #[arg(long)]
    timeout: Option<u64>,
    /// Loose mod source root to map logged errors to host files.
    #[arg(long)]
    source_root: Option<PathBuf>,
    /// Extra mod loaded only for this run (repeatable; order preserved). The client loads these
    /// before the --mod mods, and before the mods a --set fills.
    #[arg(long = "test-mod", value_parser = parse_test_mod)]
    test_mods: Vec<TestModArg>,
    /// A line the run's logs must show for its verdict to pass (repeatable;
    /// `re:` makes it a regular expression); the run stops 5 seconds after every
    /// expected line has appeared in an RPT log, with `--timeout` as the upper limit.
    #[arg(long = "expect-line", value_name = "TEXT")]
    expect_lines: Vec<String>,
    /// A line that fails the run's verdict when it appears (repeatable; `re:`
    /// makes it a regular expression).
    #[arg(long = "fail-line", value_name = "TEXT")]
    fail_lines: Vec<String>,
    /// A known script error the run's verdict ignores (repeatable; `re:` makes
    /// it a regular expression).
    #[arg(long = "allow-error", value_name = "TEXT")]
    allow_errors: Vec<String>,
}

/// The launch options `mf run-diag` takes after the mod dir.
#[derive(clap::Args)]
struct RunDiagFlags {
    /// The loose mod source root staged as the PBO prefix (needed for
    /// `-filePatching` to load loose files; with --set, the current folder by default).
    #[arg(long, required_unless_present = "set")]
    source_root: Option<PathBuf>,
    /// Server config template under the project's `server/` folder (default `serverDZ.cfg`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// A named launch set in the project's `server/launch-sets.json`; its mods and config fill
    /// what the flags leave out.
    #[arg(long)]
    set: Option<String>,
    /// PBO prefix the loose source is staged under (default: the built PBO's prefix, else the source folder name without a leading @).
    #[arg(long)]
    mod_prefix: Option<String>,
    /// DayZ client install dir override (default: platform discovery).
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// DayZServer install dir the `mpmissions`/config come from.
    #[arg(long)]
    server: Option<PathBuf>,
    /// Steam library root override (default: platform discovery).
    #[arg(long)]
    steam_dir: Option<PathBuf>,
    /// Real profiles dir the staged relative link points at.
    #[arg(long)]
    profiles_dir: Option<PathBuf>,
    /// `-port=…` Diag server port (default 2302).
    #[arg(long)]
    port: Option<u16>,
    /// Start the Diag client that joins the server (the default).
    #[arg(long, conflicts_with = "no_client")]
    client: bool,
    /// Do not start the Diag client; run the server alone.
    #[arg(long = "no-client", conflicts_with = "client")]
    no_client: bool,
    /// `-name=<profile>` game profile name for the client.
    #[arg(long)]
    connect_name: Option<String>,
    /// Windowed client size as `WxH` (default 1280x720).
    #[arg(long, value_parser = parse_window, conflicts_with = "fullscreen")]
    window: Option<WindowSize>,
    /// Open the client in a window (the default).
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,
    /// Run the client full screen instead of in the default window.
    #[arg(long, conflicts_with = "windowed")]
    fullscreen: bool,
    /// Extra mod loaded only for this run (repeatable; order preserved).
    /// `PATH` is the mod dir; append `:server` to load it on the server only.
    #[arg(long = "test-mod", value_parser = parse_test_mod)]
    test_mods: Vec<TestModArg>,
    /// Launch window in seconds; omit to keep running until stopped.
    #[arg(long)]
    timeout: Option<u64>,
    /// A line the run's logs must show for its verdict to pass (repeatable;
    /// `re:` makes it a regular expression); the run stops 5 seconds after every
    /// expected line has appeared in an RPT log, with `--timeout` as the upper limit.
    #[arg(long = "expect-line", value_name = "TEXT")]
    expect_lines: Vec<String>,
    /// A line that fails the run's verdict when it appears (repeatable; `re:`
    /// makes it a regular expression).
    #[arg(long = "fail-line", value_name = "TEXT")]
    fail_lines: Vec<String>,
    /// A known script error the run's verdict ignores (repeatable; `re:` makes
    /// it a regular expression).
    #[arg(long = "allow-error", value_name = "TEXT")]
    allow_errors: Vec<String>,
}

/// The build options `mf build` and `mf publish` share.
#[derive(clap::Args)]
struct BuildFlags {
    /// Override the mod name (default: target dir name).
    #[arg(long)]
    name: Option<String>,
    /// Output dir override.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Build profile (dir under builds/; default "default").
    #[arg(long)]
    profile: Option<String>,
    /// Signing authority; enables signing using <authority>.biprivatekey/.bikey.
    #[arg(long)]
    authority: Option<String>,
    /// Dir holding the keypair (default: target/keys).
    #[arg(long)]
    keys_dir: Option<PathBuf>,
    /// PBO header prefix (default: --name). Must match the leading path of
    /// the config's CfgMods `files[]`; set it when importing a tree whose
    /// config declares its own prefix (e.g. `JM/CF`).
    #[arg(long)]
    prefix: Option<String>,
    /// DsUtils dir: sign with the official DSSignFile instead of the
    /// built-in native signer.
    #[arg(long)]
    tools: Option<PathBuf>,
    /// Binarize `config.cpp` into `config.bin` (CfgConvert `-bin` parity)
    /// inside the packed PBO instead of shipping the raw text.
    #[arg(long, conflicts_with = "no_binarize")]
    binarize: bool,
    /// Ship the raw `config.cpp` even when the project's saved setting binarizes.
    #[arg(long)]
    no_binarize: bool,
    /// Strip comments from `.c` files in the packed PBO; line numbers still match.
    #[arg(long, conflicts_with = "no_strip_comments")]
    strip_comments: bool,
    /// Keep comments even when the project's saved setting strips them.
    #[arg(long)]
    no_strip_comments: bool,
}

/// Leaves `null` members out of a dispatch's arguments so saved settings can fill them.
fn drop_nulls(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    args
}

/// The flags `mf build` and `mf publish` send, without the target they name.
fn build_flag_args(flags: BuildFlags) -> Value {
    let mut args = drop_nulls(json!({
        "name": flags.name,
        "output": flags.out,
        "profile": flags.profile,
        "authority": flags.authority,
        "keys_dir": flags.keys_dir,
        "prefix": flags.prefix,
        "tools": flags.tools,
        "binarize_config": flags.binarize,
        "strip_comments": flags.strip_comments,
    }));
    only_if_given(
        &mut args,
        "binarize_config",
        flags.binarize || flags.no_binarize,
    );
    only_if_given(
        &mut args,
        "strip_comments",
        flags.strip_comments || flags.no_strip_comments,
    );
    args
}

fn build_args(target: PathBuf, flags: BuildFlags) -> Value {
    let mut args = build_flag_args(flags);
    args["mod_root"] = json!(target);
    args
}

/// The arguments `mf build` sends: the target it names, a mod root or a launch set.
fn build_request(
    target: Option<PathBuf>,
    set: Option<String>,
    source_root: Option<PathBuf>,
    flags: BuildFlags,
) -> Value {
    match set {
        Some(set) => {
            let mut args = build_flag_args(flags);
            args["set"] = json!(set);
            if let Some(source_root) = source_root {
                args["source_root"] = json!(source_root);
            }
            args
        }
        None => build_args(target.expect("clap requires a target without --set"), flags),
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Detect DayZ game / DayZ Tools / Steam workshop installations on this host.
    ///
    /// A release install also checks GitHub for a newer ModFoundry once a day; set
    /// `MF_NO_UPDATE_CHECK=1` to turn that off.
    Doctor,
    /// List file formats ModFoundry recognizes.
    Formats,
    /// Print what changed in each release (added, changed, fixed, removed).
    Changelog {
        /// Only the releases newer than this one.
        #[arg(long, value_name = "X.Y.Z")]
        since: Option<String>,
    },
    /// Check for or install the newest ModFoundry release.
    ///
    /// `MF_NO_UPDATE_CHECK=1` turns off this command and the daily background check.
    Update {
        /// Only ask whether a newer release is out.
        #[arg(long)]
        check: bool,
    },
    /// Scaffold a new DayZ script mod skeleton into a directory.
    New {
        /// Mod name: a config class name (a letter or _ first, then letters, digits and _); also the class prefix.
        name: String,
        /// Target directory to create the mod in (default: `./<name>`).
        #[arg(long, short)]
        dir: Option<PathBuf>,
        /// Launcher name (default: the mod name).
        #[arg(long)]
        display_name: Option<String>,
        /// Launcher description: the first line is the tooltip, all of it the overview.
        #[arg(long)]
        description: Option<String>,
        /// Author name for mod.cpp.
        #[arg(long)]
        author: Option<String>,
        /// mod.cpp version (default: 0.1.0).
        #[arg(long = "mod-version")]
        mod_version: Option<String>,
        /// PBO prefix for the files[] paths (default: the mod name).
        #[arg(long)]
        prefix: Option<String>,
        /// The CfgPatches class name (default: <name>_Scripts).
        #[arg(long)]
        patch_class: Option<String>,
        /// A CfgPatches name to list in requiredAddons after DZ_Data; repeatable.
        #[arg(long = "required-addon", value_name = "NAME")]
        required_addon: Vec<String>,
        /// Script module dir to bind; repeatable (default: 3_Game, 4_World, 5_Mission).
        #[arg(long = "module", value_name = "DIR")]
        module: Vec<String>,
        /// Do not write the 5_Mission starter script.
        #[arg(long)]
        no_starter: bool,
    },
    /// Validate a DayZ config.cpp (a file, or a directory containing one). A config.cpp the
    /// project's .pboignore leaves out gets no findings.
    Validate { target: PathBuf },
    /// Lint Enforce Script sources (a .c file, or a directory tree), then report the compile
    /// errors the game itself would print for the whole mod. Unknown-type warnings (MF-W3002)
    /// and the game check need the mod's dependencies: saved with `mf deps add`, or --dependency
    /// / --no-dependencies.
    Lint {
        /// A `.c` file or a folder of Enforce sources.
        target: PathBuf,
        /// DayZ install dir to index vanilla types from (default: host discovery).
        #[arg(long)]
        game: Option<PathBuf>,
        /// A dependency mod folder, packed or source; repeatable. Left out, the dependencies
        /// saved with `mf deps`.
        #[arg(long = "dependency", value_name = "PATH")]
        dependencies: Vec<PathBuf>,
        /// The mod needs no other mod.
        #[arg(long, conflicts_with = "dependencies")]
        no_dependencies: bool,
        /// Check only this game build in the game check; repeatable (default: all three).
        #[arg(long = "target", value_name = "BUILD", value_parser = ["retail-server", "diag-server", "diag-client"])]
        builds: Vec<String>,
    },
    /// Save the dependency mods of a mod project (used by `mf lint` and the game check).
    Deps {
        #[command(subcommand)]
        command: DepsCommands,
    },
    /// Inspect Enforce Script sources.
    Script {
        #[command(subcommand)]
        command: ScriptCommands,
    },
    /// Search the indexed game scripts, configs, layouts and docs, or build that index.
    #[command(args_conflicts_with_subcommands = true)]
    Search {
        #[command(subcommand)]
        command: Option<SearchCommands>,
        /// The question, in plain words.
        #[arg(value_name = "QUESTION")]
        question: Option<String>,
        /// Only pieces of this kind; repeatable.
        #[arg(long, value_name = "KIND")]
        kind: Vec<String>,
        /// Only pieces from these source labels; repeatable.
        #[arg(long, value_name = "LABEL")]
        source: Vec<String>,
        /// How many hits to print (default 8).
        #[arg(long)]
        top: Option<usize>,
        /// DayZ install dir (default: host discovery).
        #[arg(long)]
        game: Option<PathBuf>,
        /// Local model folder (default: the app cache).
        #[arg(long)]
        model: Option<PathBuf>,
        /// Print the whole report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check .layout files against the game's widget classes, properties and files.
    Layout {
        #[command(subcommand)]
        command: LayoutCommands,
    },
    /// Compare two DayZ builds and list where mods use what changed.
    GameDiff {
        /// A game install, an unpacked game folder, a .pbo, a snapshot .json or a saved build number.
        old: String,
        /// A game install, an unpacked game folder, a .pbo, a snapshot .json or a saved build number.
        new: String,
        /// A mod folder to check; repeatable.
        #[arg(long = "against", value_name = "DIR")]
        against: Vec<PathBuf>,
        /// Folder of saved builds (default: mf's data folder).
        #[arg(long)]
        snapshots: Option<PathBuf>,
        /// Print the whole report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List what several mods declare or modify in common: scripts, configs and packed files.
    ConflictCheck {
        /// A mod folder, in load order; two or more.
        #[arg(value_name = "MOD")]
        mods: Vec<PathBuf>,
        /// A launch set of the project at `--root`, in place of the mod folders.
        #[arg(long, value_name = "NAME")]
        set: Option<String>,
        /// The project whose server/launch-sets.json holds the set (default: the current folder).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Print the whole report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Save a snapshot of a DayZ install for game-diff.
    GameSnapshot {
        /// DayZ install dir (default: host discovery).
        game: Option<PathBuf>,
        /// Folder of saved builds (default: mf's data folder).
        #[arg(long)]
        snapshots: Option<PathBuf>,
        /// Print the whole result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deterministically pack (+ optionally sign) a mod into builds/<profile>/@Mod, or every
    /// local mod of a launch set with --set. A flag left out takes the project's saved
    /// setting where one applies.
    Build {
        /// Mod root to pack (leave out with --set).
        #[arg(required_unless_present = "set", conflicts_with = "set")]
        target: Option<PathBuf>,
        #[command(flatten)]
        flags: BuildFlags,
        /// A named launch set in the project's `server/launch-sets.json`: its local mods are
        /// built in order, stopping at the first failure.
        #[arg(long)]
        set: Option<String>,
        /// Project root holding `server/launch-sets.json` (default: the current folder).
        #[arg(long, conflicts_with = "target")]
        source_root: Option<PathBuf>,
        /// List every file and whether it packs, without building (with --binarize the root
        /// config.cpp shows as packed as config.bin, and a root config.bin beside it is a clash
        /// that refuses the build).
        #[arg(long, conflicts_with_all = ["set", "source_root"])]
        list: bool,
    },
    /// Build, check and upload a mod to the Steam Workshop (with --preview: check only and show the plan).
    /// A flag left out takes the project's saved setting where one applies. A .pbo or .bisign an earlier
    /// build left in the output's addons/ folder is removed first (MF-P020).
    Publish {
        /// Mod root to upload.
        target: PathBuf,
        #[command(flatten)]
        flags: BuildFlags,
        /// Send only the content, change note and key-value tags to an existing item.
        #[arg(long)]
        content_only: bool,
        /// What changed, shown on the item's change notes.
        #[arg(long)]
        change_note: Option<String>,
    },
    /// List your published DayZ Workshop items.
    PublishItems,
    /// Check a built mod's signatures, and optionally the keys of a DayZServer install.
    CheckSignatures {
        /// The built `@Mod` folder to check.
        mod_dir: PathBuf,
        /// DayZServer install dir whose keys/ the signatures are checked against.
        #[arg(long)]
        server: Option<PathBuf>,
    },
    /// Generate a <authority>.bikey + .biprivatekey signing keypair.
    Keygen {
        /// Authority name (becomes the key file stem).
        authority: String,
        /// Where to write the keys (default: ./keys).
        #[arg(long, short)]
        dir: Option<PathBuf>,
        /// DsUtils dir: use the official DSCreateKey instead of the built-in
        /// native keygen.
        #[arg(long)]
        tools: Option<PathBuf>,
    },
    /// Launch the native DayZServer with a built @Mod loaded, then report any
    /// captured file-mapped errors (build → launch → capture → navigate). Refused while
    /// another run's serverDZ_modfoundry.cfg is in the server folder.
    Run {
        /// The built `@Mod` dir to load via `-mod=` (content root; with --set, the set's mod at the source root).
        #[arg(required_unless_present = "set")]
        target: Option<PathBuf>,
        #[command(flatten)]
        flags: RunFlags,
    },
    /// Launch the DayZ client under Proton (via Steam) with local mods staged
    /// and auto-connect, capture the prefix logs, then report file-mapped errors.
    RunClient {
        #[command(flatten)]
        flags: RunClientFlags,
    },
    /// Launch the DayZ Diag server (and, by default, the Diag client that joins
    /// it) from the client install, capture both logs, then report file-mapped
    /// errors. A flag left out takes the project's saved setting where one applies.
    RunDiag {
        /// The built `@Mod` dir to load via `-mod=` (content root; with --set, the set's mod at the source root).
        #[arg(required_unless_present = "set")]
        target: Option<PathBuf>,
        #[command(flatten)]
        flags: RunDiagFlags,
    },
    /// Say whether a recorded run is still going, and how it ended.
    Status {
        /// The run id the launch was recorded under: the run_id a detached run answers with.
        run_id: String,
    },
    /// Parse the RPT / script.log files under a profiles/log dir and print the
    /// errors mapped back to source files.
    Errors {
        /// Profiles or log dir to parse.
        dir: PathBuf,
        /// Mod source root used to resolve logged file paths to host files; defaults to
        /// the current folder when it holds a config.cpp.
        #[arg(long)]
        source_root: Option<PathBuf>,
        /// A log file or folder from a run without this project's mods; errors also logged there are hidden.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// The project's known-error file (default `server/known-errors.json` under --source-root).
        #[arg(long)]
        known_errors: Option<PathBuf>,
        /// Add the error with this id to the known-error file.
        #[arg(long)]
        learn: Option<String>,
        /// What a learned error means, printed with it from now on.
        #[arg(long, requires = "learn")]
        meaning: Option<String>,
        /// How to fix a learned error, printed once per run.
        #[arg(long, requires = "learn")]
        hint: Option<String>,
        /// Print every error in full, baseline and known ones included.
        #[arg(long)]
        all: bool,
        /// Print the errors as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Binarize a config.cpp into RaP `config.bin` (CfgConvert `-bin` parity).
    ConfigBin {
        /// config.cpp to binarize.
        input: PathBuf,
        /// Output path (default: `config.bin` next to the input).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Convert a RaP `config.bin` back into config text.
    Derapify {
        /// config.bin to convert.
        input: PathBuf,
        /// Output path (default: stdout).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Convert a PNG, or every PNG in a folder, into a DayZ texture: .edds for UI art, .paa for models and everything else.
    /// A folder whose PNG names differ only in case, or are not valid UTF-8, is refused before anything is written.
    Image {
        /// A PNG file, or a folder of PNGs.
        input: PathBuf,
        /// Texture to write.
        #[arg(long, value_parser = ["edds", "paa"])]
        to: String,
        /// Output file, or the folder the outputs mirror into (default: beside each PNG).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Block format, only with `--to paa` (default: picked from the image).
        #[arg(long, value_parser = ["dxt1", "dxt5"])]
        format: Option<String>,
    },
    /// Pack a folder of PNGs into one DayZ texture and write the .imageset naming each image.
    Imageset {
        /// The folder of PNGs to pack; its subfolders are not read.
        input: PathBuf,
        /// The .imageset file to write; the textures are written beside it.
        #[arg(long)]
        out: PathBuf,
        /// The in-game folder the textures are packed at, e.g. "MyMod/gui/imagesets".
        #[arg(long)]
        game_dir: String,
    },
    /// Turn a TTF or OTF font into DayZ .fnt and .edds files at the given pixel sizes, with a size list a layout can name, or into one distance-field font that draws sharp at any size.
    Font {
        /// The TTF or OTF font to render.
        input: PathBuf,
        /// Pixel sizes to write, comma-separated, e.g. 10,12,14; each is the cell
        /// height unless `--sizing em`. Not needed with `--msdf`.
        #[arg(long, value_delimiter = ',')]
        sizes: Vec<u32>,
        /// The folder the .fnt, .edds and .xml files are written into.
        #[arg(long)]
        out: PathBuf,
        /// The file name stem (default: the font file's stem, lowercased, other characters turned into _).
        #[arg(long)]
        name: Option<String>,
        /// The character set: default or all.
        #[arg(long, value_parser = ["default", "all"])]
        chars: Option<String>,
        /// What a size measures: `line` (default) is the cell height, as in vanilla's
        /// fonts, so text drawn at that size fills it, and ink outside the font's Windows
        /// ascent and descent is cut; `em` is the em, as DayZ Tools' FontToTga.
        #[arg(long, value_parser = ["line", "em"])]
        sizing: Option<String>,
        /// Bake one distance-field font that DayZ scales to any size (no `--sizes`).
        #[arg(long)]
        msdf: bool,
    },
    /// Unpack a PBO, or every PBO of a mod folder, into a folder: its files as stored, its prefix in `$PBOPREFIX$`. Refuses to overwrite.
    PboUnpack {
        /// The PBO to unpack, or a mod folder.
        input: PathBuf,
        /// The folder to unpack into; created if missing.
        #[arg(long)]
        out: PathBuf,
    },
    /// Roll back the last recorded change-set (the newest one not yet undone).
    Undo {
        /// Only undo a change made under this folder.
        #[arg(long, value_name = "DIR")]
        within: Option<PathBuf>,
    },
    /// Inspect and manage the agent policy and the audit trail.
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
}

#[derive(Subcommand)]
enum DepsCommands {
    /// List the saved dependency mods of a mod project.
    List {
        /// The mod project's folder.
        root: PathBuf,
    },
    /// Save a dependency mod in the mod's .modfoundry/dependencies.json: a mod folder, or a Workshop id (all digits).
    Add {
        /// The mod project's folder.
        root: PathBuf,
        /// A dependency mod: its folder, or its Workshop id.
        dependency: String,
        /// Change this machine's saved list instead of the mod's .modfoundry/dependencies.json.
        #[arg(long)]
        machine: bool,
    },
    /// Remove a saved dependency mod from the mod's .modfoundry/dependencies.json (or this machine's list with --machine).
    Remove {
        /// The mod project's folder.
        root: PathBuf,
        /// A saved dependency mod: its folder, its Workshop id (`workshop:<id>` as `list` prints it), or the name `list` prints.
        dependency: String,
        /// Change this machine's saved list instead of the mod's .modfoundry/dependencies.json.
        #[arg(long)]
        machine: bool,
    },
}

#[derive(Subcommand)]
enum ScriptCommands {
    /// Print the parsed outline of Enforce sources as JSON (a `.c` file or a folder).
    Outline { path: PathBuf },
}

#[derive(Subcommand)]
enum SearchCommands {
    /// Build the search index for a game install, dependencies and docs folders.
    Index {
        /// DayZ install dir (default: host discovery).
        #[arg(long)]
        game: Option<PathBuf>,
        /// A dependency PBO, mod folder or loose source folder; repeatable.
        #[arg(long = "dependency", value_name = "PATH")]
        dependencies: Vec<PathBuf>,
        /// A docs folder of .md and .txt files; repeatable.
        #[arg(long = "docs", value_name = "DIR")]
        docs: Vec<PathBuf>,
        /// A source label whose hits are marked reference-only; repeatable.
        #[arg(long = "reference-only", value_name = "LABEL")]
        reference_only: Vec<String>,
        /// Local model folder (default: downloaded once into the app cache).
        #[arg(long)]
        model: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum LayoutCommands {
    /// Check a .layout file or a folder of them.
    Check {
        /// The .layout file or the folder of them.
        path: PathBuf,
        /// Mod root (default: the nearest folder holding a config.cpp).
        #[arg(long)]
        root: Option<PathBuf>,
        /// DayZ install dir (default: host discovery).
        #[arg(long)]
        game: Option<PathBuf>,
        /// A dependency PBO, mod folder or loose source folder; repeatable. Left out, the saved
        /// dependency mods of the project.
        #[arg(long = "dependency", value_name = "PATH")]
        dependencies: Vec<PathBuf>,
        /// The mod needs no other mod.
        #[arg(long, conflicts_with = "dependencies")]
        no_dependencies: bool,
    },
    /// Draw a .layout to a PNG and list what could not be drawn.
    Render {
        /// The .layout file.
        path: PathBuf,
        /// The PNG to write (default: the layout's name with .render.png, in the current folder).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Picture size as WIDTHxHEIGHT (default 1920x1080).
        #[arg(long)]
        size: Option<String>,
        /// Backdrop colour as #rrggbb or #rrggbbaa (default #202020).
        #[arg(long)]
        background: Option<String>,
        /// Draw hidden widgets faded.
        #[arg(long)]
        show_hidden: bool,
        /// Stringtable language (default english).
        #[arg(long)]
        language: Option<String>,
        /// Mod root (default: the nearest folder holding a config.cpp).
        #[arg(long)]
        root: Option<PathBuf>,
        /// DayZ install dir (default: host discovery).
        #[arg(long)]
        game: Option<PathBuf>,
        /// A dependency PBO, mod folder or loose source folder; repeatable. Left out, the saved
        /// dependency mods of the project.
        #[arg(long = "dependency", value_name = "PATH")]
        dependencies: Vec<PathBuf>,
        /// The mod needs no other mod.
        #[arg(long, conflicts_with = "dependencies")]
        no_dependencies: bool,
        /// Print the report as JSON, with every widget's box.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Print the most recent dispatched operations, newest first.
    Audit {
        /// Only rows for this actor key (`user`, `cli`, `automation`,
        /// `agent:<id>`).
        #[arg(long)]
        actor: Option<String>,
        /// How many rows to print.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Write (or replace) a policy rule.
    Grant {
        /// Actor key, or `*` for every actor.
        actor: String,
        /// Command id, or `*` for every command.
        command: String,
        /// Effect class the rule is scoped to (default: every class).
        #[arg(long)]
        effect: Option<String>,
        /// What the rule decides: `allow`, `deny` or `ask`.
        #[arg(long, default_value = "allow")]
        decision: String,
    },
    /// Drop a policy rule.
    Revoke {
        /// Actor key, or `*` for every actor.
        actor: String,
        /// Command id, or `*` for every command.
        command: String,
        /// Effect class the rule is scoped to (default: every class).
        #[arg(long)]
        effect: Option<String>,
    },
}

fn main() -> Result<()> {
    let started = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let (broker, registry) = match open_broker(cli.db) {
        Ok(opened) => opened,
        // A failure before `script outline` runs must not read as a file with syntax errors.
        Err(err) if matches!(&cli.command, Commands::Script { .. }) => {
            eprintln!("Error: {err:?}");
            std::process::exit(2);
        }
        Err(err) => return Err(err),
    };
    let session = Session {
        broker,
        registry,
        preview: cli.preview,
    };
    // `mf update` answers the same question itself.
    if !matches!(cli.command, Commands::Update { .. }) {
        *UPDATE_NOTICE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(UpdateNotice::start(started));
    }
    let result = match cli.command {
        Commands::Doctor => session
            .typed::<HostReport>("host.doctor", json!({}))
            .map(|report| doctor(&report)),
        Commands::Formats => session
            .typed::<Vec<String>>("formats.list", json!({}))
            .map(|names| formats(&names)),
        Commands::Changelog { since } => changelog(&session, since),
        Commands::Update { check } => update(&session, check),
        Commands::New {
            name,
            dir,
            display_name,
            description,
            author,
            mod_version,
            prefix,
            patch_class,
            required_addon,
            module,
            no_starter,
        } => {
            let dir = dir.unwrap_or_else(|| PathBuf::from(format!("./{name}")));
            new(
                &session,
                ScaffoldArgs {
                    name,
                    dir,
                    display_name,
                    description: description.unwrap_or_default(),
                    author: author.unwrap_or_default(),
                    version: mod_version,
                    prefix,
                    patch_class,
                    required_addons: (!required_addon.is_empty()).then_some(required_addon),
                    modules: (!module.is_empty()).then_some(module),
                    starter: no_starter.then_some(false),
                },
            )
        }
        Commands::Validate { target } => validate(&session, &target),
        Commands::Lint {
            target,
            game,
            dependencies,
            no_dependencies,
            builds,
        } => lint(
            &session,
            &target,
            game.as_deref(),
            given_dependencies(dependencies, no_dependencies),
            &builds,
        ),
        Commands::Deps { command } => deps(&session, command),
        Commands::Script {
            command: ScriptCommands::Outline { path },
        } => script_outline(&session, &path),
        Commands::Search {
            command,
            question,
            kind,
            source,
            top,
            game,
            model,
            json,
        } => match command {
            Some(SearchCommands::Index {
                game,
                dependencies,
                docs,
                reference_only,
                model,
            }) => search_index(
                &session,
                drop_nulls(json!({
                    "game": game,
                    "dependencies": dependencies,
                    "docs": docs,
                    "reference_only": reference_only,
                    "model": model,
                })),
            ),
            None => {
                let Some(question) = question else {
                    let mut cli = Cli::command();
                    let search = cli
                        .find_subcommand_mut("search")
                        .expect("the CLI defines `search`");
                    search
                        .error(
                            clap::error::ErrorKind::MissingRequiredArgument,
                            "a question is required, or `mf search index`",
                        )
                        .exit()
                };
                search(
                    &session,
                    drop_nulls(json!({
                        "question": question,
                        "kind": kind,
                        "source": source,
                        "top": top,
                        "game": game,
                        "model": model,
                    })),
                    json,
                )
            }
        },
        Commands::Layout {
            command:
                LayoutCommands::Check {
                    path,
                    root,
                    game,
                    dependencies,
                    no_dependencies,
                },
        } => layout_check(
            &session,
            with_dependencies(
                drop_nulls(json!({ "path": path, "root": root, "game": game })),
                given_dependencies(dependencies, no_dependencies).as_deref(),
            ),
        ),
        Commands::Layout {
            command:
                LayoutCommands::Render {
                    path,
                    out,
                    size,
                    background,
                    show_hidden,
                    language,
                    root,
                    game,
                    dependencies,
                    no_dependencies,
                    json,
                },
        } => {
            let out = out.unwrap_or_else(|| default_render_out(&path));
            layout_render(
                &session,
                with_dependencies(
                    drop_nulls(json!({
                        "path": path,
                        "root": root,
                        "game": game,
                        "out": out,
                        "size": size,
                        "background": background,
                        "language": language,
                        "show_hidden": show_hidden,
                        "boxes": json,
                    })),
                    given_dependencies(dependencies, no_dependencies).as_deref(),
                ),
                json,
            )
        }
        Commands::GameDiff {
            old,
            new,
            against,
            snapshots,
            json,
        } => game_diff(&session, old, new, against, snapshots, json),
        Commands::ConflictCheck {
            mods,
            set,
            root,
            json,
        } => conflict_check(&session, mods, set, root, json),
        Commands::GameSnapshot {
            game,
            snapshots,
            json,
        } => game_snapshot(&session, game, snapshots, json),
        Commands::Build {
            target,
            flags,
            set,
            source_root,
            list,
        } => {
            let args = build_request(target, set, source_root, flags);
            if list {
                build_list(&session, args)
            } else {
                build(&session, args)
            }
        }
        Commands::Publish {
            target,
            flags,
            content_only,
            change_note,
        } => publish(&session, target, flags, content_only, change_note),
        Commands::PublishItems => publish_items(&session),
        Commands::CheckSignatures { mod_dir, server } => {
            check_signatures(&session, mod_dir, server)
        }
        Commands::Keygen {
            authority,
            dir,
            tools,
        } => keygen(&session, &authority, dir, tools),
        Commands::Run { target, flags } => run(&session, run_args(target, flags)),
        Commands::RunClient { flags } => run_client(&session, run_client_args(flags)),
        Commands::RunDiag { target, flags } => run_diag(&session, run_diag_args(target, flags)),
        Commands::Errors {
            dir,
            source_root,
            baseline,
            known_errors,
            learn,
            meaning,
            hint,
            all,
            json,
        } => errors(
            &session,
            &dir,
            ErrorsFlags {
                source_root,
                baseline,
                known_errors,
                learn,
                meaning,
                hint,
                all,
                json,
            },
        ),
        Commands::Status { run_id } => status(&session, &run_id),
        Commands::ConfigBin { input, out } => config_bin(&session, input, out),
        Commands::Derapify { input, out } => derapify(&session, input, out),
        Commands::Image {
            input,
            to,
            out,
            format,
        } => image(&session, input, to, out, format),
        Commands::Imageset {
            input,
            out,
            game_dir,
        } => imageset(&session, input, out, game_dir),
        Commands::Font {
            input,
            sizes,
            out,
            name,
            chars,
            sizing,
            msdf,
        } => font(
            &session,
            FontArgs {
                path: input,
                sizes,
                out,
                name,
                chars,
                sizing,
                msdf,
            },
        ),
        Commands::PboUnpack { input, out } => pbo_unpack(&session, input, out),
        Commands::Undo { within } => undo(&session, within),
        Commands::Agent { command } => {
            if session.preview {
                bail!(
                    "`mf agent` changes the policy directly and cannot be previewed; \
                     run it without --preview"
                );
            }
            agent(&session.broker, command)
        }
    };
    finish_update_notice();
    result
}

static UPDATE_NOTICE: Mutex<Option<UpdateNotice>> = Mutex::new(None);

fn finish_update_notice() {
    let notice = UPDATE_NOTICE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(notice) = notice {
        notice.finish();
    }
}

/// Exits after the update notice has had its wait, so commands that end with a status still show it.
fn exit(code: i32) -> ! {
    finish_update_notice();
    std::process::exit(code)
}

/// Open the agent database the CLI dispatches through: `--db`, else
/// `$MODFOUNDRY_DB`, else the host data dir. A database the user named must
/// open; one this tool picked for them may not, and then the run continues
/// with a session-only policy and trail rather than failing a read-only verb.
/// The registry reads project settings from the same file, so left-out
/// arguments take them.
fn open_broker(db: Option<PathBuf>) -> Result<(Broker, RegistrySession)> {
    match open_db(&resolve_db(db)) {
        Ok(OpenedDb::File { store, path }) => {
            let registry = RegistrySession::new().with_db(path).with_actor("cli");
            let core = modfoundry_commands::registry::core_with(&registry);
            Ok((Broker::with_store(core, store), registry))
        }
        Ok(OpenedDb::SessionOnly { reason }) => {
            let registry = RegistrySession::new().with_actor("cli");
            let core = modfoundry_commands::registry::core_with(&registry);
            Ok((session_only(core, &reason)?, registry))
        }
        Err(message) => bail!("{message}"),
    }
}

/// Fall back to a database that lives for this process only, saying why.
fn session_only(core: modfoundry_core::Core, reason: &str) -> Result<Broker> {
    eprintln!("{}", session_only_warning(reason));
    Broker::in_memory(core).context("opening the in-memory agent database")
}

/// `mf agent …`: the broker's own administration surface. These read or write
/// the policy/audit tables directly rather than dispatching through the
/// registry — they are host configuration, not mod content operations, and
/// running them through the gate they configure would be circular.
fn agent(broker: &Broker, command: AgentCommands) -> Result<()> {
    match command {
        AgentCommands::Audit { actor, limit } => agent_audit(broker, actor, limit),
        AgentCommands::Grant {
            actor,
            command,
            effect,
            decision,
        } => agent_grant(broker, &actor, &command, effect.as_deref(), &decision),
        AgentCommands::Revoke {
            actor,
            command,
            effect,
        } => agent_revoke(broker, &actor, &command, effect.as_deref()),
    }
}

fn agent_audit(broker: &Broker, actor: Option<String>, limit: usize) -> Result<()> {
    let entries = broker.audit(actor.as_deref(), limit)?;
    if entries.is_empty() {
        println!("no operations audited");
        return Ok(());
    }
    for entry in &entries {
        let detail = entry
            .detail
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        println!(
            "{} {} {} {} {}{}",
            entry.at_unix,
            entry.actor,
            entry.command,
            effect_str(entry.effect),
            outcome_str(entry.outcome),
            detail
        );
    }
    Ok(())
}

fn agent_grant(
    broker: &Broker,
    actor: &str,
    command: &str,
    effect: Option<&str>,
    decision: &str,
) -> Result<()> {
    let effect = effect.map(effect_from_str).transpose()?;
    let decision = Decision::parse(decision)?;
    broker.grant(actor, command, effect, decision, now_unix())?;
    println!(
        "{actor} / {command} / {} -> {decision:?}",
        effect_selector(effect)
    );
    Ok(())
}

fn agent_revoke(broker: &Broker, actor: &str, command: &str, effect: Option<&str>) -> Result<()> {
    let effect = effect.map(effect_from_str).transpose()?;
    if !broker.revoke(actor, command, effect)? {
        bail!(
            "no rule matched {actor} / {command} / {}",
            effect_selector(effect)
        );
    }
    println!(
        "removed the rule for {actor} / {command} / {}",
        effect_selector(effect)
    );
    Ok(())
}

fn effect_selector(effect: Option<EffectKind>) -> &'static str {
    effect.map(effect_str).unwrap_or("*")
}

fn outcome_str(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Ok => "ok",
        AuditOutcome::Denied => "denied",
        AuditOutcome::Error => "error",
        AuditOutcome::Preview => "preview",
    }
}

/// One `mf` invocation: the broker, the registry session the live runs share
/// with a second core built for the signal handler, and the global `--preview`
/// switch. The switch is merged into every call's arguments, so `mf build
/// --preview` is the same request as an MCP client's `{"preview": true}`.
struct Session {
    broker: Broker,
    registry: RegistrySession,
    preview: bool,
}

impl Session {
    /// Dispatch through the registry on behalf of the terminal user.
    fn dispatch(&self, id: &str, mut args: Value) -> Result<Outcome> {
        if self.preview {
            if let Some(object) = args.as_object_mut() {
                object.insert(PREVIEW_ARG.into(), json!(true));
            }
        }
        self.dispatch_as_given(id, args)
    }

    /// Dispatch with `args` as they are, whatever `--preview` asks for.
    fn dispatch_as_given(&self, id: &str, args: Value) -> Result<Outcome> {
        self.broker
            .dispatch(Actor::CliSession, id, args)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Dispatch a mutation, rendering either what it did or what it would do.
    fn mutation(&self, id: &str, args: Value) -> Result<()> {
        match self.dispatch(id, args)? {
            Outcome::Changed { summary, .. } => {
                println!("{}", summary.description);
                Ok(())
            }
            Outcome::Preview(plan) => {
                print_plan(&plan);
                Ok(())
            }
            other => bail!("{id}: unexpected outcome {other:?}"),
        }
    }

    /// Dispatch a command whose result payload the caller renders generically.
    fn data(&self, id: &str, args: Value) -> Result<Value> {
        match self.dispatch(id, args)? {
            Outcome::Data(v) => {
                note_settings(&v);
                Ok(v)
            }
            other => bail!("{id}: unexpected outcome {other:?}"),
        }
    }

    /// Dispatch and decode the payload into the command's documented result type.
    fn typed<T: DeserializeOwned>(&self, id: &str, args: Value) -> Result<T> {
        let value = self.data(id, args)?;
        serde_json::from_value(value).with_context(|| format!("{id}: malformed result payload"))
    }
}

/// The line that names the saved settings a call filled in, when it filled any.
fn settings_line(result: &Value) -> Option<String> {
    let used: Vec<&str> = result["settings_used"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    (!used.is_empty()).then(|| format!("using saved settings: {}", used.join(", ")))
}

/// Prints the saved-settings line for a result, ahead of the command's own output.
fn note_settings(result: &Value) {
    if let Some(line) = settings_line(result) {
        println!("{line}");
    }
}

/// Render a change-set: what a mutation would touch, one line per file.
fn print_plan(plan: &Value) {
    let changes = plan["changes"].as_array();
    println!(
        "would change {} file(s) under {}",
        changes.map_or(0, Vec::len),
        plan["root"].as_str().unwrap_or("?"),
    );
    for change in changes.into_iter().flatten() {
        println!(
            "  {:>6} {}",
            change["op"].as_str().unwrap_or("?"),
            change["path"].as_str().unwrap_or("?")
        );
    }
}

fn print_run_plan(plan: &Value) {
    for line in run_plan_lines(plan) {
        println!("{line}");
    }
}

fn run_plan_lines(plan: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for launch in members(&plan["launches"]) {
        lines.push(format!(
            "would launch {}: {}",
            launch["role"].as_str().unwrap_or("?"),
            launch_line(launch)
        ));
        if let Some(cwd) = launch["cwd"].as_str() {
            lines.push(format!("  in {cwd}"));
        }
        let env: Vec<String> = members(&launch["env"])
            .filter_map(|pair| {
                Some(format!(
                    "{}={}",
                    pair.get(0)?.as_str()?,
                    shell_word(pair.get(1)?.as_str()?)
                ))
            })
            .collect();
        if !env.is_empty() {
            lines.push(format!("  env {}", env.join(" ")));
        }
        for name in members(&launch["env_remove"]).filter_map(Value::as_str) {
            lines.push(format!("  unset {name}"));
        }
    }
    let staged: Vec<&str> = members(&plan["steps"]).filter_map(step_path).collect();
    // A stale path the run relinks or removes is shown once, as that step.
    let leftovers: Vec<&str> = members(&plan["leftovers"])
        .filter_map(Value::as_str)
        .filter(|path| !staged.contains(path))
        .collect();
    if !leftovers.is_empty() {
        lines.push(format!(
            "would remove {} leftover(s) from a dead run:",
            leftovers.len()
        ));
        lines.extend(leftovers.iter().map(|path| format!("  {path}")));
    }
    let project: Vec<&Value> = members(&plan["project"]).collect();
    if !project.is_empty() {
        lines.push("would create in the project:".to_owned());
        lines.extend(project.into_iter().filter_map(stage_line));
    }
    let steps: Vec<String> = members(&plan["steps"]).filter_map(stage_line).collect();
    if !steps.is_empty() {
        lines.push(format!("would stage {} item(s):", steps.len()));
        lines.extend(steps);
    }
    lines
}

/// A launch as one pasteable command line: its program and arguments, each a shell word.
fn launch_line(launch: &Value) -> String {
    std::iter::once(&launch["program"])
        .chain(members(&launch["args"]))
        .map(|word| shell_word(word.as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `word` as a shell would read it back: plain words stay bare, anything else is single-quoted.
fn shell_word(word: &str) -> String {
    const SAFE: &str = "_-./:@%+,=";
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn stage_line(step: &Value) -> Option<String> {
    let op = step["op"].as_str()?;
    let (word, rest) = match op {
        "create_dir" => ("mkdir", step["path"].as_str()?.to_owned()),
        "link" | "relink" => (
            op,
            format!("{} -> {}", step["link"].as_str()?, step["target"].as_str()?),
        ),
        "remove" => ("remove", step["path"].as_str()?.to_owned()),
        "copy" => (
            "copy",
            format!("{} -> {}", step["from"].as_str()?, step["to"].as_str()?),
        ),
        "write" => (
            "write",
            format!(
                "{} ({} bytes)",
                step["path"].as_str()?,
                step["bytes"].as_u64()?
            ),
        ),
        _ => return None,
    };
    Some(format!("{word:>9} {rest}"))
}

fn step_path(step: &Value) -> Option<&str> {
    ["link", "path", "to"]
        .iter()
        .find_map(|key| step[*key].as_str())
}

fn members(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn new(session: &Session, args: ScaffoldArgs) -> Result<()> {
    let dir = args.dir.clone();
    let hint = build_hint(&dir, &args.name, args.prefix.as_deref());
    let out = session.dispatch("project.scaffold", json!(args))?;
    match out {
        Outcome::Changed { summary, .. } => {
            println!("{}", summary.description);
            println!("next: `mf validate {}`", dir.display());
            println!("build it with `{hint}`");
            Ok(())
        }
        Outcome::Preview(plan) => {
            print_plan(&plan);
            Ok(())
        }
        other => bail!("project.scaffold: unexpected outcome {other:?}"),
    }
}

/// `mf build` names the mod after its folder, so the name is spelled out only when they differ.
fn build_hint(dir: &Path, name: &str, prefix: Option<&str>) -> String {
    let name_arg = if modfoundry_commands::default_name(dir) == name {
        String::new()
    } else {
        format!(" --name {name}")
    };
    let prefix_arg = prefix
        .map(|prefix| format!(" --prefix {prefix}"))
        .unwrap_or_default();
    format!("mf build {}{name_arg}{prefix_arg}", dir.display())
}

/// 0 for hints and warnings, 1 for an error, 2 for a fatal: the worst severity
/// decides.
fn exit_code(severities: impl Iterator<Item = Severity>) -> i32 {
    match severities.max() {
        Some(Severity::Fatal) => 2,
        Some(Severity::Error) => 1,
        _ => 0,
    }
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Hint => "hint",
        Severity::Warning => "warn",
        Severity::Error => "error",
        Severity::Fatal => "fatal",
    }
}

fn print_diagnostic(d: &Diagnostic, path: &Path, at: Option<(usize, usize)>) {
    let at = at.map_or(String::new(), |(line, column)| format!(":{line}:{column}"));
    println!(
        "[{}] {} {}{at}: {}",
        d.code,
        severity_word(d.severity),
        path.display(),
        d.message
    );
}

fn print_findings(findings: &[FindingReport]) {
    let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
    for f in findings {
        let at = f.diagnostic.span.and_then(|span| {
            let text = file_text(&mut texts, &f.path)?;
            Some(modfoundry_core::text::line_col(text, span.start))
        });
        print_diagnostic(&f.diagnostic, &f.path, at);
    }
}

fn validate(session: &Session, target: &Path) -> Result<()> {
    let out = session.dispatch(
        "config.validate",
        json!(PathArgs {
            path: target.to_path_buf()
        }),
    )?;
    let Outcome::Diagnostics { diagnostics, .. } = out else {
        bail!("config.validate: unexpected outcome {out:?}");
    };
    if diagnostics.is_empty() {
        println!("ok: no issues in {}", target.display());
        return Ok(());
    }
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity >= Severity::Error)
        .count();
    let file = modfoundry_commands::config_file(target);
    let text = std::fs::read_to_string(&file).ok();
    for d in &diagnostics {
        let at = text
            .as_deref()
            .zip(d.span)
            .map(|(text, span)| modfoundry_core::text::line_col(text, span.start));
        print_diagnostic(d, &file, at);
    }
    println!(
        "{} issue(s), {} error(s) in {}",
        diagnostics.len(),
        errors,
        target.display()
    );
    exit(exit_code(diagnostics.iter().map(|d| d.severity)));
}

fn given_dependencies(dependencies: Vec<PathBuf>, no_dependencies: bool) -> Option<Vec<PathBuf>> {
    if no_dependencies {
        Some(Vec::new())
    } else {
        (!dependencies.is_empty()).then_some(dependencies)
    }
}

fn lint(
    session: &Session,
    target: &Path,
    game: Option<&Path>,
    dependencies: Option<Vec<PathBuf>>,
    builds: &[String],
) -> Result<()> {
    let mut args = with_dependencies(
        drop_nulls(json!({ "path": target, "game": game })),
        dependencies.as_deref(),
    );
    if !builds.is_empty() {
        args["targets"] = json!(builds);
    }
    let report: ScriptLintReport = session.typed("script.lint", args)?;
    if let Some(error) = &report.index_error {
        eprintln!("warning: no script index, so type checks are skipped: {error}");
    }
    print_findings(&report.findings);
    print_game_lines(&report.game);
    if report.findings.is_empty() {
        println!("ok: no issues in {}", target.display());
        return Ok(());
    }
    println!("{} issue(s) in {}", report.findings.len(), target.display());
    exit(exit_code(
        report.findings.iter().map(|f| f.diagnostic.severity),
    ));
}

fn deps(session: &Session, command: DepsCommands) -> Result<()> {
    match command {
        DepsCommands::List { root } => print_saved(session, &root),
        DepsCommands::Add {
            root,
            dependency,
            machine,
        } => edit_dependency(session, &root, "add", dependency, machine),
        DepsCommands::Remove {
            root,
            dependency,
            machine,
        } => edit_dependency(session, &root, "remove", dependency, machine),
    }
}

/// A saved dependency as the `project.dependencies` and `project.edit_dependencies` replies give it.
#[derive(Deserialize)]
struct SavedDependencies {
    dependencies: Vec<SavedDependency>,
}

#[derive(Deserialize)]
struct EditPreview {
    after: Vec<SavedDependency>,
    note: Option<String>,
}

fn print_saved(session: &Session, root: &Path) -> Result<()> {
    let saved: SavedDependencies =
        session.typed("project.dependencies", json!({ "root": root }))?;
    print_dependency_rows(&saved.dependencies);
    Ok(())
}

fn edit_dependency(
    session: &Session,
    root: &Path,
    action: &str,
    dependency: String,
    machine: bool,
) -> Result<()> {
    let target = if machine { "machine" } else { "project" };
    let args = json!({ "root": root, "action": action, "dependency": dependency, "where": target });
    match session.dispatch("project.edit_dependencies", args)? {
        Outcome::Changed { summary, .. } => {
            println!("{}", summary.description);
            print_saved(session, root)
        }
        Outcome::Preview(plan) => {
            let preview: EditPreview = serde_json::from_value(plan)?;
            match preview.note {
                Some(note) => println!("{note}"),
                None => {
                    println!("would save:");
                    print_dependency_rows(&preview.after);
                }
            }
            Ok(())
        }
        other => bail!("project.edit_dependencies: unexpected outcome {other:?}"),
    }
}

fn print_dependency_rows(rows: &[SavedDependency]) {
    if rows.is_empty() {
        println!("no saved dependencies");
    }
    for row in rows {
        println!("{}", dependency_line(row));
    }
}

fn dependency_line(row: &SavedDependency) -> String {
    let location = match &row.source {
        ModSource::Folder { path } => path.display().to_string(),
        ModSource::Workshop { id } => format!("workshop:{id}"),
    };
    let off = if row.enabled { "" } else { "  (off)" };
    let origin = match row.origin {
        DependencyOrigin::Project => "(file)",
        DependencyOrigin::Machine => "(machine)",
        DependencyOrigin::Defaults => "(defaults)",
    };
    format!("{}  {location}{off}  {origin}", row.name)
}

/// The game check's lines under the findings: why it was skipped, else the modules it refused and its notes.
fn print_game_lines(game: &GameReport) {
    if let Some(reason) = &game.skipped {
        println!("note: game check skipped: {reason}");
        return;
    }
    for module in &game.failed_modules {
        println!(
            "game check: module {} refused on {}",
            module.module,
            module.targets.join(", ")
        );
    }
    for note in &game.notes {
        println!("note: {note}");
    }
}

fn with_dependencies(mut args: Value, dependencies: Option<&[PathBuf]>) -> Value {
    if let Some(dependencies) = dependencies {
        args["dependencies"] = json!(dependencies);
    }
    args
}

/// `mf search index`: the build's notes, its counts and where the index went.
fn search_index(session: &Session, args: Value) -> Result<()> {
    let report = session.data("search.index", args)?;
    for note in strings(&report["notes"]) {
        println!("{note}");
    }
    for warning in strings(&report["warnings"]) {
        eprintln!("warning: {warning}");
    }
    let seconds = report["millis"].as_u64().unwrap_or(0) as f64 / 1000.0;
    println!(
        "indexed {} files, {} pieces from {} sources in {seconds:.1} s",
        report["files"], report["chunks"], report["sources"]
    );
    println!(
        "index: {} (build {})",
        text(&report["index"]),
        text(&report["build"])
    );
    Ok(())
}

/// `mf search "<question>"`: one block per hit, a summary line, or the whole report as JSON.
fn search(session: &Session, args: Value, json: bool) -> Result<()> {
    let report = session.data("search.query", args)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let hits = report["hits"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for (i, hit) in hits.iter().enumerate() {
        println!("{}. {}", i + 1, plain(&hit_line(hit)));
        for line in text(&hit["snippet"]).lines() {
            println!("   {}", plain(line));
        }
    }
    let build = text(&report["build"]);
    if hits.is_empty() {
        println!("no hits (build {build})");
    } else {
        println!(
            "{} hit(s) in {} ms (build {build})",
            hits.len(),
            report["millis"]
        );
    }
    Ok(())
}

/// `line` with every control character but a tab left out, so an indexed file never drives the terminal.
fn plain(line: &str) -> String {
    line.chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

/// A hit's first line: where it is, what it is, and how the ranking scored it.
fn hit_line(hit: &Value) -> String {
    let first = hit["lines"][0].as_u64().unwrap_or(0);
    let last = hit["lines"][1].as_u64().unwrap_or(first);
    let lines = if first == last {
        first.to_string()
    } else {
        format!("{first}-{last}")
    };
    let mut parts = vec![
        format!("{}:{lines}", text(&hit["path"])),
        text(&hit["kind"]).to_string(),
    ];
    if let Some(name) = hit_name(hit) {
        parts.push(name);
    }
    parts.push(text(&hit["source"]).to_string());
    parts.push(format!("{:.4}", hit["score"].as_f64().unwrap_or(0.0)));
    if hit["stale"].as_bool() == Some(true) {
        parts.push("stale".into());
    }
    if hit["reference_only"].as_bool() == Some(true) {
        parts.push("reference-only".into());
    }
    parts.join("  ")
}

/// `Class::method`, `Class` or a free function's name; nothing when the hit has neither.
fn hit_name(hit: &Value) -> Option<String> {
    match (hit["class"].as_str(), hit["method"].as_str()) {
        (Some(class), Some(method)) => Some(format!("{class}::{method}")),
        (Some(name), None) | (None, Some(name)) => Some(name.to_string()),
        (None, None) => None,
    }
}

/// `mf layout check`: the warnings, each finding at its line and column, then the count.
fn layout_check(session: &Session, args: Value) -> Result<()> {
    let report = session.data("layout.check", args)?;
    for warning in strings(&report["warnings"]) {
        eprintln!("warning: {warning}");
    }
    let findings: Vec<FindingReport> = serde_json::from_value(report["findings"].clone())
        .context("layout.check: malformed result payload")?;
    print_findings(&findings);
    if findings.is_empty() {
        println!("ok: no issues in {} file(s)", report["files"]);
    } else {
        println!("{} issue(s) in {} file(s)", findings.len(), report["files"]);
    }
    exit(exit_code(findings.iter().map(|f| f.diagnostic.severity)));
}

/// `<stem>.render.png` in the current folder, so a render never lands on art named after the layout.
fn default_render_out(layout: &Path) -> PathBuf {
    let stem = layout.file_stem().unwrap_or(std::ffi::OsStr::new("layout"));
    let mut name = stem.to_os_string();
    name.push(".render.png");
    PathBuf::from(name)
}

/// `mf layout render`: the warnings, each finding at its line and column, then the summary line; `--json` prints the report.
fn layout_render(session: &Session, args: Value, json_out: bool) -> Result<()> {
    let report = match session.dispatch("layout.render", args)? {
        Outcome::Data(report) => {
            match settings_line(&report) {
                Some(line) if json_out => eprintln!("{line}"),
                Some(line) => println!("{line}"),
                None => {}
            }
            report
        }
        Outcome::Preview(plan) => {
            print_plan(&plan);
            return Ok(());
        }
        other => bail!("layout.render: unexpected outcome {other:?}"),
    };
    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for warning in strings(&report["warnings"]) {
        eprintln!("warning: {warning}");
    }
    let findings: Vec<FindingReport> = serde_json::from_value(report["findings"].clone())
        .context("layout.render: malformed result payload")?;
    print_findings(&findings);
    println!(
        "rendered {}x{}: {} widgets, {} drawn, {} hidden skipped, {} findings -> {}",
        report["width"],
        report["height"],
        report["widgets"],
        report["drawn"],
        report["hidden_skipped"],
        findings.len(),
        report["png"].as_str().unwrap_or_default()
    );
    Ok(())
}

fn data_or_exit(session: &Session, command: &str, args: Value) -> Value {
    match session.data(command, args) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("Error: {err:?}");
            exit(2);
        }
    }
}

/// `mf game-diff`: the warnings to stderr, the text report or the JSON, and the exit code.
fn game_diff(
    session: &Session,
    old: String,
    new: String,
    against: Vec<PathBuf>,
    snapshots: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let value = data_or_exit(
        session,
        "game.diff",
        drop_nulls(json!({
            "old": old,
            "new": new,
            "against": against,
            "snapshots": snapshots,
        })),
    );
    for (label, side) in [("old", &value["old"]), ("new", &value["new"])] {
        for warning in strings(&side["warnings"]) {
            eprintln!("warning: {label}: {warning}");
        }
    }
    for m in value["mods"].as_array().into_iter().flatten() {
        for warning in strings(&m["warnings"]) {
            eprintln!("warning: {}: {warning}", text(&m["name"]));
        }
    }
    let breaks = value["summary"]["breaks"].as_u64().unwrap_or(0);
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let report: modfoundry_commands::game_diff::GameDiffReport =
            serde_json::from_value(value).context("game.diff: malformed result payload")?;
        println!("{}", modfoundry_commands::game_diff::render_text(&report));
    }
    exit(if breaks > 0 { 1 } else { 0 });
}

/// `mf conflict-check`: the warnings to stderr, the text report or the JSON, and the exit code.
fn conflict_check(
    session: &Session,
    mods: Vec<PathBuf>,
    set: Option<String>,
    root: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let mut args = json!({ "set": set, "root": root.as_deref().map(lossy_path) });
    if !mods.is_empty() {
        args["mods"] = json!(mods.iter().map(|m| lossy_path(m)).collect::<Vec<_>>());
    }
    let value = data_or_exit(session, "conflict.check", drop_nulls(args));
    for warning in strings(&value["warnings"]) {
        eprintln!("warning: {warning}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "{}",
            modfoundry_commands::conflict_check::render_text(&value)
        );
    }
    let fatal = value["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|finding| matches!(text(&finding["severity"]), "error" | "fatal"));
    exit(if fatal { 1 } else { 0 });
}

/// `mf game-snapshot`: the warnings to stderr, then the saved build or the JSON.
fn game_snapshot(
    session: &Session,
    game: Option<PathBuf>,
    snapshots: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let value = data_or_exit(
        session,
        "game.snapshot",
        drop_nulls(json!({ "game": game, "snapshots": snapshots })),
    );
    for warning in strings(&value["warnings"]) {
        eprintln!("warning: {warning}");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let saved: modfoundry_commands::game_snapshot::SavedSnapshot =
            serde_json::from_value(value).context("game.snapshot: malformed result payload")?;
        println!(
            "saved build {} ({} scripts, {} configs, {} layouts) to {}",
            saved.build,
            saved.counts.scripts,
            saved.counts.configs,
            saved.counts.layouts,
            saved.path.display()
        );
    }
    Ok(())
}

/// The strings of a JSON array, skipping anything else.
fn strings(value: &Value) -> impl Iterator<Item = &str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("?")
}

fn script_outline(session: &Session, path: &Path) -> Result<()> {
    let outline = data_or_exit(
        session,
        "script.outline",
        json!(PathArgs {
            path: path.to_path_buf()
        }),
    );
    println!("{}", serde_json::to_string_pretty(&outline)?);
    let failing = outline["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| file["errors"].as_array().is_some_and(|e| !e.is_empty()))
        .count();
    if failing > 0 {
        eprintln!("{failing} file(s) with syntax errors");
        exit(1);
    }
    Ok(())
}

/// `path`'s text, read at most once while rendering a lint run.
fn file_text<'a>(texts: &'a mut HashMap<PathBuf, Option<String>>, path: &Path) -> Option<&'a str> {
    texts
        .entry(path.to_path_buf())
        // Lossy, as the commands read sources, so spans land on the same offsets.
        .or_insert_with(|| {
            std::fs::read(path)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
        .as_deref()
}

/// The update notice for one run: the saved one is printed up front, and a due check may add a new one.
struct UpdateNotice {
    started: Instant,
    printed: bool,
    check: Option<mpsc::Receiver<Option<String>>>,
}

impl UpdateNotice {
    fn start(started: Instant) -> Self {
        let start = update::start_notice();
        if let Some(saved) = &start.saved {
            eprintln!("{saved}");
        }
        Self {
            started,
            printed: start.saved.is_some(),
            check: start.check,
        }
    }

    fn finish(self) {
        let Some(check) = self.check else {
            return;
        };
        let left = NOTICE_WAIT.saturating_sub(self.started.elapsed());
        if let Ok(Some(notice)) = check.recv_timeout(left) {
            if !self.printed {
                eprintln!("{notice}");
            }
        }
    }
}

fn doctor(report: &HostReport) {
    println!("host platform: {}", report.platform);
    println!("steam libraries found: {}", report.steam_libraries.len());
    for lib in &report.steam_libraries {
        println!("  {}", lib.root.display());
        println!(
            "    dayz workshop root: {}",
            lib.dayz_workshop_root.display()
        );
    }
    match &report.dayz_game {
        Some(p) => println!("dayz game: {}", p.display()),
        None => println!("dayz game: NOT FOUND"),
    }
    match &report.dayz_tools {
        Some(p) => println!("dayz tools: {}", p.display()),
        None => println!("dayz tools: NOT FOUND"),
    }
    for line in gpu_lines(&report.gpu) {
        println!("{line}");
    }
    if let Some(notice) = &report.update {
        println!("{notice}");
    }
}

fn gpu_lines(gpu: &GpuReport) -> Vec<String> {
    let mut lines = Vec::new();
    match &gpu.helper {
        Some(p) => lines.push(format!("gpu helper: {}", p.display())),
        None => lines.push("gpu helper: NOT FOUND (search embeds on the CPU)".to_owned()),
    }
    lines.push(if gpu.driver {
        "cuda driver (libcuda.so.1): found".to_owned()
    } else {
        "cuda driver (libcuda.so.1): NOT FOUND".to_owned()
    });
    lines.push(match &gpu.libraries {
        Some(p) => format!("cuda libraries folder: {}", p.display()),
        None => "cuda libraries folder: unknown (no data folder)".to_owned(),
    });
    lines.push(if gpu.missing.is_empty() {
        "cuda libraries: all found".to_owned()
    } else {
        format!(
            "cuda libraries: missing {} (run tools/setup-cuda.sh)",
            gpu.missing.join(", ")
        )
    });
    lines.push(match (gpu.device_ok, &gpu.device_error) {
        (Some(true), _) => "gpu device: ready".to_owned(),
        (Some(false), Some(err)) => {
            format!("gpu device: FAILED: {err} (search embeds on the CPU)")
        }
        (Some(false), None) => "gpu device: FAILED (search embeds on the CPU)".to_owned(),
        (None, _) => "gpu device: not checked (no GPU helper)".to_owned(),
    });
    lines
}

fn build(session: &Session, args: Value) -> Result<()> {
    let set = args.get("set").is_some();
    match session.dispatch("build.run", args)? {
        Outcome::Data(Value::Array(reports)) if set => {
            for report in &reports {
                println!("building {}", report["name"].as_str().unwrap_or("?"));
                print_report(report)?;
            }
            Ok(())
        }
        Outcome::Data(v) => print_report(&v),
        Outcome::Preview(plan) if set => {
            let entries = plan
                .as_array()
                .context("build.run: a set preview is a list of change-sets")?;
            for entry in entries {
                println!("building {}", entry["name"].as_str().unwrap_or("?"));
                note_settings(entry);
                print_plan(&entry["plan"]);
            }
            Ok(())
        }
        Outcome::Preview(plan) => {
            note_settings(&plan);
            print_plan(&plan);
            Ok(())
        }
        other => bail!("build.run: unexpected outcome {other:?}"),
    }
}

/// One build report as `mf build` prints it.
fn print_report(v: &Value) -> Result<()> {
    note_settings(v);
    let warning = v.get("warning").and_then(Value::as_str).map(str::to_owned);
    let report: BuildReport =
        serde_json::from_value(v.clone()).context("build.run: malformed result payload")?;
    println!("built @{} -> {}", report.name, report.output_dir.display());
    let marker = if report.signed {
        "(signed)"
    } else {
        "(unsigned)"
    };
    for a in &report.addons {
        println!("  addons/{a} {marker}");
    }
    if let Some(warning) = &warning {
        println!("warning: {warning}");
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    if let Some(note) = &report.note {
        println!("note: {note}");
    }
    Ok(())
}

fn build_list(session: &Session, args: Value) -> Result<()> {
    match session.dispatch("build.contents", args)? {
        Outcome::Data(listing) => {
            note_settings(&listing);
            print_contents(&listing);
            Ok(())
        }
        other => bail!("build.contents: unexpected outcome {other:?}"),
    }
}

/// Render `mf build --list`: one line per file, then the packed total and the warnings.
fn print_contents(listing: &Value) {
    let files = listing["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut packed = 0_usize;
    let mut packed_bytes = 0_u64;
    for file in files {
        let included = file["decision"]["included"].as_bool() == Some(true);
        let size = file["size"].as_u64().unwrap_or(0);
        if included {
            packed += 1;
            packed_bytes += size;
        }
        println!(
            "{} {size:>8}  {}  [{}]",
            if included { '+' } else { '-' },
            file["path"].as_str().unwrap_or("?"),
            file["why"].as_str().unwrap_or("?")
        );
    }
    println!(
        "{packed} of {} files packed, {packed_bytes} bytes",
        files.len()
    );
    for warning in listing["warnings"].as_array().into_iter().flatten() {
        println!("warning: {}", warning.as_str().unwrap_or("?"));
    }
}

/// The arguments `mf publish` sends: the build's, plus the upload options given.
fn publish_args(
    target: PathBuf,
    flags: BuildFlags,
    content_only: bool,
    change_note: Option<String>,
) -> Value {
    let mut args = build_args(target, flags);
    if content_only {
        args["content_only"] = json!(true);
    }
    if let Some(note) = change_note {
        args["change_note"] = json!(note);
    }
    args
}

fn publish(
    session: &Session,
    target: PathBuf,
    flags: BuildFlags,
    content_only: bool,
    change_note: Option<String>,
) -> Result<()> {
    let args = publish_args(target, flags, content_only, change_note);
    if session.preview {
        return publish_plan(session, args);
    }
    install_cancel_handler(session)?;
    let mod_root = PathBuf::from(args["mod_root"].as_str().unwrap_or_default());
    let result = upload(session, args)?;
    print_upload(&result, &mod_root);
    Ok(())
}

/// `publish.prepare` only stages build output, so `--preview` is answered by running it unpreviewed.
fn publish_plan(session: &Session, args: Value) -> Result<()> {
    match session.dispatch_as_given("publish.prepare", args)? {
        Outcome::Data(plan) => {
            note_settings(&plan);
            let diagnostics = print_upload_plan(&plan);
            exit(exit_code(diagnostics.iter().map(|d| d.severity)));
        }
        other => bail!("publish.prepare: unexpected outcome {other:?}"),
    }
}

/// Runs `publish.upload` and prints each change of its step to stderr while it runs.
fn upload(session: &Session, args: Value) -> Result<Value> {
    let status = session.registry.publish_status();
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            use std::io::Write as _;
            let mut shown = None;
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(Duration::from_secs(1))
            {
                if let Some(state) = status.current().filter(|state| Some(*state) != shown) {
                    // Never ends `mf` mid-upload: a closed stderr only loses the progress lines.
                    let _ = writeln!(std::io::stderr(), "{}", progress_line(state));
                    shown = Some(state);
                }
            }
        });
        let result = session.data("publish.upload", args);
        drop(stop);
        result
    })
}

fn progress_line(state: UploadState) -> String {
    match state {
        UploadState::Preparing => "preparing".into(),
        UploadState::Creating => "creating the item".into(),
        UploadState::Uploading { stage, done, total } => format!(
            "uploading: {} {done}/{total} bytes",
            json!(stage).as_str().unwrap_or_default()
        ),
        UploadState::Deleting => "deleting the new item".into(),
    }
}

fn print_upload_plan(plan: &Value) -> Vec<Diagnostic> {
    println!("title {}", plan["title"].as_str().unwrap_or("?"));
    match plan["id"].as_u64() {
        Some(id) => println!("id {id}"),
        None => println!("new item"),
    }
    if plan["content_only"] == true {
        println!("content only");
    }
    println!(
        "visibility {}",
        plan["visibility"].as_str().unwrap_or("unchanged")
    );
    let tags: Vec<&str> = plan["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    println!(
        "tags {}",
        if tags.is_empty() {
            "none".to_owned()
        } else {
            tags.join(", ")
        }
    );
    for tag in plan["kv_tags"].as_array().into_iter().flatten() {
        println!(
            "{} = {}",
            tag["key"].as_str().unwrap_or("?"),
            tag["value"].as_str().unwrap_or("?")
        );
    }
    let mod_root = PathBuf::from(plan["mod_root"].as_str().unwrap_or_default());
    let diagnostics = diagnostics_of(&plan["diagnostics"]);
    for diagnostic in &diagnostics {
        print_diagnostic(diagnostic, &mod_root, None);
    }
    println!(
        "{}",
        if plan["ready"] == true {
            "ready"
        } else {
            "not ready"
        }
    );
    diagnostics
}

fn print_upload(result: &Value, mod_root: &Path) {
    let kind = if result["new_item"] == true {
        "new item"
    } else {
        "updated"
    };
    println!(
        "published \"{}\" -> {} ({kind})",
        result["title"].as_str().unwrap_or("?"),
        result["url"].as_str().unwrap_or("?")
    );
    println!(
        "under {} ({})",
        result["account"]["name"].as_str().unwrap_or("?"),
        result["account"]["steam_id"]
    );
    for warning in diagnostics_of(&result["warnings"]) {
        print_diagnostic(&warning, mod_root, None);
    }
    if result["needs_legal_agreement"] == true {
        println!("accept the Workshop legal agreement before the item shows: https://steamcommunity.com/sharedfiles/workshoplegalagreement");
    }
}

fn publish_items(session: &Session) -> Result<()> {
    let listing = match session.dispatch_as_given("publish.items", json!({}))? {
        Outcome::Data(listing) => listing,
        other => bail!("publish.items: unexpected outcome {other:?}"),
    };
    println!(
        "account {} ({})",
        listing["account"]["name"].as_str().unwrap_or("?"),
        listing["account"]["steam_id"]
    );
    for item in listing["items"].as_array().into_iter().flatten() {
        println!(
            "{}  {}  {}",
            item["id"],
            item["visibility"].as_str().unwrap_or("?"),
            item["title"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}

fn check_signatures(session: &Session, mod_dir: PathBuf, server: Option<PathBuf>) -> Result<()> {
    let value = session.data(
        "sign.check",
        json!(SignCheckArgs {
            mod_dir: mod_dir.clone(),
            server_dir: server,
        }),
    )?;
    let diagnostics: Vec<Diagnostic> = serde_json::from_value(value["diagnostics"].clone())
        .context("sign.check: malformed result payload")?;
    if diagnostics.is_empty() {
        println!("ok: signatures match in {}", mod_dir.display());
        return Ok(());
    }
    for diagnostic in &diagnostics {
        print_diagnostic(diagnostic, &mod_dir, None);
    }
    exit(exit_code(diagnostics.iter().map(|d| d.severity)));
}

/// The diagnostics in a command's answer; anything that does not parse is left out.
fn diagnostics_of(value: &Value) -> Vec<Diagnostic> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn config_bin(session: &Session, input: PathBuf, out: Option<PathBuf>) -> Result<()> {
    session.mutation("config.binarize", json!(ConvertArgs { path: input, out }))
}

fn derapify(session: &Session, input: PathBuf, out: Option<PathBuf>) -> Result<()> {
    // Without `out` the derapified text *is* the result; with it, the command
    // writes a file and reports what it wrote.
    if out.is_some() {
        return session.mutation("config.derapify", json!(ConvertArgs { path: input, out }));
    }
    let value = session.data(
        "config.derapify",
        json!(ConvertArgs {
            path: input,
            out: None
        }),
    )?;
    // The derapified text already ends in a newline; print it verbatim so stdout
    // is exactly what `--out` would write.
    print!("{}", value.as_str().context("derapified text")?);
    Ok(())
}

fn image(
    session: &Session,
    input: PathBuf,
    to: String,
    out: Option<PathBuf>,
    format: Option<String>,
) -> Result<()> {
    session.mutation(
        "image.convert",
        json!(ImageArgs {
            path: input,
            to,
            out,
            format
        }),
    )
}

fn imageset(session: &Session, input: PathBuf, out: PathBuf, game_dir: String) -> Result<()> {
    session.mutation(
        "imageset.build",
        json!(ImagesetArgs {
            path: input,
            out,
            game_dir
        }),
    )
}

fn font(session: &Session, args: FontArgs) -> Result<()> {
    session.mutation("font.build", json!(args))
}

fn pbo_unpack(session: &Session, input: PathBuf, out: PathBuf) -> Result<()> {
    session.mutation("pbo.unpack", json!(PboUnpackArgs { path: input, out }))
}

fn undo(session: &Session, within: Option<PathBuf>) -> Result<()> {
    // `--preview` must never do the thing it asks about: `workspace.undo` has no
    // plan of its own (the change-set it would invert is the recorded one), and
    // undoing is destructive by definition.
    if session.preview {
        bail!(
            "`mf undo` cannot be previewed: the change-set it replays is already recorded — \
             run it without --preview to apply it"
        );
    }
    let within = within.map(std::path::absolute).transpose()?;
    let undone = match &within {
        Some(dir) => session
            .broker
            .undo_within(Actor::CliSession, dir, now_unix())?,
        None => session.broker.undo(Actor::CliSession, now_unix())?,
    };
    let Some(undone) = undone else {
        match &within {
            Some(dir) => println!("nothing to undo under `{}`", dir.display()),
            None => println!("nothing to undo"),
        }
        return Ok(());
    };
    match &undone.outcome {
        Outcome::Changed { summary, .. } => {
            println!("{} (change-set {})", summary.description, undone.change_set)
        }
        other => bail!("workspace.undo: unexpected outcome {other:?}"),
    }
    if !undone.first {
        println!(
            "note: another session had already reversed change-set {}",
            undone.change_set
        );
    }
    Ok(())
}

/// The source root a run sends: the one typed, else the current folder for a launch set.
fn set_source_root(typed: Option<PathBuf>, has_set: bool) -> Option<PathBuf> {
    typed.or_else(|| has_set.then(|| std::env::current_dir().unwrap_or_else(|_| ".".into())))
}

/// The arguments `mf run` sends; without `--timeout` the command's default applies.
fn run_args(target: Option<PathBuf>, flags: RunFlags) -> Value {
    let has_test_mods = !flags.test_mods.is_empty();
    let has_expect = !flags.expect_lines.is_empty();
    let has_fail = !flags.fail_lines.is_empty();
    let has_allow = !flags.allow_errors.is_empty();
    let has_target = target.is_some();
    let source_root = set_source_root(flags.source_root, flags.set.is_some());
    let mut args = drop_nulls(json!(RunServerArgs {
        mod_dir: target.unwrap_or_default(),
        server_dir: flags.server,
        config: flags.config,
        set: flags.set,
        set_config: None,
        port: flags.port,
        profiles_dir: None,
        timeout_secs: flags.timeout.unwrap_or_default(),
        source_root,
        run_id: None,
        test_mods: flags.test_mods,
        expect_lines: flags.expect_lines,
        fail_lines: flags.fail_lines,
        allow_errors: flags.allow_errors,
    }));
    only_if_given(&mut args, "mod_dir", has_target);
    only_if_given(&mut args, "timeout_secs", flags.timeout.is_some());
    only_if_given(&mut args, "test_mods", has_test_mods);
    only_if_given(&mut args, "expect_lines", has_expect);
    only_if_given(&mut args, "fail_lines", has_fail);
    only_if_given(&mut args, "allow_errors", has_allow);
    args
}

/// The arguments `mf run-client` sends; without `--timeout` the command's default applies.
fn run_client_args(flags: RunClientFlags) -> Value {
    let source_root = set_source_root(flags.source_root, flags.set.is_some());
    let has_test_mods = !flags.test_mods.is_empty();
    let has_expect = !flags.expect_lines.is_empty();
    let has_fail = !flags.fail_lines.is_empty();
    let has_allow = !flags.allow_errors.is_empty();
    let mut args = drop_nulls(json!(RunClientCommandArgs {
        client: RunClientArgs {
            mods: flags.mods,
            set: flags.set,
            game_dir: flags.game_dir,
            steam_dir: flags.steam_dir,
            launcher: flags.launcher,
            connect: None,
            profile_name: flags.name,
            extra: flags.extra,
            window: flags.window,
            windowed: !flags.fullscreen,
            timeout_secs: flags.timeout.unwrap_or_default(),
            settle_secs: modfoundry_commands::CLIENT_SETTLE_SECS,
            source_root,
            test_mods: flags.test_mods,
            expect_lines: flags.expect_lines,
            fail_lines: flags.fail_lines,
            allow_errors: flags.allow_errors,
        },
        connect: flags.connect,
        run_id: None,
    }));
    only_if_given(&mut args, "timeout_secs", flags.timeout.is_some());
    only_if_given(&mut args, "test_mods", has_test_mods);
    only_if_given(&mut args, "windowed", flags.windowed || flags.fullscreen);
    only_if_given(&mut args, "expect_lines", has_expect);
    only_if_given(&mut args, "fail_lines", has_fail);
    only_if_given(&mut args, "allow_errors", has_allow);
    args
}

/// The timeout a launch line names: the one sent, or the command's own default.
fn timeout_phrase(args: &Value) -> String {
    match args["timeout_secs"].as_u64() {
        Some(secs) => format!("for up to {secs}s"),
        None => "for the default time".to_owned(),
    }
}

fn run_target(args: &Value) -> String {
    if let Some(target) = args["mod_dir"].as_str() {
        return target.to_owned();
    }
    match args["set"].as_str() {
        Some(set) => format!("set {set}"),
        None => String::new(),
    }
}

fn preview_run(session: &Session, id: &str, args: Value) -> Result<()> {
    match session.dispatch(id, args)? {
        Outcome::Preview(plan) => {
            note_settings(&plan);
            print_run_plan(&plan);
            Ok(())
        }
        other => bail!("{id}: unexpected outcome {other:?}"),
    }
}

fn run(session: &Session, args: Value) -> Result<()> {
    if session.preview {
        let thrown_away = args.get("profiles_dir").is_none() && args.get("source_root").is_none();
        preview_run(session, "diag.run", args)?;
        if thrown_away {
            println!(
                "note: without --source-root that capture folder is throwaway; the next run \
                 picks another"
            );
        }
        return Ok(());
    }
    let target = run_target(&args);
    let window = timeout_phrase(&args);
    let outcome: RunOutcome = session.typed("diag.run", args)?;
    println!(
        "launched {target} {window} (exit {:?}, timed out: {})",
        outcome.report.exit_code, outcome.report.timed_out
    );
    if let Err(error) = print_cleared(&outcome.cleared, &mut std::io::stdout()) {
        closed_output(error);
    }
    println!("captured logs in {}", outcome.profiles_dir.display());
    for l in &outcome.report.logs {
        println!("  log: {}", l.display());
    }
    print_errors("run", &outcome.errors);
    if let Some(hint) = &outcome.hint {
        println!("hint: {hint}");
    }
    let code = report_verdict(outcome.verdict.as_ref());
    if code != 0 {
        exit(code);
    }
    Ok(())
}

fn parse_window(s: &str) -> Result<WindowSize, String> {
    WindowSize::parse(s).ok_or_else(|| format!("expected WxH, got `{s}`"))
}

/// `PATH[:server]`: the mod dir, optionally loaded by the server only.
fn parse_test_mod(s: &str) -> Result<TestModArg, String> {
    let (path, role) = match s.strip_suffix(":server") {
        Some(path) => (path, TestModRole::ServerOnly),
        None => (s, TestModRole::Both),
    };
    let path = PathBuf::from(path);
    let link_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.strip_prefix('@').unwrap_or(n))
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("`{s}` has no mod name"))?
        .to_string();
    Ok(TestModArg {
        path,
        link_name,
        role,
    })
}

fn run_client(session: &Session, args: Value) -> Result<()> {
    if session.preview {
        return preview_run(session, "diag.run_client", args);
    }
    let window = timeout_phrase(&args);
    let outcome: ClientRunOutcome = session.typed("diag.run_client", args)?;
    println!(
        "launched client {window} (launcher exit {:?}, timed out: {})",
        outcome.report.exit_code, outcome.report.timed_out
    );
    if let Err(error) = print_cleared(&outcome.cleared, &mut std::io::stdout()) {
        closed_output(error);
    }
    if outcome.links_kept {
        println!(
            "left the mod links in {} while the game runs; the next run removes them",
            outcome.game_dir.display()
        );
    }
    println!("captured logs in {}", outcome.log_dir.display());
    for l in &outcome.report.logs {
        println!("  log: {}", l.display());
    }
    print_errors("run-client", &outcome.errors);
    let code = report_verdict(outcome.verdict.as_ref());
    if code != 0 {
        exit(code);
    }
    Ok(())
}

/// The id `mf run-diag` registers its run under, so the Ctrl+C handler can stop
/// it through the registry's `diag.stop` and share its stop signal.
const DIAG_RUN_ID: &str = "mf-run-diag";

/// Install the Ctrl+C / SIGTERM handler: it only raises the run's stop signal,
/// so a second interrupt while the run tears down changes nothing.
fn install_stop_handler(session: &Session, interrupted: Arc<AtomicBool>) -> Result<()> {
    let stop: Core = modfoundry_commands::registry::core_with(&session.registry);
    ctrlc::set_handler(move || {
        interrupted.store(true, Ordering::SeqCst);
        let _ = stop.dispatch(
            Actor::CliSession,
            "diag.stop",
            json!({ "run_id": DIAG_RUN_ID }),
        );
    })
    .context("installing the Ctrl+C handler")
}

/// Install the Ctrl+C handler of `mf publish`: the first press cancels the running upload; a second, or one with nothing to cancel, exits 130.
fn install_cancel_handler(session: &Session) -> Result<()> {
    let cancel: Core = modfoundry_commands::registry::core_with(&session.registry);
    let mut cancelling = false;
    ctrlc::set_handler(move || {
        use std::io::Write;
        let mut stderr = std::io::stderr();
        // A second Ctrl+C, or one with nothing to cancel, ends mf at once as Ctrl+C did before:
        // making or deleting an item cannot be stopped, so a stuck helper there needs it.
        if !cancelling {
            let answer = cancel.dispatch(Actor::CliSession, "publish.cancel", json!({}));
            if matches!(&answer, Ok(Outcome::Data(value)) if value["cancelled"] == true) {
                cancelling = true;
                let _ = writeln!(stderr, "cancelling the upload");
                return;
            }
        }
        let _ = writeln!(stderr, "interrupted");
        std::process::exit(130);
    })
    .context("installing the Ctrl+C handler")
}

/// The status line `mf run-diag` prints once the run ends: a run the caller
/// stopped is a clean end, not a failure.
fn diag_finish(stop_requested: bool, window: &str, outcome: &DiagOutcome) -> String {
    if outcome.ready_timed_out {
        return ready_timeout_message();
    }
    if stop_requested || outcome.server.stopped {
        return "stopped".to_owned();
    }
    format!(
        "launched diag run {window} (server exit {:?}, timed out: {})",
        outcome.server.exit_code, outcome.server.timed_out
    )
}

/// The arguments `mf run-diag` sends: only the flags given, so saved settings fill the rest.
fn run_diag_args(target: Option<PathBuf>, flags: RunDiagFlags) -> Value {
    let has_test_mods = !flags.test_mods.is_empty();
    let has_expect = !flags.expect_lines.is_empty();
    let has_fail = !flags.fail_lines.is_empty();
    let has_allow = !flags.allow_errors.is_empty();
    let has_target = target.is_some();
    let source_root = set_source_root(flags.source_root, flags.set.is_some());
    let mut args = drop_nulls(json!(RunDiagArgs {
        mod_dir: target.unwrap_or_default(),
        source_root: source_root.unwrap_or_default(),
        config: flags.config,
        set: flags.set,
        mod_prefix: flags.mod_prefix,
        game_dir: flags.game_dir,
        server_dir: flags.server,
        steam_dir: flags.steam_dir,
        profiles_dir: flags.profiles_dir,
        port: flags.port,
        client: !flags.no_client,
        connect_name: flags.connect_name,
        window: flags.window,
        windowed: !flags.fullscreen,
        timeout_secs: flags.timeout,
        run_id: Some(DIAG_RUN_ID.to_owned()),
        test_mods: flags.test_mods,
        expect_lines: flags.expect_lines,
        fail_lines: flags.fail_lines,
        allow_errors: flags.allow_errors,
    }));
    only_if_given(&mut args, "mod_dir", has_target);
    only_if_given(&mut args, "client", flags.client || flags.no_client);
    only_if_given(&mut args, "windowed", flags.windowed || flags.fullscreen);
    only_if_given(&mut args, "test_mods", has_test_mods);
    only_if_given(&mut args, "expect_lines", has_expect);
    only_if_given(&mut args, "fail_lines", has_fail);
    only_if_given(&mut args, "allow_errors", has_allow);
    args
}

/// Leaves `key` out unless the flag that sets it was given.
fn only_if_given(args: &mut Value, key: &str, given: bool) {
    if !given {
        if let Some(object) = args.as_object_mut() {
            object.remove(key);
        }
    }
}

/// The window `mf run-diag` reports: the timeout it sent, a saved one it filled in, or none.
fn diag_window(sent: &Value, settings_used: &Value) -> String {
    if let Some(secs) = sent["timeout_secs"].as_u64() {
        return format!("for up to {secs}s");
    }
    let filled = settings_used
        .as_array()
        .is_some_and(|keys| keys.iter().any(|key| key.as_str() == Some("timeout_secs")));
    if filled {
        "for the saved timeout".to_owned()
    } else {
        "until stopped".to_owned()
    }
}

/// Why a Diag run started no client: the app's saved launch target is Server only.
fn no_client_note(result: &Value) -> Option<&'static str> {
    (result["saved_server_only"] == Value::Bool(true)).then_some(
        "no client: the app's saved launch target is Server only; pass --client to start one",
    )
}

fn run_diag(session: &Session, args: Value) -> Result<()> {
    if session.preview {
        return preview_run(session, "diag.run_diag", args);
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    install_stop_handler(session, Arc::clone(&interrupted))?;
    let result = session.data("diag.run_diag", args.clone())?;
    let window = diag_window(&args, &result["settings_used"]);
    let note = no_client_note(&result);
    let outcome: DiagOutcome =
        serde_json::from_value(result).context("diag.run_diag: malformed result payload")?;
    let line = diag_finish(interrupted.load(Ordering::SeqCst), &window, &outcome);
    println!("{line}");
    if let Err(error) = print_cleared(&outcome.cleared, &mut std::io::stdout()) {
        closed_output(error);
    }
    println!("captured server logs in {}", outcome.profiles_dir.display());
    for l in &outcome.server.logs {
        println!("  log: {}", l.display());
    }
    if let Some(note) = note {
        println!("{note}");
    }
    if outcome.client_closed {
        println!("the client closed, so the server was stopped");
    }
    if let Some(c) = &outcome.client {
        println!("captured client logs in {}", c.log_dir.display());
        for l in &c.report.logs {
            println!("  log: {}", l.display());
        }
    }
    print_errors("run-diag", &outcome.errors);
    if let Some(hint) = &outcome.hint {
        println!("hint: {hint}");
    }
    let code = report_verdict(outcome.verdict.as_ref());
    if outcome.ready_timed_out || code != 0 {
        exit(1);
    }
    Ok(())
}

/// The source root `mf errors` sends: the one typed, else `.` when the folder is
/// a mod project of its own. An explicit catalogue names its own project.
fn errors_source_root(flags: &ErrorsFlags, folder: &Path) -> Option<PathBuf> {
    if flags.known_errors.is_some() {
        return flags.source_root.clone();
    }
    flags.source_root.clone().or_else(|| {
        folder
            .join("config.cpp")
            .is_file()
            .then(|| PathBuf::from("."))
    })
}

fn errors(session: &Session, dir: &Path, mut flags: ErrorsFlags) -> Result<()> {
    flags.source_root = errors_source_root(&flags, Path::new("."));
    if let Some(id) = flags.learn {
        return session.mutation(
            "diag.learn_error",
            json!({
                "dir": lossy_path(dir),
                "source_root": flags.source_root.as_deref().map(lossy_path),
                "known_errors": flags.known_errors.as_deref().map(lossy_path),
                "id": id,
                "meaning": flags.meaning.unwrap_or_default(),
                "hint": flags.hint.unwrap_or_default(),
            }),
        );
    }
    let errs: Vec<modfoundry_diag::MappedError> =
        session.typed("diag.errors", error_args(dir, &flags))?;
    let (lines, code) = errors_report(&errs, dir, flags.all);
    if flags.json {
        println!("{}", serde_json::to_string_pretty(&errs)?);
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    if code != 0 {
        exit(code);
    }
    Ok(())
}

/// The `mf errors` options that ask `diag.errors` what it found.
struct ErrorsFlags {
    source_root: Option<PathBuf>,
    baseline: Option<PathBuf>,
    known_errors: Option<PathBuf>,
    learn: Option<String>,
    meaning: Option<String>,
    hint: Option<String>,
    all: bool,
    json: bool,
}

fn error_args(dir: &Path, flags: &ErrorsFlags) -> Value {
    drop_nulls(json!({
        "dir": lossy_path(dir),
        "source_root": flags.source_root.as_deref().map(lossy_path),
        "baseline": flags.baseline.as_deref().map(lossy_path),
        "known_errors": flags.known_errors.as_deref().map(lossy_path),
    }))
}

/// A path as JSON text; `json!` on a `Path` panics when it is not UTF-8.
fn lossy_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Prints one line per path an earlier run left in the install and this one removed.
fn print_cleared(cleared: &[PathBuf], out: &mut impl std::io::Write) -> std::io::Result<()> {
    for path in cleared {
        writeln!(out, "{}", cleared_line(path))?;
    }
    Ok(())
}

/// The lines `mf` prints for a run's verdict.
fn verdict_lines(verdict: &modfoundry_diag::Verdict) -> Vec<String> {
    let outcome = if verdict.passed { "passed" } else { "failed" };
    let mut lines = vec![format!("verdict: {outcome}")];
    if verdict.passed {
        return lines;
    }
    for missing in &verdict.missing {
        lines.push(format!("  expected line not seen: {missing}"));
    }
    for line in &verdict.fail_lines {
        lines.push(format!("  fail line: {line}"));
    }
    for error in &verdict.mod_errors {
        lines.push("  script error in a mod of this run:".to_owned());
        lines.extend(error.lines().map(|line| format!("    {line}")));
    }
    for log in &verdict.crash_logs {
        lines.push(format!("  crash log: {}", log.display()));
    }
    let notes = verdict_notes(verdict);
    if !notes.is_empty() {
        lines.push(format!("  ({notes})"));
    }
    lines
}

/// The exit code a verdict asks for: 1 when the run's tests failed.
fn verdict_code(verdict: &modfoundry_diag::Verdict) -> i32 {
    i32::from(!verdict.passed)
}

/// The closing note naming the script errors that did not fail the verdict.
fn verdict_notes(verdict: &modfoundry_diag::Verdict) -> String {
    let mut notes = Vec::new();
    if verdict.other_errors > 0 {
        let plural = if verdict.other_errors == 1 { "" } else { "s" };
        notes.push(format!(
            "{} other script error{plural}",
            verdict.other_errors
        ));
    }
    if verdict.ignored > 0 {
        let plural = if verdict.ignored == 1 { "" } else { "s" };
        notes.push(format!(
            "{} known vanilla error{plural} ignored",
            verdict.ignored
        ));
    }
    notes.join(", ")
}

/// Prints a run's verdict after its summary and returns the exit code it asks for.
fn report_verdict(verdict: Option<&modfoundry_diag::Verdict>) -> i32 {
    let Some(verdict) = verdict else {
        return 0;
    };
    for line in verdict_lines(verdict) {
        println!("{line}");
    }
    verdict_code(verdict)
}

/// The lines `mf` prints for a recorded run.
fn status_lines(answer: &Value) -> Vec<String> {
    let run_id = answer["run_id"].as_str().unwrap_or_default();
    let state = answer["state"].as_str().unwrap_or_default();
    let mut lines = Vec::new();
    match state {
        "ended" => match answer["outcome"].as_str() {
            Some(outcome) => lines.push(format!("run {run_id}: ended ({outcome})")),
            None => lines.push(format!("run {run_id}: ended")),
        },
        "lost" => lines.push(format!("run {run_id}: lost")),
        _ => lines.push(format!("run {run_id}: running")),
    }
    if let Some(command) = answer["command"].as_str() {
        lines.push(format!("  command: {command}"));
    }
    let elapsed = answer["elapsed_secs"].as_i64().unwrap_or(0);
    match state {
        "ended" => lines.push(format!("  ran for {elapsed} s")),
        "lost" => lines.push(format!(
            "  its launcher stopped reporting after {elapsed} s"
        )),
        _ => lines.push(format!("  running for {elapsed} s")),
    }
    if state == "running" && answer["stop_requested"] == json!(true) {
        lines.push("  stop requested".to_owned());
    }
    if let Some(dirs) = answer["log_dirs"].as_array() {
        for dir in dirs.iter().filter_map(Value::as_str) {
            lines.push(format!("  log folder: {dir}"));
        }
    }
    lines
}

/// A result a recorded run left, as much of it as `mf status` renders.
#[derive(Deserialize)]
struct StoredResult {
    #[serde(default)]
    errors: Vec<modfoundry_diag::MappedError>,
    verdict: Option<modfoundry_diag::Verdict>,
    hint: Option<String>,
}

/// Prints how a recorded run stands, and what its result says when it ended.
fn status(session: &Session, run_id: &str) -> Result<()> {
    let answer = session.data("diag.status", json!({ "run_id": run_id }))?;
    for line in status_lines(&answer) {
        println!("{line}");
    }
    let stored = answer
        .get("result")
        .cloned()
        .map(serde_json::from_value::<StoredResult>)
        .transpose()
        .context("diag.status: malformed result payload")?;
    if let Some(StoredResult {
        errors,
        verdict,
        hint,
    }) = stored
    {
        print_errors("status", &errors);
        let _ = report_verdict(verdict.as_ref());
        if let Some(hint) = hint {
            println!("hint: {hint}");
        }
    }
    Ok(())
}

/// Shared renderer for `MappedError`s: every run verb and `mf errors`.
fn print_errors(label: &str, errs: &[modfoundry_diag::MappedError]) {
    for line in errors_lines(label, errs, false).0 {
        println!("{line}");
    }
}

fn count_line(label: &str, errs: &[modfoundry_diag::MappedError]) -> String {
    let mapped = errs.iter().filter(|e| e.file.is_some()).count();
    format!("{label}: {} issue(s) ({mapped} file-mapped)", errs.len())
}

/// One script error as a line: its severity, its location and its message.
fn error_line(e: &modfoundry_diag::MappedError) -> String {
    // Prefer the resolved host file; otherwise fall back to the engine path we
    // parsed out of the log (e.g. `p3diag/Scripts/3_Game/broken.c`) so
    // packed/workshop errors still show where they point.
    let path = e
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .or_else(|| e.logged_file.clone());
    let where_ = match (path, e.line) {
        (Some(p), Some(l)) => format!("{p}:{l}"),
        (Some(p), None) => p,
        (None, Some(l)) => format!("(line {l})"),
        (None, None) => "(no location)".to_string(),
    };
    format!("[{}] {}: {}", severity_word(e.severity), where_, e.message)
}

/// The `mf errors` report: its lines and the exit code it asks for. `all` prints
/// every error in full; otherwise new errors print in full and known ones once
/// with their hint.
fn errors_report(
    errors: &[modfoundry_diag::MappedError],
    dir: &Path,
    all: bool,
) -> (Vec<String>, i32) {
    let dir = dir.display();
    if errors.is_empty() {
        return (vec![format!("ok: no errors in {dir}")], 0);
    }
    let (mut lines, code) = errors_lines("errors", errors, all);
    if !all && code == 0 {
        lines.insert(1, format!("ok: no new errors in {dir}"));
    }
    (lines, code)
}

/// The lines a run verb prints for its errors, and the exit code its new errors
/// ask for: the count line, each new error in full with its id, each known entry
/// once with its hint, and the closing `hidden:` line.
fn errors_lines(
    label: &str,
    errors: &[modfoundry_diag::MappedError],
    all: bool,
) -> (Vec<String>, i32) {
    let is_new = |e: &&modfoundry_diag::MappedError| !e.baseline && e.known.is_none();
    let new_count = errors.iter().filter(is_new).count();

    let mut lines = vec![count_line(label, errors)];
    if all {
        for e in errors {
            let mark = match (&e.baseline, &e.known) {
                (true, _) => "  [baseline]".to_owned(),
                (false, Some(known)) => format!("  [known {}]", known.id),
                (false, None) => String::new(),
            };
            lines.push(format!("  {}{mark}", error_line(e)));
        }
    } else {
        for e in errors.iter().filter(is_new) {
            lines.push(format!("  {}  (id {})", error_line(e), e.id));
        }
        for known in known_blocks(errors) {
            lines.push(known.0);
            if !known.1.is_empty() {
                lines.push(format!("  hint: {}", known.1));
            }
        }
    }

    let hidden_baseline = errors.iter().filter(|e| e.baseline).count();
    let hidden_known = errors
        .iter()
        .filter(|e| !e.baseline && e.known.is_some())
        .count();
    if hidden_baseline > 0 || hidden_known > 0 {
        lines.push(format!(
            "hidden: {hidden_baseline} as baseline, {hidden_known} as known"
        ));
    }
    (lines, i32::from(new_count > 0))
}

/// One line per known entry in first-seen order, with that entry's error count
/// and its hint.
fn known_blocks(errors: &[modfoundry_diag::MappedError]) -> Vec<(String, String)> {
    let mut seen: Vec<&str> = Vec::new();
    let mut blocks = Vec::new();
    let shown = || errors.iter().filter(|e| !e.baseline);
    for known in shown().filter_map(|e| e.known.as_ref()) {
        if seen.contains(&known.id.as_str()) {
            continue;
        }
        seen.push(&known.id);
        let count = shown()
            .filter(|other| other.known.as_ref().is_some_and(|k| k.id == known.id))
            .count();
        let line = format!("known {} ({count}x): {}", known.id, known.meaning);
        blocks.push((line, known.hint.clone()));
    }
    blocks
}

fn keygen(
    session: &Session,
    authority: &str,
    dir: Option<PathBuf>,
    tools: Option<PathBuf>,
) -> Result<()> {
    session.mutation(
        "sign.keygen",
        json!(KeygenArgs {
            authority: authority.to_owned(),
            dir: dir.unwrap_or_else(|| PathBuf::from("./keys")),
            tools,
        }),
    )
}

fn formats(names: &[String]) {
    println!("recognized formats:");
    for name in names {
        println!("  - {name}");
    }
}

fn changelog(session: &Session, since: Option<String>) -> Result<()> {
    let args = match &since {
        Some(since) => json!({ "since": since }),
        None => json!({}),
    };
    let answer = session.data("host.changelog", args)?;
    let text = answer["text"].as_str().unwrap_or_default();
    if !text.is_empty() {
        print!("{text}");
    } else if let Some(since) = since {
        println!("no changes after {since}");
    }
    Ok(())
}

fn update(session: &Session, check: bool) -> Result<()> {
    if check {
        return update_check(session);
    }
    let answer = session.data("host.update", json!({}))?;
    if answer["updated"] == json!(true) {
        println!(
            "updated ModFoundry {} to {}; restart your MCP clients to use it",
            answer["from"].as_str().unwrap_or_default(),
            answer["to"].as_str().unwrap_or_default()
        );
    } else {
        print_newest(&answer);
    }
    Ok(())
}

fn update_check(session: &Session) -> Result<()> {
    let answer = session.data("host.update_check", json!({}))?;
    if answer["checked"] != json!(true) {
        println!("{}", answer["reason"].as_str().unwrap_or_default());
    } else if let Some(notice) = answer["notice"].as_str() {
        println!("{notice}");
    } else {
        print_newest(&answer);
    }
    Ok(())
}

fn print_newest(answer: &Value) {
    println!("{}", newest_line(answer));
}

fn newest_line(answer: &Value) -> String {
    let current = answer["current"].as_str().unwrap_or_default();
    if answer["latest"].is_null() {
        format!("no ModFoundry release is published yet (this is {current})")
    } else {
        format!("ModFoundry {current} is the newest release")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mf_line_in_the_agent_skill_parses() {
        let skill = include_str!("../../../packaging/skill/modfoundry/SKILL.md");
        let lines: Vec<&str> = skill
            .lines()
            .map(|line| line.split(" #").next().unwrap().trim())
            .filter(|line| line.starts_with("mf "))
            .collect();
        assert!(lines.len() >= 5, "{lines:?}");
        let keygen = lines.iter().position(|line| line.starts_with("mf keygen "));
        let build = lines.iter().position(|line| line.starts_with("mf build "));
        assert!(keygen.is_some() && keygen < build, "{lines:?}");
        for line in lines {
            let mut words = Vec::new();
            let mut rest = line;
            while let Some(start) = rest.find('<') {
                words.extend(rest[..start].split_whitespace());
                words.push("X");
                rest = &rest[rest[start..].find('>').unwrap() + start + 1..];
            }
            words.extend(rest.split_whitespace());
            assert!(Cli::try_parse_from(&words).is_ok(), "{line}");
        }
    }

    #[test]
    fn the_default_render_file_is_named_after_the_layout_without_clobbering_art() {
        assert_eq!(
            default_render_out(Path::new("mod/gui/inventory.layout")),
            PathBuf::from("inventory.render.png")
        );
    }

    #[test]
    fn layout_render_reads_its_size_background_and_out_flags() {
        let cli = Cli::try_parse_from([
            "mf",
            "layout",
            "render",
            "gui/panel.layout",
            "--out",
            "panel.png",
            "--size",
            "200x100",
            "--background",
            "#102030",
            "--show-hidden",
            "--json",
        ])
        .unwrap();
        let Commands::Layout {
            command:
                LayoutCommands::Render {
                    out,
                    size,
                    background,
                    show_hidden,
                    json,
                    ..
                },
        } = cli.command
        else {
            panic!("layout render must parse");
        };
        assert_eq!(out, Some(PathBuf::from("panel.png")));
        assert_eq!(size.as_deref(), Some("200x100"));
        assert_eq!(background.as_deref(), Some("#102030"));
        assert!(show_hidden && json);
    }

    #[test]
    fn the_newest_line_says_when_no_release_is_published() {
        assert_eq!(
            newest_line(&json!({"current": "2.2.0", "latest": "2.2.0"})),
            "ModFoundry 2.2.0 is the newest release"
        );
        assert_eq!(
            newest_line(&json!({"current": "2.2.0", "latest": null})),
            "no ModFoundry release is published yet (this is 2.2.0)"
        );
    }

    #[test]
    fn test_mod_flag_parses_a_path_and_the_server_only_suffix() {
        let both = parse_test_mod("/mods/AdminTools").unwrap();
        assert_eq!(both.path, PathBuf::from("/mods/AdminTools"));
        assert_eq!(both.link_name, "AdminTools");
        assert_eq!(both.role, TestModRole::Both);

        let server = parse_test_mod("/mods/AdminTools:server").unwrap();
        assert_eq!(server.path, PathBuf::from("/mods/AdminTools"));
        assert_eq!(server.link_name, "AdminTools");
        assert_eq!(server.role, TestModRole::ServerOnly);

        let at = parse_test_mod("/mods/@Foo").unwrap();
        assert_eq!(at.path, PathBuf::from("/mods/@Foo"));
        assert_eq!(at.link_name, "Foo");
        assert_eq!(at.role, TestModRole::Both);

        let at_server = parse_test_mod("/mods/@Foo:server").unwrap();
        assert_eq!(at_server.path, PathBuf::from("/mods/@Foo"));
        assert_eq!(at_server.link_name, "Foo");
        assert_eq!(at_server.role, TestModRole::ServerOnly);
    }

    #[test]
    fn test_mod_flag_rejects_a_path_without_a_mod_name() {
        assert!(parse_test_mod("/").is_err());
        assert!(parse_test_mod("/mods/@").is_err());
    }

    #[test]
    fn expect_line_help_says_the_run_stops_once_every_line_has_appeared() {
        use clap::CommandFactory;
        for verb in ["run", "run-client", "run-diag"] {
            let mut cli = Cli::command();
            let help = cli
                .find_subcommand_mut(verb)
                .unwrap()
                .render_long_help()
                .to_string();
            assert!(
                help.contains(
                    "the run stops 5 seconds after every expected line has appeared in an RPT log"
                ),
                "{verb}: {help}"
            );
        }
    }

    #[test]
    fn run_client_help_names_the_runs_own_mod_and_the_test_mod_order() {
        use clap::CommandFactory;
        let help = |verb: &str| {
            let mut cli = Cli::command();
            let help = cli
                .find_subcommand_mut(verb)
                .unwrap()
                .render_long_help()
                .to_string();
            help.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let client = help("run-client");
        assert!(
            client.contains("The last one is the run's own mod"),
            "{client}"
        );
        assert!(client.contains("before the --mod mods"), "{client}");
        for verb in ["run", "run-diag"] {
            assert!(
                !help(verb).contains("The last one is the run's own mod"),
                "{verb}"
            );
        }
    }

    #[test]
    fn plain_drops_control_characters_but_keeps_tabs() {
        assert_eq!(plain("\x1b[31mred\x07\tok\r"), "[31mred\tok");
    }

    #[test]
    fn gpu_lines_show_folder_and_device() {
        let cases: [(GpuReport, &[&str]); 3] = [
            (
                GpuReport {
                    helper: Some(PathBuf::from("/bin/mf-embed-gpu")),
                    driver: true,
                    libraries: Some(PathBuf::from("/data/cuda/lib")),
                    missing: Vec::new(),
                    device_ok: Some(true),
                    device_error: None,
                },
                &[
                    "gpu helper: /bin/mf-embed-gpu",
                    "cuda driver (libcuda.so.1): found",
                    "cuda libraries folder: /data/cuda/lib",
                    "cuda libraries: all found",
                    "gpu device: ready",
                ],
            ),
            (
                GpuReport {
                    helper: Some(PathBuf::from("/bin/mf-embed-gpu")),
                    driver: false,
                    libraries: None,
                    missing: vec!["libcublasLt.so.13".to_owned()],
                    device_ok: Some(false),
                    device_error: Some("error: no CUDA device".to_owned()),
                },
                &[
                    "gpu helper: /bin/mf-embed-gpu",
                    "cuda driver (libcuda.so.1): NOT FOUND",
                    "cuda libraries folder: unknown (no data folder)",
                    "cuda libraries: missing libcublasLt.so.13 (run tools/setup-cuda.sh)",
                    "gpu device: FAILED: error: no CUDA device (search embeds on the CPU)",
                ],
            ),
            (
                GpuReport::default(),
                &[
                    "gpu helper: NOT FOUND (search embeds on the CPU)",
                    "cuda driver (libcuda.so.1): NOT FOUND",
                    "cuda libraries folder: unknown (no data folder)",
                    "cuda libraries: all found",
                    "gpu device: not checked (no GPU helper)",
                ],
            ),
        ];
        for (report, expected) in cases {
            let lines = gpu_lines(&report);
            let expected: Vec<String> = expected.iter().map(|line| (*line).to_owned()).collect();
            assert_eq!(lines, expected);
        }
    }

    fn diag_outcome(exit_code: Option<i32>, timed_out: bool, stopped: bool) -> DiagOutcome {
        DiagOutcome {
            profiles_dir: PathBuf::from("/profiles"),
            server: modfoundry_diag::RunReport {
                exit_code,
                logs: Vec::new(),
                timed_out,
                stopped,
            },
            client: None,
            errors: Vec::new(),
            ready_timed_out: false,
            hint: None,
            cleared: Vec::new(),
            client_closed: false,
            verdict: None,
        }
    }

    #[test]
    fn a_launch_prints_every_path_an_earlier_run_left() {
        let mut out = Vec::new();
        print_cleared(
            &[
                PathBuf::from("/game/keys/Stale.bikey"),
                PathBuf::from("/game/@Old"),
            ],
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "removed what an earlier run left: /game/keys/Stale.bikey\n\
             removed what an earlier run left: /game/@Old\n"
        );
    }

    #[test]
    fn diag_finish_prints_stopped_or_the_launch_line() {
        let cases = [
            (true, None, false, true, "until stopped", "stopped"),
            (false, None, false, true, "until stopped", "stopped"),
            (
                false,
                Some(0),
                true,
                false,
                "for up to 5s",
                "launched diag run for up to 5s (server exit Some(0), timed out: true)",
            ),
            (
                false,
                Some(1),
                false,
                false,
                "until stopped",
                "launched diag run until stopped (server exit Some(1), timed out: false)",
            ),
        ];
        for (stop_requested, exit_code, timed_out, stopped, window, expected) in cases {
            let outcome = diag_outcome(exit_code, timed_out, stopped);
            assert_eq!(diag_finish(stop_requested, window, &outcome), expected);
        }
    }

    #[test]
    fn diag_finish_names_a_server_that_never_reported_ready() {
        let mut outcome = diag_outcome(None, false, true);
        outcome.ready_timed_out = true;

        assert_eq!(
            diag_finish(false, "until stopped", &outcome),
            "the server never reported ready within 90 s and was stopped"
        );
    }

    #[test]
    fn the_worst_severity_sets_the_exit_code() {
        let cases = [
            (vec![], 0),
            (vec![Severity::Hint, Severity::Warning], 0),
            (vec![Severity::Warning, Severity::Error], 1),
            (vec![Severity::Error, Severity::Fatal], 2),
            (vec![Severity::Fatal, Severity::Warning], 2),
        ];
        for (severities, want) in cases {
            let code = exit_code(severities.iter().copied());
            assert_eq!(code, want, "{severities:?}");
        }
    }

    #[test]
    fn a_run_preview_prints_the_launch_lines_removals_and_staging() {
        let plan = json!({
            "launches": [
                {
                    "role": "server",
                    "program": "/srv/DayZ Server/DayZServer",
                    "args": [
                        "-config=serverDZ.cfg",
                        "-mod=@FieldKit",
                        "-port=2302",
                        "-name=My Server",
                    ],
                    "cwd": "/srv/DayZ Server",
                    "env": [],
                    "env_remove": [],
                },
                {
                    "role": "client",
                    "program": "/usr/bin/proton",
                    "args": ["run", "/game/DayZ_x64.exe", "-window"],
                    "cwd": null,
                    "env": [["STEAM_COMPAT_DATA_PATH", "/mnt/Steam Library/prefix"]],
                    "env_remove": ["LD_PRELOAD"],
                },
            ],
            "steps": [
                {"op": "link", "link": "/game/@FieldKit", "target": "/src/builds/default/@FieldKit"},
                {"op": "copy", "from": "/src/keys/Field.bikey", "to": "/game/keys/Field.bikey"},
                {"op": "write", "path": "/game/mf-diag.cfg", "bytes": 1234},
                {"op": "create_dir", "path": "/game/mf-diag-profiles"},
                {"op": "relink", "link": "/game/@Old", "target": "/src/@Old"},
                {"op": "remove", "path": "/game/stale.txt"},
            ],
            "project": [{"op": "create_dir", "path": "/src/project/server/profiles"}],
            "leftovers": ["/game/@Gone", "/game/@Old", "/game/keys/Gone.bikey", "/game/stale.txt"],
        });

        let expected = [
            "would launch server: '/srv/DayZ Server/DayZServer' -config=serverDZ.cfg \
             -mod=@FieldKit -port=2302 '-name=My Server'",
            "  in /srv/DayZ Server",
            "would launch client: /usr/bin/proton run /game/DayZ_x64.exe -window",
            "  env STEAM_COMPAT_DATA_PATH='/mnt/Steam Library/prefix'",
            "  unset LD_PRELOAD",
            "would remove 2 leftover(s) from a dead run:",
            "  /game/@Gone",
            "  /game/keys/Gone.bikey",
            "would create in the project:",
            "    mkdir /src/project/server/profiles",
            "would stage 6 item(s):",
            "     link /game/@FieldKit -> /src/builds/default/@FieldKit",
            "     copy /src/keys/Field.bikey -> /game/keys/Field.bikey",
            "    write /game/mf-diag.cfg (1234 bytes)",
            "    mkdir /game/mf-diag-profiles",
            "   relink /game/@Old -> /src/@Old",
            "   remove /game/stale.txt",
        ];
        assert_eq!(run_plan_lines(&plan), expected);
    }

    #[test]
    fn drop_nulls_leaves_out_only_null_members() {
        let cases = [
            (Value::Null, false),
            (json!(false), true),
            (json!(0), true),
            (json!(""), true),
            (json!([]), true),
        ];
        for (value, kept) in cases {
            let args = drop_nulls(json!({ "member": value }));
            assert_eq!(args.get("member").is_some(), kept, "kept: {kept}");
        }
    }

    #[test]
    fn run_diag_sends_only_the_flags_the_user_passed() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["mf", "run-diag", "builds/dev/@X", "--source-root", "src"];
            argv.extend_from_slice(extra);
            let cli = Cli::try_parse_from(argv).unwrap();
            let Commands::RunDiag { target, flags } = cli.command else {
                panic!("run-diag must parse");
            };
            run_diag_args(target, flags)
        };

        let args = parse(&[]);
        for key in [
            "client",
            "windowed",
            "port",
            "test_mods",
            "timeout_secs",
            "expect_lines",
            "fail_lines",
            "allow_errors",
        ] {
            assert!(args.get(key).is_none(), "{key} is sent without a flag");
        }

        let args = parse(&["--no-client", "--fullscreen"]);
        assert_eq!(args["client"], json!(false));
        assert_eq!(args["windowed"], json!(false));

        let args = parse(&[
            "--expect-line",
            "DONE",
            "--fail-line",
            "FAIL",
            "--allow-error",
            "boom",
        ]);
        assert_eq!(args["expect_lines"], json!(["DONE"]));
        assert_eq!(args["fail_lines"], json!(["FAIL"]));
        assert_eq!(args["allow_errors"], json!(["boom"]));
    }

    #[test]
    fn the_verdict_flags_repeat_and_are_sent_only_when_given() {
        let run = run_args_of(&[
            "--expect-line",
            "DONE",
            "--expect-line",
            "re:pass=[1-9]",
            "--allow-error",
            "boom",
        ]);
        assert_eq!(run["expect_lines"], json!(["DONE", "re:pass=[1-9]"]));
        assert_eq!(run["allow_errors"], json!(["boom"]));
        assert!(run.get("fail_lines").is_none(), "{run}");

        let client = run_client_args_of(&[]);
        for key in ["expect_lines", "fail_lines", "allow_errors"] {
            assert!(client.get(key).is_none(), "{key} is sent without a flag");
        }
    }

    fn verdict(overrides: impl FnOnce(&mut modfoundry_diag::Verdict)) -> modfoundry_diag::Verdict {
        let mut verdict = modfoundry_diag::Verdict {
            passed: true,
            missing: Vec::new(),
            fail_lines: Vec::new(),
            mod_errors: Vec::new(),
            other_errors: 0,
            ignored: 0,
            crash_logs: Vec::new(),
        };
        overrides(&mut verdict);
        verdict
    }

    #[test]
    fn status_lines_say_how_a_run_stands() {
        let cases = [
            (
                json!({
                    "run_id": "r1", "state": "running", "command": "diag.run_diag",
                    "started_millis": 100_000, "elapsed_secs": 12,
                    "source_root": null, "log_dirs": ["/w/logs", "/w/more"],
                    "stop_requested": false, "outcome": null,
                }),
                vec![
                    "run r1: running",
                    "  command: diag.run_diag",
                    "  running for 12 s",
                    "  log folder: /w/logs",
                    "  log folder: /w/more",
                ],
            ),
            (
                json!({
                    "run_id": "r1", "state": "running", "command": "diag.run",
                    "started_millis": 100_000, "elapsed_secs": 3,
                    "source_root": null, "log_dirs": [],
                    "stop_requested": true, "outcome": null,
                }),
                vec![
                    "run r1: running",
                    "  command: diag.run",
                    "  running for 3 s",
                    "  stop requested",
                ],
            ),
            (
                json!({
                    "run_id": "r1", "state": "ended", "command": "diag.run",
                    "started_millis": 100_000, "elapsed_secs": 30,
                    "source_root": "/w/mod", "log_dirs": ["/w/logs"],
                    "stop_requested": false, "outcome": "exited",
                }),
                vec![
                    "run r1: ended (exited)",
                    "  command: diag.run",
                    "  ran for 30 s",
                    "  log folder: /w/logs",
                ],
            ),
            (
                json!({
                    "run_id": "r1", "state": "ended", "command": "diag.run",
                    "started_millis": 100_000, "elapsed_secs": 2,
                    "source_root": null, "log_dirs": [],
                    "stop_requested": false, "outcome": "failed: boom",
                }),
                vec![
                    "run r1: ended (failed: boom)",
                    "  command: diag.run",
                    "  ran for 2 s",
                ],
            ),
            (
                json!({
                    "run_id": "r1", "state": "lost", "command": "diag.run",
                    "started_millis": 100_000, "elapsed_secs": 45,
                    "source_root": null, "log_dirs": ["/w/logs"],
                    "stop_requested": true, "outcome": null,
                }),
                vec![
                    "run r1: lost",
                    "  command: diag.run",
                    "  its launcher stopped reporting after 45 s",
                    "  log folder: /w/logs",
                ],
            ),
            (
                json!({
                    "run_id": "r1", "state": "ended", "command": "diag.run",
                    "started_millis": 100_000, "elapsed_secs": 0,
                    "source_root": null, "log_dirs": [],
                    "stop_requested": false, "outcome": null,
                }),
                vec!["run r1: ended", "  command: diag.run", "  ran for 0 s"],
            ),
        ];
        for (answer, lines) in cases {
            assert_eq!(status_lines(&answer), lines, "{answer}");
        }
    }

    #[test]
    fn a_failed_verdict_prints_every_reason_and_asks_for_exit_one() {
        let failed = verdict(|verdict| {
            verdict.passed = false;
            verdict.missing = vec!["[SampleModTests] DONE".into()];
            verdict.fail_lines = vec!["[SampleModTests] FAIL SampleModCfg".into()];
            verdict.mod_errors = vec!["boom\n-- Stack trace --".into()];
            verdict.other_errors = 3;
            verdict.ignored = 1;
            verdict.crash_logs = vec![PathBuf::from("/profiles/crash_1.log")];
        });
        assert_eq!(
            verdict_lines(&failed),
            [
                "verdict: failed",
                "  expected line not seen: [SampleModTests] DONE",
                "  fail line: [SampleModTests] FAIL SampleModCfg",
                "  script error in a mod of this run:",
                "    boom",
                "    -- Stack trace --",
                "  crash log: /profiles/crash_1.log",
                "  (3 other script errors, 1 known vanilla error ignored)",
            ]
        );
        assert_eq!(verdict_code(&failed), 1);

        let passed = verdict(|_| {});
        assert_eq!(verdict_lines(&passed), ["verdict: passed"]);
        assert_eq!(verdict_code(&passed), 0);
    }

    #[test]
    fn run_diag_client_and_windowed_flags_send_true() {
        let cli = Cli::try_parse_from([
            "mf",
            "run-diag",
            "builds/dev/@X",
            "--source-root",
            "src",
            "--client",
            "--windowed",
        ])
        .unwrap();
        let Commands::RunDiag { target, flags } = cli.command else {
            panic!("run-diag must parse");
        };
        let args = run_diag_args(target, flags);
        assert_eq!(args["client"], json!(true));
        assert_eq!(args["windowed"], json!(true));
    }

    fn run_args_of(extra: &[&str]) -> Value {
        let mut argv = vec!["mf", "run", "builds/dev/@X"];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Commands::Run { target, flags } = cli.command else {
            panic!("run must parse");
        };
        run_args(target, flags)
    }

    fn run_client_args_of(extra: &[&str]) -> Value {
        let mut argv = vec!["mf", "run-client", "--mod", "builds/dev/@X"];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Commands::RunClient { flags } = cli.command else {
            panic!("run-client must parse");
        };
        run_client_args(flags)
    }

    fn run_diag_args_of(extra: &[&str]) -> Value {
        let mut argv = vec!["mf", "run-diag", "builds/dev/@X", "--source-root", "src"];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Commands::RunDiag { target, flags } = cli.command else {
            panic!("run-diag must parse");
        };
        run_diag_args(target, flags)
    }

    #[test]
    fn launches_send_their_set_and_config_only_when_given() {
        assert!(run_args_of(&[]).get("set").is_none());
        assert_eq!(run_args_of(&["--set", "session"])["set"], json!("session"));
        assert!(run_client_args_of(&[]).get("set").is_none());
        assert_eq!(
            run_client_args_of(&["--set", "session"])["set"],
            json!("session")
        );
        assert!(run_diag_args_of(&[]).get("set").is_none());
        assert!(run_diag_args_of(&[]).get("config").is_none());
        assert_eq!(
            run_diag_args_of(&["--set", "session"])["set"],
            json!("session")
        );
        assert_eq!(
            run_diag_args_of(&["--config", "serverDZ_expansion.cfg"])["config"],
            json!("serverDZ_expansion.cfg")
        );
    }

    #[test]
    fn a_run_names_its_set_when_it_has_no_target() {
        let cli = Cli::try_parse_from(["mf", "run", "--set", "session"]).unwrap();
        let Commands::Run { target, flags } = cli.command else {
            panic!("run must parse");
        };
        assert_eq!(run_target(&run_args(target, flags)), "set session");

        assert_eq!(run_target(&run_args_of(&[])), "builds/dev/@X");
        assert_eq!(
            run_target(&run_args_of(&["--set", "session"])),
            "builds/dev/@X"
        );
        assert_eq!(run_target(&json!({})), "");
    }

    #[test]
    fn run_client_takes_no_mod_list_when_a_set_names_it() {
        assert!(Cli::try_parse_from(["mf", "run-client"]).is_err());
        assert!(Cli::try_parse_from(["mf", "run-client", "--set", "session"]).is_ok());
    }

    #[test]
    fn launches_send_their_timeout_only_when_given() {
        assert!(run_args_of(&[]).get("timeout_secs").is_none());
        assert!(run_args_of(&[]).get("test_mods").is_none());
        assert_eq!(run_args_of(&["--timeout", "5"])["timeout_secs"], json!(5));

        assert!(run_client_args_of(&[]).get("timeout_secs").is_none());
        assert!(run_client_args_of(&[]).get("test_mods").is_none());
        assert_eq!(
            run_client_args_of(&["--timeout", "5"])["timeout_secs"],
            json!(5)
        );
    }

    #[test]
    fn run_and_run_client_send_the_given_test_mods_in_order() {
        let one = json!({ "path": "A/@One", "link_name": "One", "role": "both" });
        let two = json!({ "path": "B/@Two", "link_name": "Two", "role": "server_only" });

        let run = run_args_of(&["--test-mod", "A/@One", "--test-mod", "B/@Two:server"]);
        assert_eq!(run["test_mods"], json!([one.clone(), two]));

        let client = run_client_args_of(&["--test-mod", "A/@One"]);
        assert_eq!(client["test_mods"], json!([one]));
    }

    #[test]
    fn run_client_sends_the_window_choice_only_when_given() {
        assert!(run_client_args_of(&[]).get("windowed").is_none());
        assert_eq!(
            run_client_args_of(&["--fullscreen"])["windowed"],
            json!(false)
        );
        assert_eq!(run_client_args_of(&["--windowed"])["windowed"], json!(true));
        for command in [
            vec!["mf", "run-client", "--mod", "@X"],
            vec!["mf", "run-diag", "@X"],
        ] {
            let argv = [command, vec!["--fullscreen", "--window", "800x600"]].concat();
            assert!(Cli::try_parse_from(argv).is_err());
        }

        let full: RunClientCommandArgs =
            serde_json::from_value(run_client_args_of(&["--fullscreen"])).unwrap();
        assert!(
            !full.client.windowed,
            "the full-screen flag reaches the field"
        );
        let bare: RunClientCommandArgs = serde_json::from_value(run_client_args_of(&[])).unwrap();
        assert!(bare.client.windowed, "no flag keeps the window");
    }

    #[test]
    fn settings_line_names_the_saved_settings_a_call_used() {
        let cases = [
            (
                json!({ "settings_used": ["authority", "keys_dir"] }),
                Some("using saved settings: authority, keys_dir"),
            ),
            (json!({ "settings_used": [] }), None),
            (json!({}), None),
        ];
        for (result, want) in cases {
            assert_eq!(settings_line(&result).as_deref(), want, "{result}");
        }
    }

    #[test]
    fn a_run_plan_carries_the_saved_settings_its_line_names() {
        let plan = json!({
            "launches": [],
            "steps": [],
            "project": [],
            "leftovers": [],
            "settings_used": ["port"],
        });
        assert_eq!(
            settings_line(&plan).as_deref(),
            Some("using saved settings: port")
        );
    }

    fn mapped(message: &str, logged_file: Option<&str>) -> modfoundry_diag::MappedError {
        modfoundry_diag::MappedError::new(
            None,
            logged_file.map(str::to_owned),
            Some(7),
            Severity::Error,
            message,
        )
    }

    fn known(
        mut error: modfoundry_diag::MappedError,
        id: &str,
        hint: &str,
    ) -> modfoundry_diag::MappedError {
        error.known = Some(modfoundry_diag::KnownRef {
            id: id.to_owned(),
            meaning: "framework noise".to_owned(),
            hint: hint.to_owned(),
        });
        error
    }

    #[test]
    fn the_errors_report_shows_new_errors_once_and_hides_the_known_ones_with_a_count() {
        let errors = vec![
            mapped("Can't find variable 'x'", Some("Mod/scripts/a.c")),
            known(
                mapped("JSON ERROR JM/CF/a.c:46", None),
                "cf000000",
                "ignore it",
            ),
            {
                let mut baseline = mapped("JSON ERROR JM/CF/b.c:47", None);
                baseline.baseline = true;
                baseline
            },
            known(
                mapped("JSON ERROR JM/CF/c.c:48", None),
                "cf000000",
                "ignore it",
            ),
        ];
        let (lines, code) = errors_report(&errors, Path::new("/profiles"), false);
        assert_eq!(code, 1, "a new error asks for exit 1");
        let text = lines.join("\n");
        assert!(
            text.contains("Can't find variable 'x'") && text.contains("Mod/scripts/a.c:7"),
            "{text}"
        );
        assert!(
            !text.contains("JM/CF/a.c"),
            "a known error is not printed in full: {text}"
        );
        assert!(
            !text.contains("JM/CF/b.c"),
            "a baseline error is not printed: {text}"
        );
        assert_eq!(
            text.matches("known cf000000 (2x): framework noise").count(),
            1,
            "{text}"
        );
        assert!(text.contains("  hint: ignore it"), "{text}");
        assert!(
            text.trim_end()
                .ends_with("hidden: 1 as baseline, 2 as known"),
            "{text}"
        );
    }

    #[test]
    fn the_errors_report_says_so_when_every_error_was_hidden() {
        let mut baseline = mapped("JSON ERROR JM/CF/a.c:46", None);
        baseline.baseline = true;
        let (lines, code) = errors_report(&[baseline], Path::new("/profiles"), false);
        assert_eq!(code, 0, "only hidden errors ask for exit 0");
        assert_eq!(
            lines,
            [
                "errors: 1 issue(s) (0 file-mapped)".to_owned(),
                "ok: no new errors in /profiles".to_owned(),
                "hidden: 1 as baseline, 0 as known".to_owned(),
            ]
        );
    }

    #[test]
    fn an_error_both_baseline_and_known_is_hidden_once_as_baseline() {
        let mut both = known(mapped("JSON ERROR JM/CF/a.c:46", None), "cf000000", "");
        both.baseline = true;
        let (lines, _) = errors_report(&[both], Path::new("/profiles"), false);
        assert_eq!(
            lines,
            [
                "errors: 1 issue(s) (0 file-mapped)".to_owned(),
                "ok: no new errors in /profiles".to_owned(),
                "hidden: 1 as baseline, 0 as known".to_owned(),
            ]
        );
    }

    #[test]
    fn known_errors_print_with_their_hint_when_nothing_new_is_left() {
        let errors = vec![known(
            mapped("JSON ERROR JM/CF/a.c:46", None),
            "cf000000",
            "ignore it",
        )];
        let (lines, code) = errors_report(&errors, Path::new("/profiles"), false);
        assert_eq!(code, 0);
        assert_eq!(
            lines,
            [
                "errors: 1 issue(s) (0 file-mapped)".to_owned(),
                "ok: no new errors in /profiles".to_owned(),
                "known cf000000 (1x): framework noise".to_owned(),
                "  hint: ignore it".to_owned(),
                "hidden: 0 as baseline, 1 as known".to_owned(),
            ]
        );
    }

    #[test]
    fn the_errors_report_opens_with_the_count_of_every_error_found() {
        let mut baseline = mapped("JSON ERROR JM/CF/a.c:46", None);
        baseline.baseline = true;
        let errors = vec![
            mapped("Can't find variable 'x'", Some("Mod/scripts/a.c")),
            baseline,
        ];
        for all in [false, true] {
            let (lines, _) = errors_report(&errors, Path::new("/profiles"), all);
            assert_eq!(lines[0], "errors: 2 issue(s) (0 file-mapped)", "all={all}");
        }
    }

    #[test]
    fn the_errors_report_names_an_empty_folder() {
        let (lines, code) = errors_report(&[], Path::new("/profiles"), false);
        assert_eq!(lines, ["ok: no errors in /profiles".to_owned()]);
        assert_eq!(code, 0);
    }

    #[test]
    fn all_prints_every_error_marked_and_keeps_the_exit_code_by_new_errors() {
        let mut baseline = mapped("JSON ERROR JM/CF/a.c:46", None);
        baseline.baseline = true;
        let errors = vec![
            baseline,
            known(mapped("JSON ERROR JM/CF/b.c:47", None), "cf000000", ""),
        ];
        let (lines, code) = errors_report(&errors, Path::new("/profiles"), true);
        let text = lines.join("\n");
        assert!(text.contains("[baseline]"), "{text}");
        assert!(text.contains("[known cf000000]"), "{text}");
        assert!(text.contains("hidden: 1 as baseline, 1 as known"), "{text}");
        assert_eq!(code, 0);
    }

    #[test]
    fn a_run_prints_its_errors_as_the_errors_report_does() {
        let errors = vec![
            mapped("Can't find variable 'x'", Some("Mod/scripts/a.c")),
            known(
                mapped("JSON ERROR JM/CF/a.c:46", None),
                "cf000000",
                "ignore it",
            ),
            known(
                mapped("JSON ERROR JM/CF/c.c:48", None),
                "cf000000",
                "ignore it",
            ),
        ];
        let (lines, _) = errors_lines("run", &errors, false);
        let text = lines.join("\n");
        assert_eq!(lines[0], "run: 3 issue(s) (0 file-mapped)", "{text}");
        assert!(
            text.contains("Can't find variable 'x'") && text.contains("(id "),
            "a new error prints in full with its id: {text}"
        );
        assert!(
            !text.contains("JM/CF/a.c"),
            "a known error is not printed in full: {text}"
        );
        assert_eq!(
            text.matches("known cf000000 (2x): framework noise").count(),
            1,
            "{text}"
        );
        assert!(text.contains("  hint: ignore it"), "{text}");
        assert!(
            text.trim_end()
                .ends_with("hidden: 0 as baseline, 2 as known"),
            "{text}"
        );
    }

    #[test]
    fn a_run_with_no_known_errors_prints_the_count_line_and_the_errors_with_ids() {
        let errors = vec![mapped("Can't find variable 'x'", Some("Mod/scripts/a.c"))];
        let (lines, _) = errors_lines("run-client", &errors, false);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], "run-client: 1 issue(s) (0 file-mapped)");
        assert!(lines[1].contains("Can't find variable 'x'"), "{lines:?}");
        assert!(lines[1].contains("(id "), "{lines:?}");
    }

    #[test]
    fn errors_takes_the_current_folder_when_it_holds_a_config_cpp() {
        let folder = std::env::temp_dir().join(format!("mf-errors-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let flags = |source_root: Option<&str>, known_errors: Option<&str>| ErrorsFlags {
            source_root: source_root.map(PathBuf::from),
            baseline: None,
            known_errors: known_errors.map(PathBuf::from),
            learn: None,
            meaning: None,
            hint: None,
            all: false,
            json: false,
        };

        assert_eq!(errors_source_root(&flags(None, None), &folder), None);

        std::fs::write(folder.join("config.cpp"), "class CfgPatches {};\n").unwrap();
        assert_eq!(
            errors_source_root(&flags(None, None), &folder),
            Some(PathBuf::from("."))
        );
        assert_eq!(
            errors_source_root(&flags(Some("src"), None), &folder),
            Some(PathBuf::from("src"))
        );
        assert_eq!(
            errors_source_root(&flags(None, Some("server/known-errors.json")), &folder),
            None,
            "an explicit catalogue keeps the folder out of it"
        );

        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn errors_help_says_the_current_folder_is_the_default_source_root() {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        let help = cli
            .find_subcommand_mut("errors")
            .unwrap()
            .render_long_help()
            .to_string();
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            help.contains("the current folder when it holds a config.cpp"),
            "{help}"
        );
    }

    #[test]
    fn no_client_note_follows_the_saved_server_only_mark() {
        assert_eq!(
            no_client_note(&json!({"saved_server_only": true})),
            Some("no client: the app's saved launch target is Server only; pass --client to start one")
        );
        assert_eq!(no_client_note(&json!({"settings_used": ["client"]})), None);
    }

    #[test]
    fn publish_sends_content_only_and_change_note_only_when_given() {
        let args = |extra: &[&str]| {
            let mut argv = vec!["mf", "publish", "mod"];
            argv.extend_from_slice(extra);
            let cli = Cli::try_parse_from(argv).unwrap();
            let Commands::Publish {
                target,
                flags,
                content_only,
                change_note,
            } = cli.command
            else {
                panic!("publish must parse");
            };
            publish_args(target, flags, content_only, change_note)
        };

        let plain = args(&[]);
        assert!(plain.get("content_only").is_none());
        assert!(plain.get("change_note").is_none());

        let given = args(&["--content-only", "--change-note", "Fixed"]);
        assert_eq!(given["content_only"], json!(true));
        assert_eq!(given["change_note"], json!("Fixed"));
    }

    #[test]
    fn progress_line_names_each_upload_step() {
        let stage = |name: &str| serde_json::from_str(&format!("\"{name}\"")).unwrap();
        let uploading = |name: &str| {
            (
                UploadState::Uploading {
                    stage: stage(name),
                    done: 5,
                    total: 10,
                },
                format!("uploading: {name} 5/10 bytes"),
            )
        };
        let cases = [
            (UploadState::Preparing, "preparing".to_owned()),
            (UploadState::Creating, "creating the item".to_owned()),
            uploading("preparing_config"),
            uploading("preparing_content"),
            uploading("uploading_content"),
            uploading("uploading_preview"),
            uploading("committing"),
            (UploadState::Deleting, "deleting the new item".to_owned()),
        ];
        for (state, want) in cases {
            assert_eq!(progress_line(state), want, "{state:?}");
        }
    }

    #[test]
    fn build_sends_each_packing_choice_only_when_given() {
        let cases: &[(&[&str], &str, Option<bool>)] = &[
            (&["mod"], "binarize_config", None),
            (&["mod", "--binarize"], "binarize_config", Some(true)),
            (&["mod", "--no-binarize"], "binarize_config", Some(false)),
            (&["mod"], "strip_comments", None),
            (&["mod", "--strip-comments"], "strip_comments", Some(true)),
            (
                &["mod", "--no-strip-comments"],
                "strip_comments",
                Some(false),
            ),
        ];
        for (args, key, expected) in cases {
            assert_eq!(
                build_request_of(args).get(key).and_then(Value::as_bool),
                *expected,
                "{args:?} {key}"
            );
        }
    }

    #[test]
    fn a_hit_line_names_its_range_its_kind_and_its_score() {
        let hit = |lines: [u64; 2], class: Option<&str>, method: Option<&str>| {
            json!({
                "path": "scripts/a.c",
                "kind": "script",
                "class": class,
                "method": method,
                "source": "game",
                "lines": lines,
                "score": 0.25,
                "stale": false,
                "reference_only": false,
            })
        };
        let cases = [
            (
                hit([4, 9], Some("Vehicle"), Some("Start")),
                "scripts/a.c:4-9  script  Vehicle::Start  game  0.2500",
            ),
            (
                hit([4, 4], Some("Vehicle"), None),
                "scripts/a.c:4  script  Vehicle  game  0.2500",
            ),
            (
                hit([4, 6], None, None),
                "scripts/a.c:4-6  script  game  0.2500",
            ),
            (
                hit([4, 6], None, Some("Main")),
                "scripts/a.c:4-6  script  Main  game  0.2500",
            ),
        ];
        for (hit, expected) in cases {
            assert_eq!(hit_line(&hit), expected);
        }

        let mut flagged = hit([4, 4], None, None);
        flagged["stale"] = json!(true);
        flagged["reference_only"] = json!(true);
        assert_eq!(
            hit_line(&flagged),
            "scripts/a.c:4  script  game  0.2500  stale  reference-only"
        );
    }

    fn build_request_of(extra: &[&str]) -> Value {
        let mut argv = vec!["mf", "build"];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Commands::Build {
            target,
            flags,
            set,
            source_root,
            ..
        } = cli.command
        else {
            panic!("build must parse");
        };
        build_request(target, set, source_root, flags)
    }

    #[test]
    fn build_sends_a_set_and_its_source_root_only_when_given() {
        let plain = build_request_of(&["mod"]);
        assert_eq!(plain["mod_root"], json!("mod"));
        assert!(plain.get("set").is_none());
        assert!(plain.get("source_root").is_none());

        let set = build_request_of(&["--set", "session"]);
        assert!(set.get("mod_root").is_none());
        assert_eq!(set["set"], json!("session"));
        assert!(set.get("source_root").is_none());

        let scoped = build_request_of(&["--set", "session", "--source-root", "proj"]);
        assert_eq!(scoped["source_root"], json!("proj"));
    }

    #[test]
    fn build_takes_one_target_a_mod_root_or_a_set() {
        assert!(Cli::try_parse_from(["mf", "build"]).is_err());
        assert!(Cli::try_parse_from(["mf", "build", "mod", "--set", "session"]).is_err());
        assert!(Cli::try_parse_from(["mf", "build", "mod", "--source-root", "proj"]).is_err());
        assert!(Cli::try_parse_from(["mf", "build", "--set", "session", "--list"]).is_err());
    }

    #[test]
    fn a_set_run_takes_its_mod_from_the_set_and_the_current_folder() {
        let cli = Cli::try_parse_from(["mf", "run-diag", "--set", "session"]).unwrap();
        let Commands::RunDiag { target, flags } = cli.command else {
            panic!("run-diag must parse");
        };
        let args = run_diag_args(target, flags);
        assert!(args.get("mod_dir").is_none(), "{args}");
        assert_eq!(args["source_root"], json!(std::env::current_dir().unwrap()));

        let cli = Cli::try_parse_from(["mf", "run", "--set", "session"]).unwrap();
        let Commands::Run { target, flags } = cli.command else {
            panic!("run must parse");
        };
        let args = run_args(target, flags);
        assert!(args.get("mod_dir").is_none(), "{args}");
        assert_eq!(args["source_root"], json!(std::env::current_dir().unwrap()));

        let cli = Cli::try_parse_from(["mf", "run-client", "--set", "session"]).unwrap();
        let Commands::RunClient { flags } = cli.command else {
            panic!("run-client must parse");
        };
        let args = run_client_args(flags);
        assert_eq!(args["source_root"], json!(std::env::current_dir().unwrap()));

        assert!(Cli::try_parse_from(["mf", "run-diag", "--source-root", "src"]).is_err());
        assert!(Cli::try_parse_from(["mf", "run-diag", "builds/@X"]).is_err());
        assert!(Cli::try_parse_from(["mf", "run"]).is_err());
    }

    #[test]
    fn run_diag_timeout_flag_is_optional() {
        let cli = Cli::try_parse_from(["mf", "run-diag", "builds/dev/@X", "--source-root", "src"])
            .unwrap();
        let Commands::RunDiag { flags, .. } = cli.command else {
            panic!("run-diag must parse");
        };
        assert_eq!(flags.timeout, None, "omitted --timeout keeps the run going");

        let cli = Cli::try_parse_from([
            "mf",
            "run-diag",
            "builds/dev/@X",
            "--source-root",
            "src",
            "--timeout",
            "5",
        ])
        .unwrap();
        let Commands::RunDiag { flags, .. } = cli.command else {
            panic!("run-diag must parse");
        };
        assert_eq!(flags.timeout, Some(5));
    }
}
