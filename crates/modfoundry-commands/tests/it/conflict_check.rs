//! `conflict.check` through the command registry.

use std::path::Path;

use modfoundry_core::Actor;
use serde_json::{json, Value};

use crate::common::{self, tmp, write};

/// A mod folder with one script at `Scripts/4_World/<name>.c`.
fn mod_with(dir: &Path, name: &str, script: &str) -> std::path::PathBuf {
    let root = dir.join(name);
    write(&root, &format!("Scripts/4_World/{name}.c"), script);
    root
}

fn check(args: Value) -> Result<Value, modfoundry_core::CommandError> {
    let core = common::core();
    match core.dispatch(Actor::CliSession, "conflict.check", args)? {
        modfoundry_core::Outcome::Data(value) => Ok(value),
        other => panic!("expected Data, got {other:?}"),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const SHARED: &str = "class Shared\n{\n}\n";
const MODS_PLAYER_BASE_WITH_SUPER: &str =
    "modded class PlayerBase\n{\n\toverride void OnConnect()\n\t{\n\t\tsuper.OnConnect();\n\t}\n}\n";
const MODS_PLAYER_BASE_WITHOUT_SUPER: &str =
    "modded class PlayerBase\n{\n\toverride void OnConnect()\n\t{\n\t}\n}\n";

#[test]
fn two_folders_that_share_a_class_and_an_override_report_them_in_load_order() {
    let dir = tmp("conflict-check-basic");
    let first = dir.join("A");
    write(&first, "Scripts/4_World/a.c", SHARED);
    write(&first, "Scripts/4_World/b.c", MODS_PLAYER_BASE_WITH_SUPER);
    let second = dir.join("B");
    write(&second, "Scripts/4_World/c.c", SHARED);
    write(
        &second,
        "Scripts/4_World/d.c",
        MODS_PLAYER_BASE_WITHOUT_SUPER,
    );

    let value = check(json!({ "mods": [first, second] })).unwrap();

    assert_eq!(value["load_order"], json!(["A", "B"]), "{value}");
    assert_eq!(value["mods"][0]["name"], "A");
    assert_eq!(value["mods"][0]["scripts"], 2);
    assert_eq!(value["mods"][1]["scripts"], 2);
    let codes: Vec<&str> = value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        ["MF-M101", "MF-M104", "MF-M103", "MF-M102"],
        "{value}"
    );
    assert_eq!(value["warnings"], json!([]), "{value}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_folders_with_one_name_take_their_paths() {
    let dir = tmp("conflict-check-shared-name");
    let first = mod_with(&dir.join("a"), "Mod", SHARED);
    let second = mod_with(&dir.join("b"), "Mod", SHARED);
    let first_name = format!("Mod ({})", first.display());
    let second_name = format!("Mod ({})", second.display());

    let value = check(json!({ "mods": [first, second] })).unwrap();

    assert_eq!(value["mods"][0]["name"], first_name.as_str(), "{value}");
    assert_eq!(value["mods"][1]["name"], second_name.as_str(), "{value}");
    assert_eq!(value["load_order"], json!([first_name, second_name]));
    let finding = &value["findings"][0];
    assert_eq!(finding["mods"], json!([first_name, second_name]), "{value}");
    assert_eq!(finding["places"][0]["mod"], first_name.as_str());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_config_parse_issue_is_a_warning_under_the_mod_name() {
    let dir = tmp("conflict-check-config");
    let first = mod_with(&dir, "A", SHARED);
    write(
        &first,
        "config.cpp",
        "class CfgPatches { class X { }; \nclass CfgPatches { class Y { }; };",
    );
    let second = mod_with(&dir, "B", SHARED);

    let value = check(json!({ "mods": [first, second] })).unwrap();

    let warnings = value["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "{value}");
    assert!(
        warnings
            .iter()
            .all(|warning| warning.as_str().unwrap().starts_with("A: ")),
        "{value}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mod_folder_with_nothing_to_read_fails_the_check_naming_it() {
    let dir = tmp("conflict-check-empty");
    let empty = dir.join("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    let other = mod_with(&dir, "A", SHARED);

    let err = check(json!({ "mods": [empty, other] })).unwrap_err();

    assert!(
        matches!(err, modfoundry_core::CommandError::Failed(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("Empty"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_arguments_must_name_one_form() {
    let dir = tmp("conflict-check-args");
    let first = mod_with(&dir, "A", SHARED);
    let second = mod_with(&dir, "B", SHARED);
    let project = dir.join("project");
    write(
        &project,
        "server/launch-sets.json",
        &json!({"sets": {"session": {"mods": []}}}).to_string(),
    );

    let cases: Vec<(Value, &str)> = vec![
        (json!({}), "needs mods"),
        (
            json!({ "mods": [first, second], "set": "session" }),
            "not both",
        ),
        (
            json!({ "mods": [first, second], "root": project }),
            "root belongs to a launch set",
        ),
        (json!({ "mods": [first] }), "two or more mods"),
        (
            json!({ "set": "session", "root": project.join("nowhere") }),
            "is missing",
        ),
        (json!({ "set": "nope", "root": project }), "it has: session"),
    ];
    for (args, fragment) in cases {
        let err = check(args.clone()).unwrap_err();
        assert!(
            matches!(err, modfoundry_core::CommandError::InvalidArguments(_)),
            "{args}: {err:?}"
        );
        assert!(err.to_string().contains(fragment), "{args}: {err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_set_reads_its_mods_then_its_server_mods() {
    let dir = tmp("conflict-check-set");
    let project = dir.join("project");
    for (folder, name) in [("mods/Core", "Core"), ("mods/Hud", "Hud")] {
        let root = project.join(folder);
        write(&root, &format!("Scripts/4_World/{name}.c"), SHARED);
        std::fs::create_dir_all(root.join(format!("builds/default/@{name}"))).unwrap();
    }
    let server = mod_with(&project.join("mods"), "Server", SHARED);
    std::fs::create_dir_all(project.join("mods/Server/builds/default/@Server")).unwrap();
    write(
        &project,
        "server/launch-sets.json",
        &json!({"sets": {"session": {
            "mods": [{"local": "mods/Core"}, {"local": "mods/Hud"}],
            "server_mods": [{"local": "mods/Server"}],
        }}})
        .to_string(),
    );

    let value = check(json!({ "set": "session", "root": project })).unwrap();

    assert_eq!(
        value["load_order"],
        json!(["Core", "Hud", "Server"]),
        "{value}"
    );
    assert_eq!(
        value["mods"][0]["path"],
        json!(path_string(&project.join("mods/Core"))),
        "a local mod is read from its source folder"
    );
    assert_eq!(
        value["mods"][2]["path"],
        json!(path_string(&server)),
        "{value}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_set_with_an_unbuilt_mod_fails_with_the_build_hint() {
    let dir = tmp("conflict-check-set-unbuilt");
    let project = dir.join("project");
    for (folder, name) in [("mods/Core", "Core"), ("mods/Hud", "Hud")] {
        write(
            &project.join(folder),
            &format!("Scripts/4_World/{name}.c"),
            SHARED,
        );
    }
    std::fs::create_dir_all(project.join("mods/Core/builds/default/@Core")).unwrap();
    write(
        &project,
        "server/launch-sets.json",
        &json!({"sets": {"session": {"mods": [{"local": "mods/Core"}, {"local": "mods/Hud"}]}}})
            .to_string(),
    );

    let err = check(json!({ "set": "session", "root": project })).unwrap_err();

    assert!(
        matches!(err, modfoundry_core::CommandError::InvalidArguments(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("is not built"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_same_folder_twice_is_refused() {
    let dir = tmp("conflict-check-same-folder");
    let first = mod_with(&dir, "A", SHARED);
    let again = dir.join("A").join("..").join("A");

    let refused = check(json!({"mods": [path_string(&first), path_string(&again)]})).unwrap_err();

    assert!(
        refused
            .to_string()
            .starts_with("command arguments invalid: the same mod folder is given twice"),
        "{refused}"
    );
}

#[cfg(unix)]
#[test]
fn a_mod_folder_whose_name_is_not_utf8_is_still_checked() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tmp("conflict-check-non-utf8");
    let odd = dir.join(std::ffi::OsStr::from_bytes(b"A\xff"));
    write(&odd, "Scripts/4_World/a.c", SHARED);
    let second = mod_with(&dir, "B", SHARED);

    let report = modfoundry_commands::conflict_check::conflict_check(&[odd.clone(), second])
        .expect("the check runs");

    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(value["mods"][0]["path"], json!(path_string(&odd)));
}

#[test]
fn config_findings_and_the_required_addons_order_come_through_the_command() {
    let dir = tmp("conflict-check-data");
    let a = mod_with(&dir, "A", SHARED);
    write(
        &a,
        "config.cpp",
        "class CfgPatches { class A_Patch { requiredAddons[] = {}; }; class Same {}; };",
    );
    let b = mod_with(&dir, "B", SHARED);
    write(
        &b,
        "config.cpp",
        "class CfgPatches { class B_Patch { requiredAddons[] = { \"A_Patch\" }; }; class Same {}; };",
    );

    let report = check(json!({"mods": [path_string(&b), path_string(&a)]})).unwrap();

    assert_eq!(report["load_order"], json!(["A", "B"]));
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["MF-M101", "MF-M201"]);
    assert_eq!(report["findings"][1]["mods"], json!(["A", "B"]));
}
