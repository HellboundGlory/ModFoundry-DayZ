//! Agent calls fill the arguments they leave out from the app's saved settings.

use std::path::{Path, PathBuf};

use crate::common::{self, tmp};
use modfoundry_commands::project_settings::{project_id, PREFS_PROJECT_KEY};
use modfoundry_commands::registry::Session;
use modfoundry_core::{Actor, CommandError, Core, Outcome};
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::{json, Value};

/// A database whose app-saved project settings for the mod at `mod_root` are `project`.
fn saved_db(mod_root: &Path, project: Value) -> PathBuf {
    let db = tmp("settings-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = Store::open(&db).unwrap();
    store
        .set_setting(
            &Scope::Workspace(project_id(mod_root)),
            PREFS_PROJECT_KEY,
            &project,
        )
        .unwrap();
    db
}

/// A database whose saved project signs with a named key whose private key file is absent; returns both.
fn saved_missing_key(mod_root: &Path) -> (PathBuf, PathBuf) {
    let private_key = tmp("settings-key-dir").join("keys/Hellbound.biprivatekey");
    let db = saved_db(
        mod_root,
        json!({"build.signing.key": {"Key": {"Named": {
            "authority": "Hellbound",
            "private_key": private_key,
        }}}}),
    );
    (db, private_key)
}

/// A database whose saved project enables a test mod whose folder is absent; returns both.
fn saved_gone_test_mod(mod_root: &Path) -> (PathBuf, PathBuf) {
    let gone = tmp("settings-diag-gone").join("@Gone");
    let db = saved_db(
        mod_root,
        json!({"launch.mods.test": {"Mods": [{
            "id": "gone",
            "name": "Gone",
            "role": "Both",
            "enabled": true,
            "source": {"Folder": {"path": gone}},
        }]}}),
    );
    (db, gone)
}

fn plan(session: &Session, mut args: Value) -> Result<Value, CommandError> {
    args["preview"] = json!(true);
    match common::core_with(session).dispatch(Actor::CliSession, "build.run", args)? {
        Outcome::Preview(change_set) => Ok(change_set),
        other => panic!("expected Preview, got {other:?}"),
    }
}

/// A new mod named `MyMod` at `root`.
fn scaffold_mod(root: &Path) {
    modfoundry_commands::scaffold(root, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
}

fn build(core: &Core, args: Value) -> Value {
    let Outcome::Data(report) = core
        .dispatch(Actor::CliSession, "build.run", args)
        .unwrap_or_else(|e| panic!("build.run failed: {e}"))
    else {
        panic!("expected Data");
    };
    report
}

#[test]
fn a_build_reports_the_saved_settings_it_filled_and_nothing_for_an_unsaved_project() {
    let mod_root = tmp("settings-used");
    scaffold_mod(&mod_root);
    let args = json!({ "mod_root": mod_root, "name": "MyMod", "profile": "dev" });
    let unsaved = tmp("settings-unsaved").join("agents.db");
    std::fs::create_dir_all(unsaved.parent().unwrap()).unwrap();
    Store::open(&unsaved).unwrap();

    let session = Session::new().with_db(unsaved);
    assert_eq!(
        build(&common::core_with(&session), args.clone())["settings_used"],
        json!([])
    );

    let saved = saved_db(
        &mod_root,
        json!({"build.packing.prefix": {"Text": "MyMod"}}),
    );
    let app = Session::new()
        .with_db(saved.clone())
        .without_settings_fill();
    assert_eq!(
        build(&common::core_with(&app), args.clone())["settings_used"],
        json!([])
    );
    let session = Session::new().with_db(saved);
    let used = build(&common::core_with(&session), args)["settings_used"].clone();
    assert!(
        used.as_array().unwrap().contains(&json!("prefix")),
        "{used}"
    );
    assert!(
        !used.as_array().unwrap().contains(&json!("profile")),
        "{used}"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}

/// A fake DayZServer install whose executable exits at once.
#[cfg(unix)]
fn fake_exiting_server(install: &Path) {
    std::fs::create_dir_all(install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_run_preview_names_the_saved_settings_it_would_use() {
    let source_root = tmp("settings-preview");
    std::fs::create_dir_all(&source_root).unwrap();
    let mod_dir = tmp("settings-preview-mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let install = tmp("settings-preview-srv");
    fake_exiting_server(&install);
    let saved = saved_db(
        &source_root,
        json!({"launch.window.port": {"Number": 2402}}),
    );
    let session = Session::new().with_db(saved);

    let Outcome::Preview(run_plan) = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "source_root": source_root,
                "server_dir": install,
                "preview": true,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run preview failed: {e}"))
    else {
        panic!("expected Preview");
    };
    assert!(
        run_plan["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("port")),
        "the run preview does not name the saved port: {run_plan}"
    );

    let mod_root = tmp("settings-preview-build");
    let mut options = modfoundry_commands::ScaffoldOptions::new("MyMod");
    options.prefix = Some("JM/CF".to_owned());
    modfoundry_commands::scaffold(&mod_root, &options).unwrap();
    let saved = saved_db(
        &mod_root,
        json!({"build.packing.prefix": {"Text": "JM/CF"}}),
    );
    let session = Session::new().with_db(saved);
    let build_plan = plan(
        &session,
        json!({ "mod_root": mod_root, "name": "MyMod", "profile": "dev" }),
    )
    .unwrap();
    assert!(
        build_plan["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("prefix")),
        "the build preview does not name the saved prefix: {build_plan}"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[cfg(unix)]
#[test]
fn a_detached_run_preview_names_only_the_settings_a_detached_run_fills() {
    let source_root = tmp("settings-preview-detached");
    std::fs::create_dir_all(&source_root).unwrap();
    let mod_dir = tmp("settings-preview-detached-mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let install = tmp("settings-preview-detached-srv");
    fake_exiting_server(&install);
    let saved = saved_db(
        &source_root,
        json!({"launch.window.keep_running": {"Bool": true}}),
    );
    let session = Session::new().with_db(saved);

    let Outcome::Preview(run_plan) = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "source_root": source_root,
                "server_dir": install,
                "wait": false,
                "preview": true,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run preview failed: {e}"))
    else {
        panic!("expected Preview");
    };
    let used = run_plan["settings_used"].as_array().unwrap();
    assert!(used.contains(&json!("port")), "{run_plan}");
    assert!(!used.contains(&json!("timeout_secs")), "{run_plan}");

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    let _ = std::fs::remove_dir_all(&install);
}

#[cfg(unix)]
#[test]
fn a_waited_diag_run_keeps_the_saved_run_length() {
    let source_root = tmp("settings-waited");
    std::fs::create_dir_all(&source_root).unwrap();
    let mod_dir = tmp("settings-waited-mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let install = tmp("settings-waited-srv");
    fake_exiting_server(&install);
    let profiles = tmp("settings-waited-profiles");
    let saved = saved_db(
        &source_root,
        json!({
            "launch.window.keep_running": {"Bool": true},
            "launch.window.run_secs": {"Number": 25},
        }),
    );
    let session = Session::new().with_db(saved);

    let Outcome::Data(report) = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_dir,
                "source_root": source_root,
                "server_dir": install,
                "profiles_dir": profiles,
            }),
        )
        .unwrap_or_else(|e| panic!("diag.run failed: {e}"))
    else {
        panic!("expected Data");
    };
    assert!(
        report["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("timeout_secs")),
        "a waited run keeps the saved run length: {}",
        report["settings_used"]
    );
    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(mod_dir.parent().unwrap());
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&profiles);
}

#[test]
fn a_saved_named_key_whose_private_key_is_gone_refuses_the_build() {
    let mod_root = tmp("settings-key");
    scaffold_mod(&mod_root);
    let (saved, private_key) = saved_missing_key(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "build.run",
            json!({ "mod_root": mod_root, "name": "MyMod" }),
        )
        .expect_err("a build with a missing saved key must be refused")
        .to_string();
    assert!(
        err.contains(&format!("signing key {} not found", private_key.display())),
        "{err}"
    );
    assert!(
        !mod_root.join("builds").exists(),
        "a refused build wrote output"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn publish_prepare_refuses_a_saved_named_key_whose_private_key_is_gone() {
    let mod_root = tmp("settings-publish-key");
    scaffold_mod(&mod_root);
    let (saved, private_key) = saved_missing_key(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "publish.prepare",
            json!({ "mod_root": mod_root, "name": "MyMod" }),
        )
        .expect_err("a prepare with a missing saved key must be refused")
        .to_string();
    assert!(
        err.contains(&format!("signing key {} not found", private_key.display())),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn diag_run_diag_refuses_a_saved_test_mod_whose_folder_is_gone() {
    let mod_root = tmp("settings-diag");
    scaffold_mod(&mod_root);
    let (saved, gone) = saved_gone_test_mod(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run_diag",
            json!({
                "mod_dir": mod_root.join("builds/default/@MyMod"),
                "source_root": mod_root,
                "game_dir": tmp("no-game-install"),
            }),
        )
        .expect_err("a saved test mod with no folder must be refused")
        .to_string();
    assert!(
        err.contains(&format!("test mod Gone not found at {}", gone.display())),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn diag_run_diag_keeps_an_explicit_empty_test_mod_list() {
    let mod_root = tmp("settings-diag-explicit");
    scaffold_mod(&mod_root);
    let (saved, _) = saved_gone_test_mod(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run_diag",
            json!({
                "mod_dir": mod_root.join("builds/default/@MyMod"),
                "source_root": mod_root,
                "game_dir": tmp("no-game-install"),
                "test_mods": [],
            }),
        )
        .expect_err("the missing game install stops the launch")
        .to_string();
    assert!(err.contains("DayZDiag_x64.exe not found"), "{err}");
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn build_and_diag_schemas_say_a_left_out_setting_takes_the_saved_value() {
    let core = common::core();
    let build = core.command("build.run").unwrap().args_schema();
    let diag = core.command("diag.run_diag").unwrap().args_schema();
    let description = |schema: &Value, arg: &str| {
        schema["properties"][arg]["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(description(&build, "mod_root").contains("saved"), "{build}");
    assert!(
        description(&diag, "source_root").contains("saved"),
        "{diag}"
    );
}

#[test]
fn run_client_takes_a_set_without_mods() {
    let core = common::core();
    let schema = core.command("diag.run_client").unwrap().args_schema();
    assert!(schema.get("required").is_none(), "{schema}");
    for form in ["oneOf", "anyOf", "allOf"] {
        assert!(schema.get(form).is_none(), "{form}: {schema}");
    }
    let description = |arg: &str| {
        schema["properties"][arg]["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(description("set").contains("source_root"), "{schema}");
    assert!(description("mods").contains("unless set"), "{schema}");
}

#[test]
fn run_client_schema_names_the_runs_own_mod_and_the_test_mod_order() {
    let core = common::core();
    let schema = core.command("diag.run_client").unwrap().args_schema();
    let description = |arg: &str| {
        schema["properties"][arg]["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let mods = description("mods");
    assert!(mods.contains("last one is the run's own mod"), "{schema}");
    assert!(mods.contains("unless set"), "{schema}");
    assert!(description("test_mods").contains("before mods"), "{schema}");
    for property in ["path", "link_name", "role"] {
        assert!(
            schema["properties"]["test_mods"]["items"]["properties"]
                .get(property)
                .is_some(),
            "{property}: {schema}"
        );
    }
}

#[test]
fn a_saved_prefix_fills_the_build_plan_and_an_explicit_prefix_wins() {
    let mod_root = tmp("settings-prefix");
    let mut options = modfoundry_commands::ScaffoldOptions::new("MyMod");
    options.prefix = Some("JM/CF".to_owned());
    modfoundry_commands::scaffold(&mod_root, &options).unwrap();
    let saved = saved_db(
        &mod_root,
        json!({"build.packing.prefix": {"Text": "JM/CF"}}),
    );
    let with_db = Session::new().with_db(saved);
    let bare = json!({ "mod_root": mod_root, "name": "MyMod", "profile": "dev" });
    let given = |prefix: &str| {
        let mut args = bare.clone();
        args["prefix"] = json!(prefix);
        args
    };

    // The plan differs only in naming the settings it filled, which the saved side did.
    let without_settings = |mut plan: Value| {
        plan.as_object_mut().unwrap().remove("settings_used");
        plan
    };
    assert_eq!(
        without_settings(plan(&with_db, bare.clone()).unwrap()),
        without_settings(plan(&Session::new(), given("JM/CF")).unwrap())
    );
    let refused = plan(&with_db, given("MyMod")).unwrap_err().to_string();
    assert!(refused.contains("prefix is \"MyMod\""), "{refused}");
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn prepare_packs_without_comments_when_the_saved_setting_strips_them() {
    let mod_root = tmp("settings-strip-publish");
    scaffold_mod(&mod_root);
    common::write(&mod_root, "Scripts/4_World/a.c", "class A {} // note\n");
    let saved = saved_db(
        &mod_root,
        json!({"build.packing.strip_comments": {"Bool": true}}),
    );
    let session = Session::new().with_db(saved);

    let Outcome::Data(plan) = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "publish.prepare",
            json!({ "mod_root": mod_root, "name": "MyMod", "profile": "pub" }),
        )
        .expect("prepare succeeds")
    else {
        panic!("expected Data");
    };

    assert!(
        plan["settings_used"]
            .as_array()
            .unwrap()
            .contains(&json!("strip_comments")),
        "{}",
        plan["settings_used"]
    );
    let pbo = PathBuf::from(plan["content_dir"].as_str().unwrap()).join("addons/MyMod.pbo");
    let (header, data) = modfoundry_pbo::read_pbo(&std::fs::read(pbo).unwrap()).unwrap();
    let index = header
        .entries
        .iter()
        .position(|entry| entry.name_str() == "Scripts\\4_World\\a.c")
        .expect("the script is packed");
    assert_eq!(data[index], b"class A {}\n");
    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn publish_upload_fills_the_saved_build_settings_like_prepare() {
    let mod_root = tmp("settings-upload-key");
    scaffold_mod(&mod_root);
    let (saved, private_key) = saved_missing_key(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::User,
            "publish.upload",
            json!({ "mod_root": mod_root, "name": "MyMod" }),
        )
        .expect_err("an upload with a missing saved key must be refused before Steam")
        .to_string();
    assert!(
        err.contains(&format!("signing key {} not found", private_key.display())),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}

/// A mod folder with one script declaring `class <name> {}`.
fn dep_mod(dir: &Path, name: &str) -> PathBuf {
    common::write(dir, "Scripts/3_Game/dep.c", &format!("class {name} {{}}\n"));
    dir.to_path_buf()
}

fn dependency_mods_json(entries: &[(&str, &Path, &str, bool)]) -> Value {
    let mods: Vec<Value> = entries
        .iter()
        .map(|(name, path, use_as, enabled)| {
            json!({
                "id": name.to_lowercase(),
                "name": name,
                "role": "Both",
                "enabled": enabled,
                "use_as": use_as,
                "source": {"Folder": {"path": path}},
            })
        })
        .collect();
    json!({"launch.mods.test": {"Mods": mods}})
}

#[test]
fn script_index_fills_dependencies_from_the_saved_settings() {
    let root = tmp("settings-deps-root");
    common::write(&root, "Scripts/3_Game/b.c", "class B { Dep d; }\n");
    let dep = dep_mod(&tmp("settings-deps-dep"), "Dep");
    let test_only = dep_mod(&tmp("settings-deps-test"), "TestOnly");
    let off = dep_mod(&tmp("settings-deps-off"), "Off");
    let game = tmp("settings-deps-game");
    std::fs::create_dir_all(&game).unwrap();
    let indexed = vec![
        ("Dep", dep.as_path(), "Dependency", true),
        ("TestOnly", test_only.as_path(), "TestOnly", true),
        ("Off", off.as_path(), "Dependency", false),
    ];
    let db = saved_db(&root, dependency_mods_json(&indexed));

    let saved = Session::new().with_db(db.clone());
    let app = Session::new().with_db(db).without_settings_fill();
    let cases = [
        (
            "saved db",
            common::core_with(&saved),
            json!({"root": root, "game": game}),
            json!(["dependencies"]),
            false,
        ),
        (
            "given list",
            common::core_with(&saved),
            json!({"root": root, "game": game, "dependencies": []}),
            json!([]),
            true,
        ),
        (
            "app session",
            common::core_with(&app),
            json!({"root": root, "game": game}),
            json!([]),
            true,
        ),
        (
            "no database",
            common::core(),
            json!({"root": root, "game": game}),
            json!([]),
            true,
        ),
    ];
    for (label, core, args, used, unknown_type) in cases {
        let report = match core
            .dispatch(Actor::CliSession, "script.index", args)
            .unwrap()
        {
            Outcome::Data(value) => value,
            other => panic!("{label}: expected Data, got {other:?}"),
        };
        assert_eq!(report["settings_used"], used, "{label}");
        let origins: Vec<&str> = report["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|source| source["origin"].as_str())
            .filter(|origin| origin.starts_with("dependency:"))
            .collect();
        let wanted: &[&str] = if unknown_type { &[] } else { &["dependency:0"] };
        assert_eq!(origins, wanted, "{label}");
        let Outcome::Data(findings) = core
            .dispatch(
                Actor::CliSession,
                "script.diagnostics",
                json!({"path": root}),
            )
            .unwrap()
        else {
            panic!("{label}: expected Data");
        };
        let warned = findings
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "MF-W3002");
        assert_eq!(warned, unknown_type, "{label}: {findings}");
    }

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dep);
    let _ = std::fs::remove_dir_all(&test_only);
    let _ = std::fs::remove_dir_all(&off);
    let _ = std::fs::remove_dir_all(&game);
}

/// A temp game scripts folder with one base class, so `script.check` never reads the real install.
fn check_vanilla(label: &str) -> PathBuf {
    let vanilla = tmp(label);
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    vanilla
}

#[test]
fn script_check_fills_dependencies_from_the_saved_settings() {
    let root = tmp("settings-check-root");
    common::write(
        &root,
        "Scripts/3_Game/use.c",
        "class UseIt\n{\n\tDepThing d;\n}\n",
    );
    let dep = dep_mod(&tmp("settings-check-dep"), "DepThing");
    let vanilla = check_vanilla("settings-check-vanilla");
    let cache = tmp("settings-check-cache");
    let db = saved_db(
        &root,
        dependency_mods_json(&[("DepThing", dep.as_path(), "Dependency", true)]),
    );

    let saved = Session::new().with_db(db.clone());
    let app = Session::new().with_db(db).without_settings_fill();
    let cases = [
        (
            "saved db",
            common::core_with(&saved),
            json!({"root": root, "vanilla": vanilla, "cache": cache}),
            json!(["dependencies"]),
            false,
        ),
        (
            "given list",
            common::core_with(&saved),
            json!({"root": root, "vanilla": vanilla, "cache": cache, "dependencies": []}),
            json!([]),
            true,
        ),
        (
            "app session",
            common::core_with(&app),
            json!({"root": root, "vanilla": vanilla, "cache": cache}),
            json!([]),
            true,
        ),
        (
            "no database",
            common::core(),
            json!({"root": root, "vanilla": vanilla, "cache": cache}),
            json!([]),
            true,
        ),
    ];
    for (label, core, args, used, unknown_type) in cases {
        let Outcome::Data(value) = core
            .dispatch(Actor::CliSession, "script.check", args)
            .unwrap()
        else {
            panic!("{label}: expected Data");
        };
        assert_eq!(value["settings_used"], used, "{label}");
        let messages: Vec<&str> = value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|finding| finding["message"].as_str())
            .collect();
        assert_eq!(
            messages.contains(&"Unknown type 'DepThing'"),
            unknown_type,
            "{label}: {value}"
        );
        assert_eq!(
            value["failed_modules"].as_array().unwrap().is_empty(),
            !unknown_type,
            "{label}: {value}"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dep);
    let _ = std::fs::remove_dir_all(&vanilla);
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn script_check_refuses_a_saved_dependency_the_disk_lacks() {
    let root = tmp("settings-check-gone-root");
    common::write(&root, "Scripts/3_Game/b.c", "class B {}\n");
    let gone = tmp("settings-check-gone");
    let db = saved_db(
        &root,
        dependency_mods_json(&[("Gone", gone.as_path(), "Dependency", true)]),
    );
    let core = common::core_with(&Session::new().with_db(db));
    let vanilla = check_vanilla("settings-check-gone-vanilla");
    let cache = tmp("settings-check-gone-cache");

    let err = core
        .dispatch(
            Actor::CliSession,
            "script.check",
            json!({"root": root, "vanilla": vanilla, "cache": cache}),
        )
        .expect_err("a saved dependency with no folder must be refused");
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(message.contains("Gone"), "{message}");
    assert!(message.contains("dependencies"), "{message}");

    core.dispatch(
        Actor::CliSession,
        "script.check",
        json!({"root": root, "vanilla": vanilla, "cache": cache, "dependencies": []}),
    )
    .expect("a given list skips the saved dependencies");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&vanilla);
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn layout_check_fills_dependencies_from_the_saved_settings() {
    let dir = tmp("settings-layout-deps");
    let root = dir.join("mod");
    common::write(&root, "config.cpp", "class CfgPatches {};\n");
    let layout = common::write(
        &root,
        "layouts/mine.layout",
        "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/t3.edds\"\n}\n",
    );
    let dep = dir.join("Dep");
    common::write(&dep, "gui/t3.edds", "edds");
    let game = dir.join("game");
    common::pack(
        &game.join("dta/gui.pbo"),
        &[("prefix", "gui")],
        &[(
            "layouts/a.layout",
            "TextWidgetClass Label {\n}\n".as_bytes(),
        )],
    );
    let db = saved_db(
        &root,
        dependency_mods_json(&[("Dep", dep.as_path(), "Dependency", true)]),
    );

    let saved = Session::new().with_db(db);
    let cases = [
        (
            "saved db",
            common::core_with(&saved),
            json!({"path": layout, "game": game}),
            json!(["dependencies"]),
            0,
        ),
        (
            "given list",
            common::core_with(&saved),
            json!({"path": layout, "game": game, "dependencies": []}),
            json!([]),
            1,
        ),
        (
            "no database",
            common::core(),
            json!({"path": layout, "game": game}),
            json!([]),
            1,
        ),
    ];
    for (label, core, args, used, unresolved) in cases {
        let Outcome::Data(value) = core
            .dispatch(Actor::CliSession, "layout.check", args)
            .unwrap()
        else {
            panic!("{label}: expected Data");
        };
        assert_eq!(value["settings_used"], used, "{label}");
        let missing = value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|finding| finding["code"] == "MF-L106")
            .count();
        assert_eq!(missing, unresolved, "{label}: {value}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn layout_check_refuses_a_saved_dependency_the_disk_lacks() {
    let dir = tmp("settings-layout-gone");
    let root = dir.join("mod");
    common::write(&root, "config.cpp", "class CfgPatches {};\n");
    let layout = common::write(&root, "layouts/mine.layout", "TextWidgetClass Root {\n}\n");
    let gone = dir.join("Gone");
    let game = dir.join("game");
    let db = saved_db(
        &root,
        dependency_mods_json(&[("Gone", gone.as_path(), "Dependency", true)]),
    );
    let core = common::core_with(&Session::new().with_db(db));

    let err = core
        .dispatch(
            Actor::CliSession,
            "layout.check",
            json!({"path": layout, "game": game}),
        )
        .expect_err("a saved dependency with no folder must be refused");
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(
        message.starts_with("dependency mod Gone not found"),
        "{message}"
    );
    assert!(
        message.ends_with(
            "; remove it with mf deps remove or from the Defaults mods list, or pass dependencies"
        ),
        "{message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_index_refuses_a_saved_dependency_the_disk_lacks() {
    let root = tmp("settings-deps-gone-root");
    common::write(&root, "Scripts/3_Game/b.c", "class B {}\n");
    let gone = tmp("settings-deps-gone");
    let db = saved_db(
        &root,
        dependency_mods_json(&[("Gone", gone.as_path(), "Dependency", true)]),
    );
    let session = Session::new().with_db(db);
    let core = common::core_with(&session);
    let game = tmp("settings-deps-gone-game");

    let err = core
        .dispatch(
            Actor::CliSession,
            "script.index",
            json!({"root": root, "game": game}),
        )
        .expect_err("a saved dependency with no folder must be refused");
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(message.contains("Gone"), "{message}");
    assert!(message.contains("dependencies"), "{message}");

    core.dispatch(
        Actor::CliSession,
        "script.index",
        json!({"root": root, "game": game, "dependencies": []}),
    )
    .expect("a given list skips the saved dependencies");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn diag_run_refuses_a_saved_test_mod_whose_folder_is_gone() {
    let mod_root = tmp("settings-diag-run");
    scaffold_mod(&mod_root);
    let (saved, gone) = saved_gone_test_mod(&mod_root);
    let session = Session::new().with_db(saved);

    let err = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "diag.run",
            json!({
                "mod_dir": mod_root.join("builds/default/@MyMod"),
                "source_root": mod_root,
                "server_dir": tmp("no-server-install"),
            }),
        )
        .expect_err("a saved test mod with no folder must be refused")
        .to_string();
    assert!(
        err.contains(&format!("test mod Gone not found at {}", gone.display())),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&mod_root);
}
