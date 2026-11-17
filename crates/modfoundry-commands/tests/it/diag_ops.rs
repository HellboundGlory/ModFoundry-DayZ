//! Contract tests for the launch + diagnostics shared bodies (`mf run` /
//! `mf errors`). These pin the operations the CLI binds.

use crate::common::{self, tmp, write};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Real-shaped RPT with a script warning + an engine error + a VM exception.
const SAMPLE_RPT: &str = "15:52:04.285  SCRIPT    (W): @\"MyMod/scripts/5_Mission/ui\\ui.c,12\": FIX-ME: Undefined function 'X'\r\n\
14:56:29.490 GUI       (E): LoadImageSet 'Missing/GUI.imageset' file not exist\r\n\
15:52:04.974  SCRIPT    (E): Virtual Machine Exception\r\n\r\nReason: Function 'SetTransform' not linked\r\n\r\nStack trace:\r\nMyMod/scripts/3_Game/game.c:99 Function ApplyTextStyle\r\n";

#[test]
fn errors_from_dir_parses_and_maps_all_logs() {
    let dir = tmp("parse");
    write(&dir, "DayZServer_x64.RPT", SAMPLE_RPT);
    write(
        &dir,
        "script.log",
        " SCRIPT    (W): @\"Other/scripts/4_World/w.c,3\": FIX-ME: ref\r\n",
    );
    // Source root that should resolve the MyMod/scripts paths.
    let src = tmp("src");
    write(&src, "scripts/5_mission/ui/ui.c", "class U {}\n");
    write(&src, "scripts/3_game/game.c", "class G {}\n");

    let errs = modfoundry_commands::errors_from_dir(&dir, Some(&src)).unwrap();
    // Warning + GUI error + VM exception + second file warning = 4 errors.
    assert_eq!(errs.len(), 4);
    // VM exception maps to innermost frame file:line.
    let vm = errs
        .iter()
        .find(|e| e.message == "Virtual Machine Exception")
        .unwrap();
    assert_eq!(vm.line, Some(99));
    assert!(vm
        .file
        .as_deref()
        .unwrap()
        .ends_with("scripts/3_game/game.c"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn errors_from_dir_reads_an_rpt_with_non_utf8_bytes() {
    let dir = tmp("badutf8");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ErrorMessage_DayZ_x64.mdmp"),
        [0xFFu8, 0x00, 0x9C, 0xFE, 0x00, 0x1F],
    )
    .unwrap();
    std::fs::write(
        dir.join("DayZ_x64.RPT"),
        b"player caf\xE9\r\n SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'undefined_symbol_x'\r\n",
    )
    .unwrap();
    let src = tmp("badutf8src");
    write(&src, "scripts/3_game/broken.c", "class Broken {}\n");

    let errs = modfoundry_commands::errors_from_dir(&dir, Some(&src)).unwrap();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].line, Some(5));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn errors_from_dir_dedups_error_mirrored_across_log_files() {
    let dir = tmp("dedup");
    // The same compile error appears in the RPT and in error.log/script.log.
    let line = " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'undefined_symbol_x'\r\n";
    write(&dir, "DayZServer_2026.RPT", line);
    write(&dir, "script.log", line);
    write(&dir, "error.log", line);
    let src = tmp("dedupsrc");
    write(&src, "scripts/3_game/broken.c", "class Broken {}\n");

    let errs = modfoundry_commands::errors_from_dir(&dir, Some(&src)).unwrap();
    // Mirrored across three files but one underlying error.
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].line, Some(5));
    assert!(errs[0]
        .file
        .as_deref()
        .unwrap()
        .ends_with("scripts/3_game/broken.c"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn errors_from_dir_returns_empty_for_missing_dir_or_clean_logs() {
    let empty = tmp("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(modfoundry_commands::errors_from_dir(&empty, None)
        .unwrap()
        .is_empty());
    assert!(
        modfoundry_commands::errors_from_dir(&empty.join("nope"), None)
            .unwrap()
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&empty);
}

#[cfg(unix)]
#[test]
fn run_server_uses_relative_mod_path_under_fake_dayzserver_dir() {
    // A fake DayZServer install dir: a `DayZServer` script that records its
    // argv + cwd, writes an RPT, then exits. run_server must spawn it with
    // cwd = the install dir and pass -mod as a RELATIVE path (the engine
    // ignores absolute -mod on Linux) under that install dir.
    let install = tmp("srv");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("DayZServer");
    let recording = install.join("argv.txt");
    let rec = recording.display();
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\npwd > \"{rec}\"\nprintf '%s\\n' \"$@\" >> \"{rec}\"\nprof=''; for a in \"$@\"; do case \"$a\" in -profiles=*) prof=\"${{a#-profiles=}}\";; esac; done\nprintf '15:00:00.000  SCRIPT    (E): boom\\r\\n' > \"$prof/x.RPT\"\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The built @Mod must sit under the fake install dir (as the engine requires).
    let mod_dir = install.join("@mymod");
    std::fs::create_dir_all(&mod_dir).unwrap();

    let profiles = tmp("runpro");
    std::fs::create_dir_all(&profiles).unwrap();
    let args = modfoundry_commands::RunServerArgs {
        mod_dir,
        server_dir: Some(install.clone()),
        config: None,
        set: None,
        set_config: None,
        port: Some(2302),
        profiles_dir: Some(profiles.clone()),
        timeout_secs: 10,
        source_root: None,
        run_id: None,
        test_mods: Vec::new(),
        expect_lines: Vec::new(),
        fail_lines: Vec::new(),
        allow_errors: Vec::new(),
    };
    let outcome =
        modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new()).unwrap();
    assert_eq!(outcome.report.exit_code, Some(0));
    let recording_text = std::fs::read_to_string(&recording).unwrap();
    let lines: Vec<&str> = recording_text.lines().collect();
    assert_eq!(lines[0], install.to_string_lossy()); // cwd == install dir
    let has_relative_mod = lines.contains(&"-mod=@mymod");
    assert!(
        has_relative_mod,
        "expected relative -mod in argv, got {lines:?}"
    );
    assert!(!lines
        .iter()
        .any(|l| l.contains("@mymod") && l.starts_with("-mod=/")));
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
}

#[cfg(unix)]
#[test]
fn run_report_locates_and_lists_captured_logs() {
    // Launch a real /bin/sh child that writes a script.log then exits — the
    // process-bridge path `mf run` binds, without needing a DayZServer binary.
    let dir = tmp("run");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = PathBuf::from("/bin/sh");
    // `sh -c CMD DIR` sets $0 = DIR (the first arg after the command string).
    let argv = vec![
        "-c".to_string(),
        "printf ' SCRIPT    (E): Virtual Machine Exception\\r\\n' > \"$0/script.log\"; sleep 0.2"
            .to_string(),
        dir.to_string_lossy().into_owned(),
    ];
    let report = modfoundry_commands::run_profile(
        &exe,
        &argv,
        &dir,
        10,
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();
    assert_eq!(report.exit_code, Some(0));
    assert!(!report.timed_out);
    assert!(report.logs.iter().any(|l| l.ends_with("script.log")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
#[ignore = "real process, over a second: cargo test -- --ignored"]
fn run_client_stages_mod_discovers_prefix_and_maps_new_logs() {
    // A fake Steam library: compatdata/221100/pfx with a client log dir, a DayZ
    // game dir, and a fake `steam` launcher that writes a client RPT + script.log
    // holding a mod script error. run_client must: resolve the pfx log dir from
    // the Steam root, stage the @Mod as a game-dir symlink (removing it when the
    // run ends), launch the launcher, and map the *new* logs' error back to the
    // mod source tree.
    let steam_root = tmp("steamroot");
    let game_dir = steam_root.join("steamapps/common/DayZ");
    let log_dir = steam_root
        .join("steamapps/compatdata/221100/pfx/drive_c/users/steamuser/AppData/Local/DayZ");
    std::fs::create_dir_all(&game_dir).unwrap();
    std::fs::create_dir_all(&log_dir).unwrap();

    // A pre-existing log that must NOT be reported or mapped.
    std::fs::write(
        log_dir.join("stale.RPT"),
        " SCRIPT    (E): @\"Other/x.c,1\": old\r\n",
    )
    .unwrap();

    // The mod source root with the broken script the client will report.
    let src = tmp("clientsrc");
    write(&src, "Scripts/3_Game/Broken.c", "class X {}\n");
    // The built @Mod dir that gets staged into the game dir.
    let mod_dir = tmp("clientmod").join("@p3diag");
    std::fs::create_dir_all(&mod_dir).unwrap();

    // Fake launcher: writes the new-run logs into the (fixed) log dir, then exits.
    let launcher = tmp("fakesteam").with_extension("sh");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf ' 1:17:37.261 ENGINE    (E): boom\\r\\n' > \"{d}/DayZ_x64_x.RPT\"\nprintf ' SCRIPT    (E): @\"p3diag/Scripts/3_Game/Broken.c,5\": Can\\047t find variable\\r\\n' > \"{d}/script_x.log\"\n",
            d = log_dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &launcher,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let args = modfoundry_commands::RunClientArgs {
        mods: vec![mod_dir.clone()],
        set: None,
        game_dir: Some(game_dir.clone()),
        steam_dir: Some(steam_root.clone()),
        launcher: Some(launcher.clone()),
        connect: Some(modfoundry_diag::Connect {
            host: "127.0.0.1".into(),
            game_port: 2402,
            query_port: Some(27016),
        }),
        profile_name: Some("mftest".into()),
        extra: vec!["-nolauncher".into()],
        window: None,
        windowed: true,
        timeout_secs: 15,
        settle_secs: 1, // short settle: the fake launcher writes once and exits
        source_root: Some(src.join("Scripts")),
        test_mods: Vec::new(),
        expect_lines: Vec::new(),
        fail_lines: Vec::new(),
        allow_errors: Vec::new(),
    };
    let outcome =
        modfoundry_commands::run_client(&args, &modfoundry_diag::StopSignal::new()).unwrap();

    // The staged @Mod symlink is gone once the run ends: the client must not
    // leave its dev links behind in the user's game dir.
    let staged = game_dir.join("@p3diag");
    assert!(
        staged.symlink_metadata().is_err(),
        "the staged @p3diag symlink at {} must be removed after the run",
        staged.display()
    );
    // Only the new logs are reported (not the stale one).
    assert!(outcome
        .report
        .logs
        .iter()
        .any(|l| l.ends_with("script_x.log")));
    assert!(!outcome.report.logs.iter().any(|l| l.ends_with("stale.RPT")));
    assert_eq!(outcome.log_dir, log_dir);
    // The engine-logged virtual `p3diag/Scripts/3_Game/Broken.c` maps to the
    // host source file under the source root.
    let mapped = outcome
        .errors
        .iter()
        .find(|e| e.logged_file.as_deref() == Some("p3diag/Scripts/3_Game/Broken.c"))
        .expect("expected the broken-script error");
    // The resolved host path keeps the on-disk case (openable on a
    // case-sensitive filesystem).
    assert_eq!(
        mapped.file.as_deref(),
        Some(src.join("Scripts/3_Game/Broken.c").as_path())
    );

    for d in [&steam_root, &src, &mod_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
    let _ = std::fs::remove_file(&launcher);
}

#[cfg(unix)]
#[test]
fn a_server_run_preview_plans_the_launch_and_stages_nothing() {
    let install = tmp("preview-install");
    fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"preview\";").unwrap();
    let mod_dir = tmp("preview-src").join("builds/default/@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("preview-profiles");

    let before = listing(&install);
    let outcome = common::core()
        .dispatch(
            modfoundry_core::Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "preview": true,
            }),
        )
        .unwrap();

    let modfoundry_core::Outcome::Preview(plan) = outcome else {
        panic!("expected a plan, got {outcome:?}");
    };
    let argv = plan["launches"][0]["args"].as_array().unwrap();
    let names = |prefix: &str| {
        argv.iter()
            .filter_map(|arg| arg.as_str())
            .any(|arg| arg.starts_with(prefix))
    };
    assert!(names("-config="), "{plan}");
    assert!(names("-mod=@FieldKit"), "{plan}");
    assert_eq!(listing(&install), before, "a preview writes nothing");
    assert!(!profiles.exists(), "a preview makes no capture folder");
}

/// The names in `dir`, sorted, so a listing can be compared before and after.
#[cfg(unix)]
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A fake Steam root with the Proton runtime a client launch looks for, returning its client log folder.
#[cfg(unix)]
pub(crate) fn fake_steam(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let common = root.join("steamapps/common");
    for (dir, exe) in [
        ("Proton - Experimental", "proton"),
        ("SteamLinuxRuntime_sniper", "run"),
    ] {
        let dir = common.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(exe);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, PermissionsExt::from_mode(0o755)).unwrap();
    }
    let log_dir =
        root.join("steamapps/compatdata/221100/pfx/drive_c/users/steamuser/AppData/Local/DayZ");
    std::fs::create_dir_all(&log_dir).unwrap();
    log_dir
}

/// A fake install set a preview plans against: a client and server install, a Steam root and a mod's sources.
#[cfg(unix)]
pub(crate) struct PreviewInstall {
    pub game: PathBuf,
    pub server: PathBuf,
    pub steam: PathBuf,
    pub source: PathBuf,
    pub own: PathBuf,
}

#[cfg(unix)]
pub(crate) fn preview_install(label: &str) -> PreviewInstall {
    let root = tmp(label);
    let game = root.join("game");
    std::fs::create_dir_all(&game).unwrap();
    for exe in ["DayZDiag_x64.exe", "DayZ_x64.exe"] {
        std::fs::write(game.join(exe), "").unwrap();
    }
    let server = root.join("server");
    std::fs::create_dir_all(server.join("mpmissions")).unwrap();
    std::fs::write(server.join("serverDZ.cfg"), "BattlEye = 1;").unwrap();
    let steam = root.join("steam");
    fake_steam(&steam);
    let source = root.join("source");
    std::fs::create_dir_all(source.join("server")).unwrap();
    let own = source.join("builds/default/@mod");
    std::fs::create_dir_all(&own).unwrap();
    PreviewInstall {
        game,
        server,
        steam,
        source,
        own,
    }
}

#[cfg(unix)]
#[test]
fn a_client_preview_plans_the_launch_and_stages_nothing() {
    let install = preview_install("preview-client");
    let before = listing(&install.game);

    let outcome = common::core()
        .dispatch(
            modfoundry_core::Actor::CliSession,
            "diag.run_client",
            json!({
                "mods": [install.own],
                "game_dir": install.game,
                "steam_dir": install.steam,
                "connect": "10.0.0.5:2302",
                "preview": true,
            }),
        )
        .unwrap();

    let modfoundry_core::Outcome::Preview(plan) = outcome else {
        panic!("expected a plan, got {outcome:?}");
    };
    let launch = &plan["launches"][0];
    assert_eq!(launch["role"], json!("client"), "{plan}");
    let argv = launch["args"].as_array().unwrap();
    assert!(
        argv.iter()
            .filter_map(|arg| arg.as_str())
            .any(|arg| arg.starts_with("-mod=@mod")),
        "{plan}"
    );
    let steps = plan["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1, "{plan}");
    assert_eq!(steps[0]["op"], json!("link"), "{plan}");
    assert_eq!(listing(&install.game), before, "a preview writes nothing");
}

#[cfg(unix)]
#[test]
fn a_diag_preview_plans_both_launches_and_stages_nothing() {
    let install = preview_install("preview-diag");
    let profiles = install.source.join("profiles-x");
    let before: Vec<Vec<String>> = [&install.game, &install.server, &install.source]
        .iter()
        .map(|dir| listing(dir))
        .collect();
    let db = tmp("preview-diag-db").join("agents.db");

    let outcome =
        common::core_with(&modfoundry_commands::registry::Session::new().with_db(db.clone()))
            .dispatch(
                modfoundry_core::Actor::CliSession,
                "diag.run_diag",
                json!({
                    "mod_dir": install.own,
                    "source_root": install.source,
                    "game_dir": install.game,
                    "server_dir": install.server,
                    "steam_dir": install.steam,
                    "profiles_dir": profiles,
                    "preview": true,
                }),
            )
            .unwrap();

    let modfoundry_core::Outcome::Preview(plan) = outcome else {
        panic!("expected a plan, got {outcome:?}");
    };
    let launches = plan["launches"].as_array().unwrap();
    assert_eq!(launches.len(), 2, "{plan}");
    assert_eq!(launches[0]["role"], json!("server"), "{plan}");
    assert_eq!(launches[1]["role"], json!("client"), "{plan}");
    for launch in launches {
        let argv = launch["args"].as_array().unwrap();
        assert!(
            argv.iter()
                .filter_map(|arg| arg.as_str())
                .any(|arg| arg.starts_with("-mod=@mod")),
            "{plan}"
        );
    }
    let after: Vec<Vec<String>> = [&install.game, &install.server, &install.source]
        .iter()
        .map(|dir| listing(dir))
        .collect();
    assert_eq!(after, before, "a preview writes nothing");
    assert!(!profiles.exists(), "a preview makes no profiles folder");
    assert!(!db.exists(), "a preview records no run");
}

/// A fake `DayZServer` that records its cwd, argv and what `@FieldKit` points
/// at while it runs, writes one RPT into its `-profiles` dir and exits.
#[cfg(unix)]
fn fake_server(install: &Path) -> PathBuf {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    let recording = install.join("argv.txt");
    let rec = recording.display();
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\npwd > \"{rec}\"\nprintf '%s\\n' \"$@\" >> \"{rec}\"\nprintf 'link=%s\\n' \"$(readlink @FieldKit)\" >> \"{rec}\"\nprintf 'keys=%s\\n' \"$(ls keys 2>/dev/null | tr '\\n' ' ')\" >> \"{rec}\"\nprintf 'cfg=%s\\n' \"$(cat serverDZ_modfoundry.cfg 2>/dev/null)\" >> \"{rec}\"\nprof=''; for a in \"$@\"; do case \"$a\" in -profiles=*) prof=\"${{a#-profiles=}}\";; esac; done\nprintf '15:00:00.000  SCRIPT    (E): boom\\r\\n' > \"$prof/x.RPT\"\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    recording
}

#[cfg(unix)]
fn run_args(
    mod_dir: PathBuf,
    install: &Path,
    profiles: &Path,
) -> modfoundry_commands::RunServerArgs {
    modfoundry_commands::RunServerArgs {
        mod_dir,
        server_dir: Some(install.to_path_buf()),
        config: None,
        set: None,
        set_config: None,
        port: Some(2302),
        profiles_dir: Some(profiles.to_path_buf()),
        timeout_secs: 10,
        source_root: None,
        run_id: None,
        test_mods: Vec::new(),
        expect_lines: Vec::new(),
        fail_lines: Vec::new(),
        allow_errors: Vec::new(),
    }
}

/// Arguments for a project whose `server/` folder keeps the profiles and the config.
#[cfg(unix)]
fn project_run_args(
    mod_dir: PathBuf,
    install: &Path,
    root: &Path,
) -> modfoundry_commands::RunServerArgs {
    modfoundry_commands::RunServerArgs {
        mod_dir,
        server_dir: Some(install.to_path_buf()),
        config: None,
        set: None,
        set_config: None,
        port: Some(2302),
        profiles_dir: None,
        timeout_secs: 10,
        source_root: Some(root.to_path_buf()),
        run_id: None,
        test_mods: Vec::new(),
        expect_lines: Vec::new(),
        fail_lines: Vec::new(),
        allow_errors: Vec::new(),
    }
}

#[cfg(unix)]
#[test]
fn run_server_stages_a_mod_built_outside_the_install_and_removes_the_link_after() {
    let install = tmp("stage-srv");
    let recording = fake_server(&install);
    let mod_dir = tmp("stage-src").join("builds/default/@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("stage-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    let outcome = modfoundry_commands::run_server(
        &run_args(mod_dir.clone(), &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .expect("a mod outside the install is staged, not refused");

    assert_eq!(outcome.report.exit_code, Some(0));
    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(
        recorded.lines().any(|l| l == "-mod=@FieldKit"),
        "{recorded}"
    );
    assert!(
        recorded.contains(&format!("link={}", mod_dir.display())),
        "the server saw @FieldKit pointing at the build: {recorded}"
    );
    assert!(
        std::fs::symlink_metadata(install.join("@FieldKit")).is_err(),
        "the staged link is removed once the run ends"
    );
}

#[cfg(unix)]
#[test]
fn run_server_leaves_a_link_it_did_not_create() {
    let install = tmp("keep-srv");
    fake_server(&install);
    let mod_dir = tmp("keep-src").join("builds/default/@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::os::unix::fs::symlink(&mod_dir, install.join("@FieldKit")).unwrap();
    let profiles = tmp("keep-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir.clone(), &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_link(install.join("@FieldKit")).unwrap(),
        mod_dir,
        "a link the user made stays"
    );
}

#[cfg(unix)]
#[test]
fn run_server_defaults_to_the_installs_server_config() {
    let install = tmp("cfg-srv");
    let recording = fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"x\";\n").unwrap();
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("cfg-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(
        recorded.lines().any(|l| l == "-config=serverDZ.cfg"),
        "without -config the server shuts itself down after 10 s: {recorded}"
    );
}

#[cfg(unix)]
#[test]
fn run_server_passes_no_config_when_the_install_has_none() {
    let install = tmp("nocfg-srv");
    let recording = fake_server(&install);
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("nocfg-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(
        !recorded.lines().any(|l| l.starts_with("-config=")),
        "{recorded}"
    );
}

#[cfg(unix)]
#[test]
fn run_server_with_a_source_root_keeps_profiles_and_config_in_the_project_server_folder() {
    let install = tmp("kept-srv");
    let recording = fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"retail\";\n").unwrap();
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let root = tmp("kept-root");
    std::fs::create_dir_all(&root).unwrap();

    let args = project_run_args(mod_dir, &install, &root);
    let outcome =
        modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new()).unwrap();

    let profiles = root.join("server").join("profiles");
    assert_eq!(outcome.profiles_dir, profiles);
    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(
        recorded
            .lines()
            .any(|l| l == format!("-profiles={}", profiles.display())),
        "{recorded}"
    );
    assert!(
        recorded
            .lines()
            .any(|l| l == "-config=serverDZ_modfoundry.cfg"),
        "{recorded}"
    );
    assert!(
        recorded.contains("cfg=hostname = \"retail\";"),
        "the server read the project's config: {recorded}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("server").join("serverDZ.cfg")).unwrap(),
        "hostname = \"retail\";\n"
    );
    assert!(
        !install.join("serverDZ_modfoundry.cfg").exists(),
        "the staged config is removed once the run ends"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&install);
}

#[cfg(unix)]
#[test]
fn run_server_reports_only_the_errors_of_the_logs_this_run_wrote() {
    let install = tmp("errs-srv");
    fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"x\";\n").unwrap();
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let root = tmp("errs-root");
    let kept = root.join("server").join("profiles");
    std::fs::create_dir_all(&kept).unwrap();
    write(
        &kept,
        "DayZServer_earlier.RPT",
        "15:00:00.000  SCRIPT    (E): left behind\r\n",
    );

    let args = project_run_args(mod_dir, &install, &root);
    let outcome =
        modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new()).unwrap();

    let messages: Vec<&str> = outcome.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["boom"]);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&install);
}

#[cfg(unix)]
#[test]
fn run_server_prunes_the_kept_profiles_folder_to_the_newest_logs() {
    let install = tmp("prune-srv");
    fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"x\";\n").unwrap();
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let root = tmp("prune-root");
    let kept = root.join("server").join("profiles");
    std::fs::create_dir_all(&kept).unwrap();
    let base = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
    for i in 0..11 {
        let file = kept.join(format!("DayZServer_{i:02}.RPT"));
        std::fs::write(&file, "15:00:00.000  SCRIPT    (E): old\r\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(base + std::time::Duration::from_secs(i))
            .unwrap();
    }

    let args = project_run_args(mod_dir, &install, &root);
    modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new()).unwrap();

    assert!(!kept.join("DayZServer_00.RPT").exists());
    assert!(kept.join("DayZServer_01.RPT").exists());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&install);
}

#[cfg(unix)]
#[test]
fn run_server_refuses_a_source_root_that_is_not_a_folder() {
    let install = tmp("noroot-srv");
    let recording = fake_server(&install);
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let missing = tmp("noroot-missing");

    let args = project_run_args(mod_dir, &install, &missing);
    let err = modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new())
        .unwrap_err()
        .to_string();

    assert!(err.contains("is not a folder"), "{err}");
    assert!(!recording.exists(), "nothing was launched");
    let _ = std::fs::remove_dir_all(&install);
}

#[cfg(unix)]
#[test]
fn run_server_with_a_source_root_passes_no_config_when_neither_has_one() {
    let install = tmp("nocfg-srv");
    let recording = fake_server(&install);
    let mod_dir = install.join("@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let root = tmp("nocfg-root");
    std::fs::create_dir_all(&root).unwrap();

    let args = project_run_args(mod_dir, &install, &root);
    modfoundry_commands::run_server(&args, &modfoundry_diag::StopSignal::new()).unwrap();

    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(!recorded.contains("-config="), "{recorded}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&install);
}

/// A build's `keys/` dir holding `names`, each with distinct contents.
#[cfg(unix)]
fn mod_keys(mod_dir: &Path, names: &[&str]) {
    let keys = mod_dir.join("keys");
    std::fs::create_dir_all(&keys).unwrap();
    for n in names {
        std::fs::write(keys.join(n), format!("bytes of {n}")).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn run_server_stages_the_builds_public_key_and_removes_it() {
    let install = tmp("key-srv");
    let recording = fake_server(&install);
    let mod_dir = tmp("key-src").join("builds/default/@FieldKit");
    mod_keys(&mod_dir, &["a.bikey"]);
    let profiles = tmp("key-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(
        recorded.lines().any(|l| l == "keys=a.bikey "),
        "the server must see the mod's key while it runs: {recorded}"
    );
    assert!(
        std::fs::symlink_metadata(install.join("keys/a.bikey")).is_err(),
        "the staged key is removed once the run ends"
    );
    assert!(
        std::fs::symlink_metadata(install.join("keys")).is_err(),
        "the keys dir the run created is removed when left empty"
    );
}

#[cfg(unix)]
#[test]
fn run_server_keeps_a_key_that_matches_the_builds() {
    let install = tmp("keysame-srv");
    fake_server(&install);
    let mod_dir = tmp("keysame-src").join("builds/default/@FieldKit");
    mod_keys(&mod_dir, &["a.bikey"]);
    std::fs::create_dir_all(install.join("keys")).unwrap();
    std::fs::write(install.join("keys/a.bikey"), "bytes of a.bikey").unwrap();
    let profiles = tmp("keysame-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(install.join("keys/a.bikey")).unwrap(),
        "bytes of a.bikey",
        "an identical key the user placed is not ours to remove"
    );
}

#[cfg(unix)]
#[test]
fn run_server_refuses_a_conflicting_key_before_launching() {
    let install = tmp("keydiff-srv");
    let recording = fake_server(&install);
    let mod_dir = tmp("keydiff-src").join("builds/default/@FieldKit");
    mod_keys(&mod_dir, &["a.bikey"]);
    std::fs::create_dir_all(install.join("keys")).unwrap();
    std::fs::write(install.join("keys/a.bikey"), "the user's key").unwrap();
    let profiles = tmp("keydiff-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    let err = modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .expect_err("a conflicting key must fail the run");

    assert!(err.to_string().contains("a.bikey"), "{err}");
    assert!(
        !recording.exists(),
        "the server must not launch with a key conflict"
    );
    assert_eq!(
        std::fs::read_to_string(install.join("keys/a.bikey")).unwrap(),
        "the user's key",
        "a conflicting key is never overwritten"
    );
}

#[cfg(unix)]
#[test]
fn run_server_without_build_keys_creates_no_keys_dir() {
    let install = tmp("nokey-srv");
    fake_server(&install);
    let mod_dir = tmp("nokey-src").join("builds/default/@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("nokey-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    assert!(
        std::fs::symlink_metadata(install.join("keys")).is_err(),
        "an unsigned build stages nothing"
    );
}

#[cfg(unix)]
#[test]
fn run_server_never_copies_a_private_key() {
    let install = tmp("privkey-srv");
    let recording = fake_server(&install);
    let mod_dir = tmp("privkey-src").join("builds/default/@FieldKit");
    mod_keys(&mod_dir, &["a.bikey", "a.biprivatekey"]);
    let profiles = tmp("privkey-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .unwrap();

    let recorded = std::fs::read_to_string(&recording).unwrap();
    assert!(recorded.lines().any(|l| l == "keys=a.bikey "), "{recorded}");
    assert!(
        !recorded.contains("biprivatekey"),
        "a private key must never reach the server: {recorded}"
    );
}

#[cfg(unix)]
#[test]
fn run_server_removes_staged_keys_when_the_launch_fails() {
    let install = tmp("keyfail-srv");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let mod_dir = tmp("keyfail-src").join("builds/default/@FieldKit");
    mod_keys(&mod_dir, &["a.bikey"]);
    let profiles = tmp("keyfail-pro");
    std::fs::create_dir_all(&profiles).unwrap();

    modfoundry_commands::run_server(
        &run_args(mod_dir, &install, &profiles),
        &modfoundry_diag::StopSignal::new(),
    )
    .expect_err("a non-executable server binary must fail the launch");

    assert!(
        std::fs::symlink_metadata(install.join("keys/a.bikey")).is_err(),
        "a failed launch still cleans up the staged key"
    );
    assert!(std::fs::symlink_metadata(install.join("keys")).is_err());
}

#[cfg(unix)]
#[test]
fn a_server_run_preview_refuses_a_broken_known_errors_file() {
    let install = tmp("preview-known-install");
    fake_server(&install);
    std::fs::write(install.join("serverDZ.cfg"), "hostname = \"preview\";").unwrap();
    let source = tmp("preview-known-src");
    let mod_dir = source.join("builds/default/@FieldKit");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::create_dir_all(source.join("server")).unwrap();
    std::fs::write(source.join("server/known-errors.json"), "not json").unwrap();

    let err = common::core()
        .dispatch(
            modfoundry_core::Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "source_root": source,
                "profiles_dir": tmp("preview-known-profiles"),
                "preview": true,
            }),
        )
        .unwrap_err()
        .to_string();

    assert!(err.contains("known-errors.json: malformed"), "{err}");
}
