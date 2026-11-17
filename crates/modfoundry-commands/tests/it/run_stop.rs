//! Stopping a live `diag.run` through the registry.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::common::{self, tmp};
use modfoundry_commands::project_settings::{project_id, PREFS_PROJECT_KEY};
use modfoundry_commands::registry::Session;
use modfoundry_core::{Actor, CommandError, Outcome};
use modfoundry_store::run::RunState;
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::{json, Value};

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

#[test]
fn diag_stop_for_an_unknown_run_id_reports_not_stopped() {
    let core = common::core();
    let out = dispatch(&core, "diag.stop", json!({ "run_id": "nope" }));
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(v, json!({ "stopped": false }));
}

/// A fake DayZServer install: an executable `DayZServer` that sleeps far past
/// the test window, so only a stop ends the run. Never touches the real game.
#[cfg(unix)]
fn fake_sleeping_server(install: &Path) {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(&exe, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn diag_stop_ends_a_live_run_and_leaves_no_staged_link() {
    let install = tmp("srv");
    fake_sleeping_server(&install);
    let mod_dir = tmp("mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("pro");
    std::fs::create_dir_all(&profiles).unwrap();

    let core = Arc::new(common::core());
    let args = json!({
        "mod_dir": mod_dir,
        "server_dir": install,
        "profiles_dir": profiles,
        "timeout_secs": 30,
        "run_id": "r1",
    });

    let runner = core.clone();
    let start = Instant::now();
    let run = std::thread::spawn(move || dispatch(&runner, "diag.run", args));

    // The stop lands once the run has registered its signal.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let out = dispatch(&core, "diag.stop", json!({ "run_id": "r1" }));
        let Outcome::Data(v) = out else {
            panic!("expected Data, got {out:?}");
        };
        if v["stopped"] == json!(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the run never registered its signal"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let Outcome::Data(report) = run.join().unwrap() else {
        panic!("expected the run outcome");
    };
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "stop did not end the run: {:?}",
        start.elapsed()
    );
    assert_eq!(report["report"]["stopped"], json!(true));
    assert_eq!(report["report"]["timed_out"], json!(false));
    assert!(
        install.join("@Mod").symlink_metadata().is_err(),
        "the staged @Mod link must be removed once the stopped run ends"
    );

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

/// A fake DayZServer that writes the ready line into its profiles folder, then sleeps until stopped.
#[cfg(unix)]
fn fake_ready_server(install: &Path, profiles: &Path) {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    let body = format!(
        "#!/bin/sh\necho 'Player connect enabled' >> '{}/DayZServer_x64.RPT'\nexec sleep 30\n",
        profiles.display()
    );
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_detached_diag_run_returns_once_ready_and_diag_stop_ends_it() {
    let install = tmp("srv-ready");
    let profiles = tmp("pro-ready");
    fake_ready_server(&install, &profiles);
    let mod_dir = tmp("mod-ready").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("db-ready").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let started = Instant::now();
    let out = dispatch(
        &core,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "run_id": "d1",
            "wait": false,
        }),
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call waited for the run to end"
    );
    let Outcome::Data(launched) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(launched["state"], json!("running"));
    assert_eq!(launched["run_id"], json!("d1"));
    assert!(
        launched.get("stops_after_secs").is_none(),
        "an unsaved run with no timeout names no limit: {launched}"
    );

    let store = Store::open(&db).unwrap();
    assert_eq!(store.run("d1").unwrap().unwrap().state, RunState::Running);
    let Outcome::Data(stop) = dispatch(&core, "diag.stop", json!({ "run_id": "d1" })) else {
        panic!("expected Data from diag.stop");
    };
    assert_eq!(stop["stopped"], json!(true));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = store.run("d1").unwrap().unwrap();
        if row.state == RunState::Ended {
            assert_eq!(row.outcome.as_deref(), Some("stopped"));
            break;
        }
        assert!(Instant::now() < deadline, "the row never ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn a_detached_diag_run_with_a_timeout_names_it_and_ends_timed_out() {
    let install = tmp("srv-bounded");
    let profiles = tmp("pro-bounded");
    fake_ready_server(&install, &profiles);
    let mod_dir = tmp("mod-bounded").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("db-bounded").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let out = dispatch(
        &core,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "timeout_secs": 1,
            "run_id": "b1",
            "wait": false,
        }),
    );
    let Outcome::Data(launched) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(launched["state"], json!("running"));
    assert_eq!(launched["ready"], json!(true));
    assert_eq!(launched["stops_after_secs"], json!(1));

    let store = Store::open(&db).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = store.run("b1").unwrap().unwrap();
        if row.state == RunState::Ended {
            assert_eq!(row.outcome.as_deref(), Some("timed out"));
            break;
        }
        assert!(Instant::now() < deadline, "the row never ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

/// Saves the app's keep-running choice and run length for the project at `root`.
#[cfg(unix)]
fn saved_run_settings(db: &Path, root: &Path, keep_running: bool, run_secs: u64) {
    Store::open(db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(root)),
            PREFS_PROJECT_KEY,
            &json!({
                "launch.window.keep_running": {"Bool": keep_running},
                "launch.window.run_secs": {"Number": run_secs},
            }),
        )
        .unwrap();
}

#[cfg(unix)]
#[test]
fn a_detached_diag_run_follows_saved_keep_running_not_the_saved_run_length() {
    let install = tmp("srv-saved");
    let profiles = tmp("pro-saved");
    fake_ready_server(&install, &profiles);
    let mod_dir = tmp("mod-saved").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let source = tmp("source-saved");
    std::fs::create_dir_all(&source).unwrap();
    let db = tmp("db-saved").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));

    for (keep_running, limit) in [(true, None), (false, Some(7))] {
        saved_run_settings(&db, &source, keep_running, 7);
        let run_id = if keep_running { "k-on" } else { "k-off" };
        let Outcome::Data(launched) = dispatch(
            &core,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "source_root": source,
                "run_id": run_id,
                "wait": false,
            }),
        ) else {
            panic!("expected Data");
        };
        assert_eq!(
            launched.get("stops_after_secs"),
            limit.map(|secs| json!(secs)).as_ref(),
            "keep_running {keep_running}: {launched}"
        );
        assert_eq!(
            launched["settings_used"]
                .as_array()
                .unwrap()
                .contains(&json!("timeout_secs")),
            limit.is_some(),
            "keep_running {keep_running}: {launched}"
        );

        let Outcome::Data(stop) = dispatch(&core, "diag.stop", json!({ "run_id": run_id })) else {
            panic!("expected Data from diag.stop");
        };
        assert_eq!(stop["stopped"], json!(true));
        let store = Store::open(&db).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while store.run(run_id).unwrap().unwrap().state != RunState::Ended {
            assert!(Instant::now() < deadline, "{run_id} never ended");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            store.run(run_id).unwrap().unwrap().outcome.as_deref(),
            Some("stopped")
        );
    }

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn shutting_a_session_down_stops_its_detached_runs_and_ends_their_rows() {
    let install = tmp("srv-shutdown");
    let profiles = tmp("pro-shutdown");
    fake_ready_server(&install, &profiles);
    let mod_dir = tmp("mod-shutdown").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("db-shutdown").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let session = Session::new().with_db(db.clone()).with_actor("cli");
    let core = common::core_with(&session);
    dispatch(
        &core,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "timeout_secs": 10,
            "run_id": "s1",
            "wait": false,
        }),
    );

    session.shutdown(Duration::from_secs(5));

    let row = Store::open(&db).unwrap().run("s1").unwrap().unwrap();
    assert_eq!(row.state, RunState::Ended);
    assert_eq!(row.outcome.as_deref(), Some("stopped"));
    assert!(
        install.join("@Mod").symlink_metadata().is_err(),
        "the staged link is removed before the session ends"
    );
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

/// A Proton runtime and proton that hand the game its argv the way the real pair does, and a
/// Diag exe that reports ready and then waits for its file to go.
#[cfg(unix)]
fn fake_proton_server(install: &crate::diag_ops::PreviewInstall) {
    use std::os::unix::fs::PermissionsExt;

    let write_exec = |path: &Path, body: &str| {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_exec(
        &install
            .steam
            .join("steamapps/common/SteamLinuxRuntime_sniper/run"),
        "#!/bin/sh\nshift\nexec \"$@\"\n",
    );
    write_exec(
        &install
            .steam
            .join("steamapps/common/Proton - Experimental/proton"),
        "#!/bin/sh\nshift\nexec \"$@\"\n",
    );
    write_exec(
        &install.game.join("DayZDiag_x64.exe"),
        "#!/bin/sh\n\
         printf 'Player connect enabled\\r\\n' >> mf-diag-profiles/DayZDiag_x64_1.RPT\n\
         n=0; while [ -e \"$0\" ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done\n",
    );
}

#[cfg(unix)]
#[test]
fn a_detached_diag_run_diag_with_a_timeout_ends_timed_out() {
    let install = crate::diag_ops::preview_install("diag-bounded");
    fake_proton_server(&install);
    let profiles = install.source.join("profiles-x");
    let db = tmp("diag-bounded-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let Outcome::Data(launched) = dispatch(
        &core,
        "diag.run_diag",
        json!({
            "mod_dir": install.own,
            "source_root": install.source,
            "game_dir": install.game,
            "server_dir": install.server,
            "steam_dir": install.steam,
            "profiles_dir": profiles,
            "client": false,
            "timeout_secs": 1,
            "run_id": "t1",
            "wait": false,
        }),
    ) else {
        panic!("expected Data from diag.run_diag");
    };
    assert_eq!(launched["state"], json!("running"));
    assert_eq!(launched["ready"], json!(true), "{launched}");
    assert_eq!(launched["stops_after_secs"], json!(1));

    let store = Store::open(&db).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = store.run("t1").unwrap().unwrap();
        if row.state == RunState::Ended {
            assert_eq!(row.outcome.as_deref(), Some("timed out"));
            break;
        }
        assert!(Instant::now() < deadline, "the row never ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(install.game.parent().unwrap());
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn a_session_shutdown_stops_a_detached_run_with_no_stop_time() {
    let install = tmp("srv-untimed");
    let profiles = tmp("pro-untimed");
    fake_ready_server(&install, &profiles);
    let mod_dir = tmp("mod-untimed").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("db-untimed").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let session = Session::new().with_db(db.clone()).with_actor("cli");
    let core = common::core_with(&session);
    let Outcome::Data(launched) = dispatch(
        &core,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "run_id": "u1",
            "wait": false,
        }),
    ) else {
        panic!("expected Data from diag.run");
    };
    assert!(
        launched.get("stops_after_secs").is_none(),
        "a run with no stop time names none: {launched}"
    );
    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.run("u1").unwrap().unwrap().state,
        RunState::Running,
        "the run is going before the session ends"
    );

    session.shutdown(Duration::from_secs(5));

    let row = store.run("u1").unwrap().unwrap();
    assert_eq!(row.state, RunState::Ended);
    assert_eq!(row.outcome.as_deref(), Some("stopped"));
    assert!(
        install.join("@Mod").symlink_metadata().is_err(),
        "the staged link is removed before the session ends"
    );
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
}

#[test]
fn a_detached_diag_run_needs_the_agent_database() {
    let core = common::core();
    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": "/nonexistent/@Mod",
                "server_dir": "/nonexistent/server",
                "wait": false,
            }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, CommandError::InvalidArguments(m) if m == "wait: false needs the agent database"),
        "{err}"
    );
}

#[test]
fn a_detached_diag_run_with_a_verdict_list_is_refused() {
    let dir = tmp("db-verdict");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("agents.db");
    let core = common::core_with(&Session::new().with_db(db).with_actor("cli"));
    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": "/nonexistent/@Mod",
                "server_dir": "/nonexistent/server",
                "expect_lines": ["DONE"],
                "wait": false,
            }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, CommandError::InvalidArguments(m) if m == "a verdict needs the run to wait; pass wait: true"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_detached_run_with_any_verdict_list_is_refused_for_both_servers() {
    let dir = tmp("db-verdict-lists");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("agents.db");
    let core = common::core_with(&Session::new().with_db(db).with_actor("cli"));
    let base = |command: &str, key: &str| match command {
        "diag.run" => json!({
            "mod_dir": "/nonexistent/@Mod",
            "server_dir": "/nonexistent/server",
            "game_dir": "/nonexistent/game",
            key: ["DONE"],
            "wait": false,
        }),
        _ => json!({
            "mod_dir": "/nonexistent/@Mod",
            "source_root": "/nonexistent/src",
            "game_dir": "/nonexistent/game",
            "server_dir": "/nonexistent/server",
            key: ["DONE"],
            "wait": false,
        }),
    };

    for command in ["diag.run", "diag.run_diag"] {
        for key in ["expect_lines", "fail_lines", "allow_errors"] {
            let err = core
                .dispatch(Actor::CliSession, command, base(command, key))
                .unwrap_err();
            assert!(
                matches!(&err, CommandError::InvalidArguments(m) if m == "a verdict needs the run to wait; pass wait: true"),
                "{command} with {key}: {err}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
