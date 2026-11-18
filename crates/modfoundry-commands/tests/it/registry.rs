//! Registry contract tests.
//!
//! The registry is the single entry point every frontend (CLI, MCP, GUI,
//! automation) dispatches through: these tests pin the registered ids + effect
//! classes, the effect gate for agents, and the JSON outcome each command
//! produces for its frontends.

use std::path::Path;

use crate::common::{self, tmp, write};
use modfoundry_commands::project_settings::{project_id, PREFS_PROJECT_KEY};
use modfoundry_commands::registry;
use modfoundry_core::{Actor, CommandError, EffectKind, Grants, Outcome};
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::{json, Value};

/// Every operation the frontends expose, sorted, with its effect class.
const EXPECTED: &[(&str, EffectKind)] = &[
    ("build.contents", EffectKind::Read),
    ("build.run", EffectKind::Mutate),
    ("ce.coverage", EffectKind::Read),
    ("ce.edit", EffectKind::Mutate),
    ("ce.get_tree", EffectKind::Read),
    ("ce.index", EffectKind::Read),
    ("ce.reference", EffectKind::Read),
    ("ce.source", EffectKind::Read),
    ("ce.validate", EffectKind::Read),
    ("config.binarize", EffectKind::Mutate),
    ("config.derapify", EffectKind::Mutate),
    ("config.edit", EffectKind::Mutate),
    ("config.get_tree", EffectKind::Read),
    ("config.index", EffectKind::Read),
    ("config.patches", EffectKind::Read),
    ("config.resolve", EffectKind::Read),
    ("config.source", EffectKind::Read),
    ("config.validate", EffectKind::Read),
    ("conflict.check", EffectKind::Read),
    ("diag.errors", EffectKind::Read),
    ("diag.learn_error", EffectKind::Mutate),
    ("diag.run", EffectKind::Launch),
    ("diag.run_client", EffectKind::Launch),
    ("diag.run_diag", EffectKind::Launch),
    ("diag.status", EffectKind::Read),
    ("diag.stop", EffectKind::Launch),
    ("font.build", EffectKind::Mutate),
    ("formats.list", EffectKind::Read),
    ("game.diff", EffectKind::Read),
    ("game.snapshot", EffectKind::IndexWrite),
    ("host.changelog", EffectKind::Read),
    ("host.doctor", EffectKind::Read),
    ("host.update", EffectKind::Destructive),
    ("host.update_check", EffectKind::Network),
    ("image.convert", EffectKind::Mutate),
    ("imageset.build", EffectKind::Mutate),
    ("layout.check", EffectKind::Read),
    ("layout.render", EffectKind::Mutate),
    ("pbo.unpack", EffectKind::Mutate),
    ("project.dependencies", EffectKind::Read),
    ("project.edit_dependencies", EffectKind::Mutate),
    ("project.scaffold", EffectKind::Mutate),
    ("publish.cancel", EffectKind::Publish),
    ("publish.details", EffectKind::Read),
    ("publish.items", EffectKind::Network),
    ("publish.prepare", EffectKind::Mutate),
    ("publish.save_details", EffectKind::Mutate),
    ("publish.status", EffectKind::Read),
    ("publish.upload", EffectKind::Publish),
    ("script.check", EffectKind::IndexWrite),
    ("script.complete", EffectKind::Read),
    ("script.definition", EffectKind::Read),
    ("script.diagnostics", EffectKind::Read),
    ("script.find_class", EffectKind::Read),
    ("script.hover", EffectKind::Read),
    ("script.index", EffectKind::Read),
    ("script.index_status", EffectKind::Read),
    ("script.lint", EffectKind::IndexWrite),
    ("script.outline", EffectKind::Read),
    ("script.source", EffectKind::Read),
    ("search.index", EffectKind::IndexWrite),
    ("search.query", EffectKind::Read),
    ("sign.check", EffectKind::Read),
    ("sign.keygen", EffectKind::Mutate),
    ("text.edit", EffectKind::Mutate),
    ("vfs.list", EffectKind::Read),
    ("vfs.read", EffectKind::Read),
    ("workshop.details", EffectKind::Network),
    ("workspace.undo", EffectKind::Mutate),
];

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

#[test]
fn publish_status_answers_null_when_no_upload_runs() {
    let outcome = dispatch(&common::core(), "publish.status", json!({}));
    let Outcome::Data(status) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    assert_eq!(status, Value::Null);
}

#[test]
fn publish_cancel_says_no_upload_ran() {
    let outcome = dispatch(&common::core(), "publish.cancel", json!({}));
    let Outcome::Data(answer) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    assert_eq!(answer, json!({ "cancelled": false }));
}

#[test]
fn registers_every_frontend_operation_once_with_its_effect_class() {
    let core = common::core();
    let mut actual: Vec<(&str, EffectKind)> = core
        .commands()
        .map(|c| (c.spec().id, c.spec().effect))
        .collect();
    actual.sort_by_key(|(id, _)| *id);
    assert_eq!(actual, EXPECTED.to_vec());

    // Every command carries a summary and an object args schema.
    for cmd in core.commands() {
        assert!(
            !cmd.spec().summary.is_empty(),
            "{} has no summary",
            cmd.spec().id
        );
        assert_eq!(
            cmd.args_schema()["type"],
            "object",
            "{} args schema must be an object schema",
            cmd.spec().id
        );
        // MCP clients refuse a tool whose input schema combines at the top level.
        for combinator in ["oneOf", "anyOf", "allOf"] {
            assert!(
                cmd.args_schema().get(combinator).is_none(),
                "{} args schema has a top-level {combinator}",
                cmd.spec().id
            );
        }
    }
}

#[test]
fn one_session_shares_the_script_index_between_its_cores() {
    let workspace = tmp("session-ws");
    write(&workspace, "Scripts/4_World/A.c", "class A { };\n");
    let session = registry::Session::new();

    let loaded = dispatch(
        &common::core_with(&session),
        "script.index",
        json!({ "root": workspace, "game": tmp("session-no-game") }),
    );
    let Outcome::Data(report) = loaded else {
        panic!("expected Data, got {loaded:?}");
    };
    assert_eq!(report["files"], 1, "{report}");

    let status = dispatch(
        &common::core_with(&session),
        "script.index_status",
        json!({}),
    );
    let Outcome::Data(status) = status else {
        panic!("expected Data, got {status:?}");
    };
    assert_eq!(status["state"], "ready");
    assert_eq!(status["files"], 1);

    let other = dispatch(
        &common::core_with(&registry::Session::new()),
        "script.index_status",
        json!({}),
    );
    let Outcome::Data(other) = other else {
        panic!("expected Data, got {other:?}");
    };
    assert_eq!(other["state"], "idle", "another session shares nothing");
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn lookup_is_by_namespaced_id_and_unknown_ids_are_not_found() {
    let core = common::core();
    for (id, _) in EXPECTED {
        assert!(core.command(id).is_some(), "{id} not registered");
    }
    let err = core
        .dispatch(Actor::CliSession, "nope.nope", json!({}))
        .unwrap_err();
    assert!(matches!(err, CommandError::NotFound(_)));
}

#[test]
fn agents_may_run_read_commands_but_are_denied_mutations_and_launches() {
    let core = common::core();
    let dir = tmp("gate");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let agent = Actor::Agent {
        id: 1,
        grants: Grants::NONE,
    };

    // Read commands are permitted without a grant.
    assert!(core
        .dispatch(agent, "config.validate", json!({ "path": dir }))
        .is_ok());

    // Mutations and launches need an explicit grant the agent does not have.
    for id in [
        "project.scaffold",
        "config.binarize",
        "build.run",
        "diag.run",
    ] {
        let Err(err) = core.dispatch(agent, id, json!({})) else {
            panic!("agent must not reach {id}")
        };
        assert!(
            matches!(err, CommandError::PermissionDenied(_)),
            "{id} returned {err:?}"
        );
    }

    // A CLI session (a human at the terminal) may run everything.
    assert!(core
        .dispatch(
            Actor::CliSession,
            "project.scaffold",
            json!({ "name": "Other", "dir": tmp("gate2") })
        )
        .is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_granted_agent_reaches_the_mutation_it_was_granted() {
    let core = common::core();
    let dir = tmp("grant");
    let granted = Actor::Agent {
        id: 1,
        grants: Grants::NONE,
    }
    .with_grant(EffectKind::Mutate);

    let out = core.dispatch(
        granted,
        "project.scaffold",
        json!({ "name": "Granted", "dir": dir }),
    );
    assert!(matches!(out, Ok(Outcome::Changed { .. })), "{out:?}");
    assert!(dir.join("config.cpp").is_file());

    // The grant is per effect class: it does not unlock a Launch command.
    let err = core
        .dispatch(granted, "diag.run", json!({}))
        .expect_err("a mutate grant is not a launch grant");
    assert!(matches!(err, CommandError::PermissionDenied(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_retail_launch_of_an_unsigned_build_is_refused_before_starting() {
    let mod_dir = tmp("unsigned-launch").join("@Mod");
    std::fs::create_dir_all(mod_dir.join("addons")).unwrap();
    std::fs::write(mod_dir.join("addons/Mod.pbo"), b"").unwrap();

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({ "mod_dir": mod_dir, "server_dir": mod_dir.join("no-such-server") }),
        )
        .expect_err("an unsigned build must not launch");
    let message = match err {
        CommandError::Failed(message) => message,
        other => panic!("expected a failure, got {other:?}"),
    };
    assert!(
        message.contains("Mod.pbo") && message.contains("Settings › Signing"),
        "{message}"
    );
}

#[test]
fn invalid_arguments_are_reported_not_panicked() {
    let core = common::core();
    for id in [
        "config.validate",
        "script.diagnostics",
        "vfs.read",
        "vfs.list",
    ] {
        let err = core.dispatch(Actor::CliSession, id, json!({})).unwrap_err();
        assert!(
            matches!(err, CommandError::InvalidArguments(_)),
            "{id} accepted empty args: {err:?}"
        );
    }
    // A path that does not exist is an operation failure, not bad arguments.
    let err = core
        .dispatch(
            Actor::CliSession,
            "vfs.read",
            json!({ "path": "/no/such/file.c" }),
        )
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
}

#[test]
fn an_unusable_verdict_pattern_is_refused_by_name() {
    let core = common::core();
    let cases = [
        (
            "diag.run",
            json!({ "mod_dir": "/nonexistent/@Mod", "expect_lines": ["re:[unclosed"] }),
        ),
        (
            "diag.run_diag",
            json!({
                "mod_dir": "/nonexistent/@Mod",
                "source_root": "/nonexistent",
                "fail_lines": ["re:[unclosed"],
            }),
        ),
        (
            "diag.run_client",
            json!({ "mods": [], "allow_errors": ["re:[unclosed"] }),
        ),
    ];
    for (id, args) in cases {
        let err = core.dispatch(Actor::CliSession, id, args).unwrap_err();
        assert!(
            matches!(&err, CommandError::InvalidArguments(message) if message.contains("re:[unclosed")),
            "{id}: {err:?}"
        );
    }
}

#[test]
fn vfs_read_returns_the_file_text_as_a_string_payload() {
    let dir = tmp("read");
    let file = write(&dir, "x.txt", "hello world");
    let out = dispatch(&common::core(), "vfs.read", json!({ "path": file }));
    match out {
        Outcome::Data(v) => assert_eq!(v, json!("hello world")),
        other => panic!("expected Data, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_validate_returns_the_shared_diagnostic_outcome() {
    let dir = tmp("cfg");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let cfg = dir.join("config.cpp");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("Scripts/4_World", "Scripts/3_Game")).unwrap();

    let out = dispatch(&common::core(), "config.validate", json!({ "path": dir }));
    match out {
        Outcome::Diagnostics { count, diagnostics } => {
            assert_eq!(count, diagnostics.len());
            assert!(
                diagnostics.iter().any(|d| d.code == "MF-C310"),
                "expected MF-C310, got {:?}",
                diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
            );
        }
        other => panic!("expected Diagnostics, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_diagnostics_reports_the_code_and_the_host_path() {
    let dir = tmp("lint");
    let file = write(&dir, "Scripts/3_Game/Broken.c", "}");
    let out = dispatch(
        &common::core(),
        "script.diagnostics",
        json!({ "path": file }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    let findings = v.as_array().expect("findings array");
    assert_eq!(findings[0]["code"], "MF-E1001");
    assert!(findings[0]["path"].as_str().unwrap().ends_with("Broken.c"));
    assert!(findings[0]["severity"].is_string());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_diagnostics_lints_unsaved_text_without_touching_disk() {
    let dir = tmp("lint-text");
    let path = dir.join("Scripts/4_World/x.c");
    assert!(!path.exists());
    let out = dispatch(
        &common::core(),
        "script.diagnostics",
        json!({ "path": &path, "text": "class A { @ % }" }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    let findings = v.as_array().expect("findings array");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["code"], "MF-E1001");
    assert_eq!(findings[0]["path"], json!(&path));
    assert!(!path.exists(), "text linting must not touch the disk");
}

#[test]
fn script_diagnostics_is_quiet() {
    assert!(common::core()
        .command("script.diagnostics")
        .unwrap()
        .quiet());
}

#[test]
fn build_run_packs_and_reports_the_staged_addon() {
    let dir = tmp("build");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let out = dispatch(
        &common::core(),
        "build.run",
        json!({ "mod_root": dir, "name": "MyMod", "profile": "dev" }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(v["name"], "MyMod");
    assert_eq!(v["addons"], json!(["MyMod.pbo"]));
    assert!(Path::new(v["output_dir"].as_str().unwrap())
        .join("addons/MyMod.pbo")
        .is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_contents_lists_each_file_with_its_pack_decision() {
    let dir = tmp("build-contents");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let out = dispatch(
        &common::core(),
        "build.contents",
        json!({ "mod_root": dir, "name": "MyMod", "profile": "dev" }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    let config = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "config.cpp")
        .expect("config.cpp is listed");
    assert_eq!(
        config["decision"],
        json!({ "included": true, "reason": "GameFile" })
    );
    assert_eq!(v["warnings"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_contents_follows_binarize_config() {
    let dir = tmp("build-contents-binarize");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    write(&dir, "sub/config.cpp", "class CfgPatches {}");
    let listing = |binarize_config: bool| {
        let out = dispatch(
            &common::core(),
            "build.contents",
            json!({
                "mod_root": dir,
                "name": "MyMod",
                "profile": "dev",
                "binarize_config": binarize_config,
            }),
        );
        let Outcome::Data(v) = out else {
            panic!("expected Data, got {out:?}");
        };
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                (
                    file["path"].as_str().unwrap().to_owned(),
                    file["why"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };

    let binarized = listing(true);
    assert!(
        binarized.contains(&(
            "config.cpp".to_owned(),
            "packed as config.bin (--binarize)".to_owned()
        )),
        "{binarized:?}"
    );
    assert!(
        binarized.contains(&("sub/config.cpp".to_owned(), "game file".to_owned())),
        "{binarized:?}"
    );

    let plain = listing(false);
    assert!(
        plain.contains(&("config.cpp".to_owned(), "game file".to_owned())),
        "{plain:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_contents_fills_binarize_from_saved_settings() {
    let dir = tmp("build-contents-saved-binarize");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let db = tmp("build-contents-saved-binarize-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&dir)),
            PREFS_PROJECT_KEY,
            &json!({"build.packing.binarize": {"Bool": true}}),
        )
        .unwrap();
    let session = registry::Session::new().with_db(db.clone());

    let out = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "build.contents",
            json!({ "mod_root": dir, "name": "MyMod", "profile": "dev" }),
        )
        .unwrap_or_else(|e| panic!("build.contents failed: {e}"));
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };

    let config = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "config.cpp")
        .expect("config.cpp is listed");
    assert_eq!(config["why"], "packed as config.bin (--binarize)");
    assert!(
        v["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("binarize_config")),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

/// The wall clock is the only thing that can differ between two builds of an
/// unchanged tree, so a fixed entry time is what makes them compare equal.
#[test]
fn two_builds_of_an_unchanged_tree_match() {
    let dir = tmp("build-twice");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let build = || {
        let Outcome::Data(report) = dispatch(
            &common::core(),
            "build.run",
            json!({ "mod_root": dir, "name": "MyMod", "profile": "dev" }),
        ) else {
            panic!("expected Data");
        };
        report
    };

    let first = build();
    let second = build();
    assert_eq!(first["pbo_sha256"], second["pbo_sha256"], "{first}");

    let pbo = Path::new(second["output_dir"].as_str().unwrap()).join("addons/MyMod.pbo");
    let (header, _) = modfoundry_pbo::read_header_file(&pbo).unwrap();
    let stamps: Vec<u32> = header.entries.iter().map(|e| e.timestamp).collect();
    assert!(
        stamps.iter().all(|t| *t == 0),
        "entries carry a fixed time: {stamps:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_authority_is_refused_before_anything_is_written() {
    let core = common::core();
    let keys = tmp("keygen-bad-authority");
    for authority in ["", "  ", "a/b"] {
        let err = core
            .dispatch(
                Actor::CliSession,
                "sign.keygen",
                json!({ "authority": authority, "dir": keys }),
            )
            .expect_err("a bad authority must be refused");
        assert!(
            err.to_string().contains("authority"),
            "{authority:?}: {err}"
        );
    }
    assert!(
        !keys.join(".biprivatekey").exists(),
        "an empty authority wrote a hidden private key"
    );
    assert!(
        !keys.join("a").exists(),
        "an authority with a path separator wrote a key"
    );

    let root = tmp("build-bad-authority");
    modfoundry_commands::scaffold(&root, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    for authority in ["", "  ", "a/b"] {
        let err = core
            .dispatch(
                Actor::CliSession,
                "build.run",
                json!({ "mod_root": root, "name": "MyMod", "authority": authority }),
            )
            .expect_err("a bad build authority must be refused");
        assert!(
            err.to_string().contains("authority"),
            "{authority:?}: {err}"
        );
    }
    assert!(
        !root.join("builds").exists(),
        "a refused build staged its output"
    );
}

#[test]
fn diag_errors_maps_logged_errors_back_to_source_files() {
    let logs = tmp("logs");
    write(
        &logs,
        "DayZServer_x64.RPT",
        " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'x'\r\n",
    );
    let src = tmp("src");
    write(&src, "scripts/3_game/broken.c", "class Broken {}\n");

    let out = dispatch(
        &common::core(),
        "diag.errors",
        json!({ "dir": logs, "source_root": src }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    let errs = v.as_array().expect("errors array");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0]["line"], 5);
    assert!(errs[0]["file"].as_str().unwrap().ends_with("broken.c"));
    let _ = std::fs::remove_dir_all(&logs);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn formats_list_and_host_doctor_report_their_payloads() {
    let core = common::core();
    let Outcome::Data(formats) = dispatch(&core, "formats.list", json!({})) else {
        panic!("expected Data");
    };
    let names: Vec<&str> = formats
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(names.contains(&"Pbo"), "got {names:?}");
    assert!(names.contains(&"EnforceScript"), "got {names:?}");

    let Outcome::Data(doctor) = dispatch(&core, "host.doctor", json!({})) else {
        panic!("expected Data");
    };
    assert!(doctor["platform"].is_string());
    assert_eq!(doctor["steam_libraries"], json!([]));
    assert_eq!(doctor.get("dayz_game"), Some(&Value::Null));
    assert_eq!(doctor.get("dayz_tools"), Some(&Value::Null));
    assert!(doctor["gpu"]["driver"].is_boolean());
    assert!(doctor["gpu"]["missing"].is_array());
    assert!(doctor["gpu"].get("device_ok").is_some());
}

#[cfg(unix)]
#[test]
fn doctor_runs_the_helper_check() {
    use modfoundry_commands::registry::gpu_report_with;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let script = |name: &str, body: &str| {
        let path = tmp(name).with_extension("sh");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let limit = Duration::from_secs(10);

    let ready = script(
        "gpu-ready",
        "#!/bin/sh\n[ \"$1\" = --check ] || exit 3\nexit 0\n",
    );
    let report = gpu_report_with(Some(ready.clone()), None, "", limit);
    assert_eq!(report.device_ok, Some(true), "{:?}", report.device_error);
    assert_eq!(report.device_error, None);
    let _ = std::fs::remove_file(&ready);

    let broken = script(
        "gpu-broken",
        "#!/bin/sh\n[ \"$1\" = --check ] || exit 3\necho 'error: no CUDA device' >&2\nexit 1\n",
    );
    let report = gpu_report_with(Some(broken.clone()), None, "", limit);
    assert_eq!(report.device_ok, Some(false));
    assert!(
        report
            .device_error
            .as_deref()
            .unwrap_or_default()
            .contains("no CUDA device"),
        "{:?}",
        report.device_error
    );
    let _ = std::fs::remove_file(&broken);

    let report = gpu_report_with(None, None, "", limit);
    assert_eq!(report.device_ok, None);
    assert_eq!(report.device_error, None);
}

#[cfg(unix)]
#[test]
fn diag_run_launches_the_server_and_reports_the_captured_logs() {
    // A fake DayZServer install (records argv, writes an RPT, exits) so the
    // launch command's happy path is covered without a real install.
    let install = tmp("srv");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(
        &exe,
        "#!/bin/sh\nprof=''; for a in \"$@\"; do case \"$a\" in -profiles=*) prof=\"${a#-profiles=}\";; esac; done\nprintf '15:00:00.000  SCRIPT    (E): boom\\r\\n' > \"$prof/x.RPT\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mod_dir = install.join("@mymod");
    std::fs::create_dir_all(&mod_dir).unwrap();

    let out = dispatch(
        &common::core(),
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": install.join("profiles"),
            "timeout_secs": 10
        }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    assert_eq!(v["report"]["exit_code"], 0);
    assert!(v["report"]["logs"][0].is_string());
    assert!(Path::new(v["profiles_dir"].as_str().unwrap()).is_dir());
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0]["message"].as_str().unwrap().contains("boom"));
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn run_client_connect_arguments_are_validated_before_launching() {
    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "diag.run_client",
            json!({ "mods": ["/tmp/@x"], "connect": "not-a-target" }),
        )
        .unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidArguments(_)),
        "expected InvalidArguments, got {err:?}"
    );
}

/// A project with each `(folder, name)` scaffolded under it and `sets` written as its
/// `server/launch-sets.json`.
fn project_with_mods(label: &str, mods: &[(&str, &str)], sets: Value) -> std::path::PathBuf {
    let project = tmp(label);
    for (folder, name) in mods {
        modfoundry_commands::scaffold(
            &project.join(folder),
            &modfoundry_commands::ScaffoldOptions::new(*name),
        )
        .unwrap();
    }
    write(&project, "server/launch-sets.json", &sets.to_string());
    project
}

/// A project whose set `session` names two local mods — one under a folder whose name differs
/// from the mod's — a workshop item and a built folder to skip, plus a local server mod.
fn project_with_a_set(label: &str) -> std::path::PathBuf {
    project_with_mods(
        label,
        &[
            ("mods/Core", "Core"),
            ("mods/hud", "FSHud"),
            ("mods/server", "FS_Server"),
        ],
        json!({"sets": {"session": {
            "mods": [
                {"workshop": "1559212036"},
                {"local": "mods/Core"},
                {"local": "mods/hud", "name": "FSHud"},
                {"folder": "/built/@Hud"},
            ],
            "server_mods": [{"local": "mods/server", "name": "FS_Server"}],
        }}}),
    )
}

#[test]
fn a_set_builds_its_local_mods_and_server_mods_in_order() {
    let project = project_with_a_set("set-build");

    let out = dispatch(
        &common::core(),
        "build.run",
        json!({ "set": "session", "source_root": project }),
    );

    let Outcome::Data(reports) = &out else {
        panic!("expected Data, got {out:?}");
    };
    let names: Vec<&str> = reports
        .as_array()
        .expect("one report per local mod")
        .iter()
        .map(|report| report["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Core", "FSHud", "FS_Server"]);
    for (folder, name) in [
        ("mods/Core", "Core"),
        ("mods/hud", "FSHud"),
        ("mods/server", "FS_Server"),
    ] {
        let built = project
            .join(folder)
            .join("builds/default")
            .join(format!("@{name}/addons/{name}.pbo"));
        assert!(built.is_file(), "{} is missing", built.display());
    }
    let _ = std::fs::remove_dir_all(&project);
}

#[cfg(unix)]
#[test]
fn a_set_built_from_a_relative_root_names_the_folder_in_each_report() {
    let project = project_with_a_set("set-relative-root");
    let mut relative = std::path::PathBuf::new();
    for _ in std::env::current_dir().unwrap().components().skip(1) {
        relative.push("..");
    }
    relative.push(project.strip_prefix("/").unwrap());

    let out = dispatch(
        &common::core(),
        "build.run",
        json!({ "set": "session", "source_root": relative }),
    );

    let Outcome::Data(reports) = &out else {
        panic!("expected Data, got {out:?}");
    };
    let canonical = project.canonicalize().unwrap();
    for report in reports.as_array().expect("one report per local mod") {
        assert_eq!(report["resolved_root"], json!(canonical), "{report}");
    }
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_set_stops_at_the_first_failing_mod_and_keeps_the_builds_before_it() {
    let project = project_with_mods(
        "set-fail",
        &[("mods/Core", "Core"), ("mods/hud", "FSHud")],
        json!({"sets": {"session": {"mods": [
            {"local": "mods/Core"},
            {"local": "mods/hud", "name": "Mistyped"},
        ]}}}),
    );

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "build.run",
            json!({ "set": "session", "source_root": project }),
        )
        .expect_err("the second mod builds under a name its config does not bind");

    let message = err.to_string();
    assert!(message.contains("mod Mistyped"), "{message}");
    let kept = project.join("mods/Core/builds/default/@Core/addons/Core.pbo");
    assert!(kept.is_file(), "{} is missing", kept.display());
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_set_with_a_bad_local_mod_name_is_refused_before_any_build() {
    let project = project_with_mods(
        "set-bad-name",
        &[("mods/Core", "Core")],
        json!({"sets": {"session": {"mods": [{"local": "mods/Core", "name": "@Core"}]}}}),
    );

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "build.run",
            json!({ "set": "session", "source_root": project }),
        )
        .expect_err("a name with a leading @ cannot be a link name");

    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
    assert!(
        err.to_string().contains("must be a bare folder name"),
        "{err}"
    );
    assert!(
        !project.join("mods/Core/builds").exists(),
        "the build started before the set was checked"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_set_with_no_local_mod_is_refused_not_built_as_nothing() {
    let sets = [
        json!({"sets": {"session": {"mods": [
            {"workshop": "1559212036"},
            {"folder": "/built/@Hud"},
        ]}}}),
        json!({"sets": {"session": {"mods": []}}}),
    ];

    for (case, sets) in sets.into_iter().enumerate() {
        for preview in [false, true] {
            let project =
                project_with_mods(&format!("set-empty-{case}-{preview}"), &[], sets.clone());

            let err = common::core()
                .dispatch(
                    Actor::CliSession,
                    "build.run",
                    json!({ "set": "session", "source_root": project, "preview": preview }),
                )
                .expect_err("a set with nothing local has nothing to build");

            assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
            let message = err.to_string();
            assert!(message.contains("launch set \"session\""), "{message}");
            assert!(message.contains("no local mod"), "{message}");
            assert!(
                !project.join("builds").exists(),
                "a build folder was made under the project"
            );
            let _ = std::fs::remove_dir_all(&project);
        }
    }
}

#[test]
fn a_set_build_treats_null_flags_as_not_given() {
    let project = project_with_a_set("set-null-flags");

    let out = dispatch(
        &common::core(),
        "build.run",
        json!({
            "set": "session",
            "source_root": project,
            "output": null,
            "name": null,
            "profile": null,
        }),
    );

    let Outcome::Data(reports) = &out else {
        panic!("expected Data, got {out:?}");
    };
    let names: Vec<&str> = reports
        .as_array()
        .expect("one report per local mod")
        .iter()
        .map(|report| report["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Core", "FSHud", "FS_Server"]);
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn build_run_refuses_a_target_it_cannot_build() {
    let project = project_with_a_set("set-refused");
    let cases: Vec<(Value, &[&str])> = vec![
        (json!({}), &["mod root, or a launch set"]),
        (
            json!({ "mod_root": project.join("mods/Core"), "set": "session" }),
            &["not both"],
        ),
        (
            json!({ "mod_root": project.join("mods/Core"), "source_root": project }),
            &["not both"],
        ),
        (
            json!({ "set": "nope", "source_root": project }),
            &["no launch set \"nope\"", "it has: session"],
        ),
        (
            json!({ "set": "session", "source_root": project.join("elsewhere") }),
            &["launch-sets.json is missing"],
        ),
        (
            json!({ "set": "session", "source_root": project, "output": project.join("out") }),
            &["takes no output"],
        ),
        (
            json!({ "set": "session", "source_root": project, "name": "Renamed" }),
            &["takes no name"],
        ),
        (
            json!({ "set": "session", "source_root": project, "profile": "renamed" }),
            &["takes no profile"],
        ),
    ];
    for (args, expected) in cases {
        let err = common::core()
            .dispatch(Actor::CliSession, "build.run", args.clone())
            .expect_err(&args.to_string());

        assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
        for fragment in expected {
            assert!(err.to_string().contains(fragment), "{args}: {err}");
        }
    }
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_set_builds_under_its_profile_and_fills_each_mods_saved_settings() {
    let project = project_with_mods(
        "set-profile",
        &[("mods/Core", "Core")],
        json!({"sets": {"session": {"mods": [{"local": "mods/Core"}], "profile": "dev"}}}),
    );
    let core = project.join("mods/Core");
    let db = tmp("set-profile-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&core)),
            PREFS_PROJECT_KEY,
            &json!({
                "build.packing.profile": {"Text": "release"},
                "build.packing.prefix": {"Text": "Core"},
            }),
        )
        .unwrap();
    let session = registry::Session::new().with_db(db);

    let out = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "build.run",
            json!({ "set": "session", "source_root": project }),
        )
        .unwrap_or_else(|e| panic!("build.run failed: {e}"));

    let Outcome::Data(reports) = out else {
        panic!("expected Data, got {out:?}");
    };
    let built = core.join("builds/dev/@Core/addons/Core.pbo");
    assert!(built.is_file(), "{} is missing: {reports}", built.display());
    assert!(
        !core.join("builds/release").exists(),
        "the saved profile won: {reports}"
    );
    assert!(
        reports[0]["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("prefix")),
        "the mod's saved settings did not fill: {reports}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn a_set_preview_returns_one_change_set_per_mod_and_writes_nothing() {
    let project = project_with_a_set("set-preview");

    let out = common::core()
        .dispatch(
            Actor::CliSession,
            "build.run",
            json!({ "set": "session", "source_root": project, "preview": true }),
        )
        .unwrap_or_else(|e| panic!("build.run failed: {e}"));

    let Outcome::Preview(plan) = out else {
        panic!("expected Preview, got {out:?}");
    };
    let entries = plan.as_array().expect("one entry per local mod");
    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Core", "FSHud", "FS_Server"]);
    for entry in entries {
        assert!(
            !entry["plan"]["changes"].as_array().unwrap().is_empty(),
            "{entry}"
        );
    }
    assert!(
        !project.join("mods/Core/builds").exists(),
        "a preview built"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn config_binarize_writes_rap_bytes_and_derapify_returns_text() {
    let dir = tmp("config-bin");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let bin = dir.join("out.bin");
    let out = dispatch(
        &common::core(),
        "config.binarize",
        json!({ "path": dir.join("config.cpp"), "out": bin }),
    );
    let Outcome::Changed { summary, .. } = &out else {
        panic!("expected a recorded change, got {out:?}");
    };
    assert!(
        summary
            .description
            .contains(&bin.to_string_lossy().to_string()),
        "the written path is not reported: {}",
        summary.description
    );
    assert_eq!(&std::fs::read(&bin).unwrap()[..4], b"\0raP");

    let out = dispatch(&common::core(), "config.derapify", json!({ "path": bin }));
    match out {
        Outcome::Data(v) => {
            let text = v.as_str().expect("derapified text");
            assert!(text.contains("class CfgPatches"), "got: {text}");
        }
        other => panic!("expected Data, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_host_report_without_gpu_lines_still_reads() {
    let report: modfoundry_commands::HostReport = serde_json::from_value(json!({
        "platform": "linux",
        "steam_libraries": [],
        "dayz_game": null,
        "dayz_tools": null,
    }))
    .unwrap();
    assert!(report.gpu.missing.is_empty());
}

#[test]
fn every_command_another_command_runs_is_registered() {
    let core = common::core();
    for cmd in core.commands() {
        for inner in cmd.runs() {
            assert!(
                core.command(inner).is_some(),
                "{} runs {inner}, which is not registered",
                cmd.spec().id
            );
            // The broker applies the rules of one level of inner commands only.
            assert!(
                core.command(inner).unwrap().runs().is_empty(),
                "{inner}, which {} runs, runs other commands itself",
                cmd.spec().id
            );
        }
    }
}
