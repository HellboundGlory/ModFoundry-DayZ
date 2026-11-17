//! A launch with a database is recorded in its run row while it runs.

use std::path::Path;

use crate::common::{self, tmp};
use modfoundry_commands::registry::Session;
use modfoundry_core::{Actor, CommandError, Outcome};
use modfoundry_store::run::{NewRun, RunState};
use modfoundry_store::Store;
use serde_json::json;

/// A fake DayZServer install whose executable exits at once. Never touches the real game.
#[cfg(unix)]
fn fake_exiting_server(install: &Path) {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

/// A fake DayZServer install whose executable leaves `marker` behind when it runs.
#[cfg(unix)]
fn fake_marking_server(install: &Path, marker: &Path) {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    let body = format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display());
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn diag_run_records_its_launch_and_ends_the_row_when_the_server_exits() {
    let install = tmp("srv");
    fake_exiting_server(&install);
    let mod_dir = tmp("mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("pro");
    let db = tmp("db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let session = Session::new().with_db(db.clone()).with_actor("cli");
    let core = common::core_with(&session);
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "timeout_secs": 30,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run failed: {e}"));
    let Outcome::Data(report) = out else {
        panic!("expected Data, got {out:?}");
    };

    let run_id = report["run_id"].as_str().expect("the result names its run");
    let row = Store::open(&db)
        .unwrap()
        .run(run_id)
        .unwrap()
        .expect("the launch has a run row");
    assert_eq!(row.actor, "cli");
    assert_eq!(row.command, "diag.run");
    assert_eq!(row.log_dirs, vec![profiles.clone()]);
    assert_eq!(row.state, RunState::Ended);
    assert_eq!(row.outcome.as_deref(), Some("exited"));

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
}

#[test]
fn diag_stop_asks_a_live_run_of_another_process_and_skips_a_dead_one() {
    let db = tmp("db-stop").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = Store::open(&db).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (id, at_millis) in [("live-1", now * 1000), ("dead-1", 1_000)] {
        store
            .start_run(&NewRun {
                id: id.into(),
                actor: "cli".into(),
                command: "diag.run".into(),
                source_root: None,
                profiles_dir: None,
                log_dirs: Vec::new(),
                at_millis,
            })
            .unwrap();
    }

    let core = common::core_with(&Session::new().with_db(db.clone()));
    let stop = |id: &str| {
        let out = core
            .dispatch(Actor::CliSession, "diag.stop", json!({ "run_id": id }))
            .unwrap_or_else(|e| panic!("diag.stop failed: {e}"));
        let Outcome::Data(v) = out else {
            panic!("expected Data, got {out:?}");
        };
        v
    };

    assert_eq!(stop("live-1"), json!({ "stopped": true }));
    assert!(store.run_stop_requested("live-1").unwrap());
    assert_eq!(
        stop("dead-1"),
        json!({ "stopped": false }),
        "nobody is left to stop a run its launcher stopped keeping current"
    );
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

fn seed_running(db: &Path, id: &str, actor: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    Store::open(db)
        .unwrap()
        .start_run(&NewRun {
            id: id.into(),
            actor: actor.into(),
            command: "diag.run".into(),
            source_root: None,
            profiles_dir: None,
            log_dirs: Vec::new(),
            at_millis: now,
        })
        .unwrap();
}

#[cfg(unix)]
#[test]
fn a_launch_under_a_live_run_id_is_refused_before_it_starts() {
    for wait in [true, false] {
        let install = tmp("srv-dup");
        let marker = tmp("dup-marker").join("ran");
        fake_marking_server(&install, &marker);
        let mod_dir = tmp("mod-dup").join("@Mod");
        std::fs::create_dir_all(&mod_dir).unwrap();
        let profiles = tmp("pro-dup");
        let db = tmp("db-dup").join("agents.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        seed_running(&db, "dup", "other");

        let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
        let refused = core
            .dispatch(
                Actor::CliSession,
                "diag.run",
                json!({
                    "mod_dir": mod_dir,
                    "server_dir": install,
                    "profiles_dir": profiles,
                    "run_id": "dup",
                    "wait": wait,
                }),
            )
            .expect_err("a live run id is refused");
        let CommandError::Failed(message) = refused else {
            panic!("{wait}: expected Failed, got {refused:?}");
        };
        assert!(
            message.contains("`dup` is already running"),
            "{wait}: {message}"
        );
        assert!(
            !marker.exists(),
            "{wait}: the refused launch still ran the server"
        );

        let row = Store::open(&db).unwrap().run("dup").unwrap().unwrap();
        assert_eq!(row.actor, "other", "{wait}");
        assert_eq!(row.state, RunState::Running, "{wait}");
        assert_eq!(row.outcome, None, "{wait}");

        for dir in [&install, &profiles] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(unix)]
#[test]
fn a_finished_run_id_can_launch_again() {
    let install = tmp("srv-again");
    fake_exiting_server(&install);
    let mod_dir = tmp("mod-again").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("pro-again");
    let db = tmp("db-again").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    {
        seed_running(&db, "again", "other");
        let store = Store::open(&db).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        store.end_run("again", now, "exited").unwrap();
    }

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "run_id": "again",
                "timeout_secs": 30,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run failed: {e}"));
    assert!(matches!(out, Outcome::Data(_)), "{out:?}");

    let row = Store::open(&db).unwrap().run("again").unwrap().unwrap();
    assert_eq!(row.state, RunState::Ended);
    assert_eq!(row.outcome.as_deref(), Some("exited"));
    assert_eq!(row.actor, "cli");

    for dir in [&install, &profiles] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn status(core: &modfoundry_core::Core, run_id: &str) -> serde_json::Value {
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.status",
            json!({ "run_id": run_id }),
        )
        .unwrap_or_else(|e| panic!("diag.status failed: {e}"));
    let Outcome::Data(value) = out else {
        panic!("expected Data, got {out:?}");
    };
    value
}

#[test]
fn diag_status_reports_a_live_run_with_its_log_folders() {
    let db = tmp("db-status-live").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = Store::open(&db).unwrap();
    store
        .start_run(&NewRun {
            id: "live-1".into(),
            actor: "other".into(),
            command: "diag.run_diag".into(),
            source_root: Some("/w/mod".into()),
            profiles_dir: None,
            log_dirs: vec!["/w/logs".into(), "/w/more".into()],
            at_millis: now_millis(),
        })
        .unwrap();
    assert!(store.request_run_stop("live-1").unwrap());

    let core = common::core_with(&Session::new().with_db(db.clone()));
    let answer = status(&core, "live-1");

    let mut keys: Vec<&str> = answer
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "actor",
            "command",
            "elapsed_secs",
            "log_dirs",
            "outcome",
            "run_id",
            "source_root",
            "started_millis",
            "state",
            "stop_requested",
        ]
    );
    assert_eq!(answer["state"], json!("running"));
    assert_eq!(answer["run_id"], json!("live-1"));
    assert_eq!(answer["command"], json!("diag.run_diag"));
    assert_eq!(answer["actor"], json!("other"));
    assert_eq!(answer["source_root"], json!("/w/mod"));
    assert_eq!(answer["log_dirs"], json!(["/w/logs", "/w/more"]));
    assert_eq!(answer["stop_requested"], json!(true));
    assert_eq!(answer["outcome"], json!(null));
    assert_eq!(answer["elapsed_secs"], json!(0));
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn diag_status_reports_a_run_that_stopped_reporting_as_lost() {
    let db = tmp("db-status-lost").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    Store::open(&db)
        .unwrap()
        .start_run(&NewRun {
            id: "gone-1".into(),
            actor: "other".into(),
            command: "diag.run".into(),
            source_root: None,
            profiles_dir: None,
            log_dirs: Vec::new(),
            at_millis: 1_000,
        })
        .unwrap();
    Store::open(&db)
        .unwrap()
        .set_run_result("gone-1", &json!({"partial": true}))
        .unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()));
    let answer = status(&core, "gone-1");

    assert_eq!(answer["state"], json!("lost"));
    assert!(answer.get("result").is_none(), "{answer}");
    assert_eq!(answer["outcome"], json!(null));
    assert_eq!(answer["elapsed_secs"], json!(0));
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn diag_status_gives_an_ended_runs_outcome_and_elapsed_time() {
    for outcome in ["stopped", "failed: boom"] {
        let db = tmp("db-status-ended").join("agents.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let store = Store::open(&db).unwrap();
        store.start_run(&new_run("run-1", 100_000)).unwrap();
        store.end_run("run-1", 130, outcome).unwrap();

        let core = common::core_with(&Session::new().with_db(db.clone()));
        let answer = status(&core, "run-1");

        assert_eq!(answer["state"], json!("ended"), "{outcome}");
        assert_eq!(answer["outcome"], json!(outcome));
        assert_eq!(answer["elapsed_secs"], json!(30), "{outcome}");
        assert!(answer.get("result").is_none(), "{outcome}: {answer}");
        assert_eq!(answer["started_millis"], json!(100_000));
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
}

fn new_run(id: &str, at_millis: i64) -> NewRun {
    NewRun {
        id: id.into(),
        actor: "agent:1".into(),
        command: "diag.run".into(),
        source_root: None,
        profiles_dir: None,
        log_dirs: Vec::new(),
        at_millis,
    }
}

#[cfg(unix)]
#[test]
fn a_waited_diag_run_leaves_its_answer_for_diag_status() {
    let install = tmp("srv-waited");
    fake_exiting_server(&install);
    let mod_dir = tmp("mod-waited").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let profiles = tmp("pro-waited");
    let db = tmp("db-waited").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "run_id": "st1",
                "timeout_secs": 30,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run failed: {e}"));
    let Outcome::Data(mut launched) = out else {
        panic!("expected Data, got {out:?}");
    };
    launched.as_object_mut().unwrap().remove("run_id");
    launched.as_object_mut().unwrap().remove("settings_used");

    let answer = status(&core, "st1");
    assert_eq!(answer["state"], json!("ended"));
    assert_eq!(answer["outcome"], json!("exited"));
    assert_eq!(
        answer["result"], launched,
        "diag.status keeps what the waited launch answered"
    );

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn a_detached_runs_result_is_there_for_diag_status_once_it_ends() {
    let install = tmp("srv-detached");
    let profiles = tmp("pro-detached");
    let release = tmp("release-detached").join("go");
    std::fs::create_dir_all(release.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("DayZServer");
    let body = format!(
        "#!/bin/sh\necho 'Player connect enabled' >> '{}/DayZServer_x64.RPT'\n\
         n=0; while [ ! -e '{}' ] && [ -e \"$0\" ] && [ $n -lt 600 ]; do sleep 0.05; n=$((n+1)); done\n",
        profiles.display(),
        release.display()
    );
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mod_dir = tmp("mod-detached").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("db-detached").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "server_dir": install,
                "profiles_dir": profiles,
                "run_id": "det1",
                "wait": false,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run failed: {e}"));
    let Outcome::Data(launched) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(launched["state"], json!("running"));

    let running = status(&core, "det1");
    assert_eq!(running["state"], json!("running"));
    assert!(running.get("result").is_none(), "{running}");

    std::fs::write(&release, "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let ended = loop {
        let answer = status(&core, "det1");
        if answer["state"] == json!("ended") {
            break answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the run never ended: {answer}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(ended["outcome"], json!("exited"));
    assert_eq!(ended["result"]["report"]["exit_code"], json!(0), "{ended}");

    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn diag_status_names_a_run_nobody_recorded() {
    let db = tmp("db-status-none").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    Store::open(&db).unwrap();

    for session in [Session::new().with_db(db.clone()), Session::new()] {
        let core = common::core_with(&session);
        let refused = core
            .dispatch(
                Actor::CliSession,
                "diag.status",
                json!({ "run_id": "nope" }),
            )
            .expect_err("an unknown run is an error");
        let CommandError::Failed(message) = refused else {
            panic!("expected Failed, got {refused:?}");
        };
        assert!(message.contains("no run `nope` is recorded"), "{message}");
    }

    let core = common::core_with(&Session::new());
    let refused = core
        .dispatch(Actor::CliSession, "diag.status", json!({}))
        .expect_err("a run id is required");
    assert!(
        matches!(refused, CommandError::InvalidArguments(_)),
        "{refused:?}"
    );
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn diag_run_client_records_its_launch_and_ends_the_row_when_the_client_exits() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("client-record");
    let log_dir = crate::diag_ops::fake_steam(&dir);
    let game = tmp("client-game");
    let launcher = dir.join("launcher");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{}/DayZ_x64_x.RPT'\n",
            log_dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, PermissionsExt::from_mode(0o755)).unwrap();
    let db = dir.join("agents.db");

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let out = core
        .dispatch(
            Actor::CliSession,
            "diag.run_client",
            json!({
                "mods": [],
                "game_dir": game,
                "steam_dir": dir,
                "launcher": launcher,
                "connect": "10.0.0.5:2302",
                "timeout_secs": 5,
                "settle_secs": 0,
                "run_id": "c1",
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run_client failed: {e}"));
    let Outcome::Data(answer) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(answer["run_id"], json!("c1"));

    let store = Store::open(&db).unwrap();
    let row = store.run("c1").unwrap().expect("the launch was recorded");
    assert_eq!(row.actor, "cli");
    assert_eq!(row.command, "diag.run_client");
    assert_eq!(row.log_dirs, vec![log_dir.clone()]);
    assert_eq!(row.state, RunState::Ended);
    assert_eq!(row.outcome.as_deref(), Some("exited"));
    let result = store.run_result("c1").unwrap().expect("a result");
    assert_eq!(result["log_dir"], json!(log_dir));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&game);
}

#[cfg(unix)]
#[test]
fn diag_run_diag_records_its_launch_and_a_failed_start_ends_the_row_as_failed() {
    let game = tmp("diag-game");
    std::fs::create_dir_all(&game).unwrap();
    let profiles = tmp("diag-profiles");
    let source = tmp("diag-source");
    std::fs::create_dir_all(&source).unwrap();
    let mod_dir = tmp("diag-mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = tmp("diag-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let core = common::core_with(&Session::new().with_db(db.clone()).with_actor("cli"));
    let refused = core
        .dispatch(
            Actor::CliSession,
            "diag.run_diag",
            json!({
                "mod_dir": mod_dir,
                "source_root": source,
                "game_dir": game,
                "profiles_dir": profiles,
                "client": false,
                "run_id": "d1",
            }),
        )
        .expect_err("a game folder with no Diag exe cannot launch");
    let CommandError::Failed(message) = refused else {
        panic!("expected Failed, got {refused:?}");
    };
    assert!(message.contains("DayZDiag_x64.exe not found"), "{message}");

    let store = Store::open(&db).unwrap();
    let row = store.run("d1").unwrap().expect("the attempt was recorded");
    assert_eq!(row.command, "diag.run_diag");
    assert_eq!(row.source_root, Some(source));
    assert_eq!(row.log_dirs, vec![profiles]);
    assert_eq!(row.state, RunState::Ended);
    assert!(
        row.outcome
            .as_deref()
            .is_some_and(|outcome| outcome.starts_with("failed: DayZDiag_x64.exe not found")),
        "{:?}",
        row.outcome
    );
    assert_eq!(store.run_result("d1").unwrap(), None);

    let _ = std::fs::remove_dir_all(&game);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}
