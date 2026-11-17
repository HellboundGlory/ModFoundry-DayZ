//! `config.get_tree` and `config.validate {text}` contract tests.
//!
//! `get_tree` is the editor/agent view of one config file without resolution;
//! `config.validate` gained an optional unsaved `text` that must be validated
//! against the config's own directory as the source root.

use crate::common::{self, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_core::{Actor, CommandError, Outcome};
use serde_json::{json, Value};

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

const VALID_MOD: &str = r#"class CfgPatches { class MyMod { requiredAddons[] = { "DZ_Data" }; }; };
class CfgMods {
    class MyMod {
        dir = "MyMod";
        type = "mod";
        class defs {
            class worldScriptModule { files[] = { "MyMod/Scripts/4_World" }; };
        };
    };
};
"#;

#[test]
fn get_tree_reads_the_file_into_an_outline_with_parse_diagnostics() {
    let dir = tmp("file");
    let cfg = write(&dir, "config.cpp", "class Root : Base\n{\n    a = 1;\n};\n");

    let out = dispatch(&common::core(), "config.get_tree", json!({ "path": cfg }));

    assert_eq!(out["outline"]["roots"][0]["name"], "Root", "{out}");
    assert_eq!(out["outline"]["roots"][0]["base"], "Base");
    assert_eq!(out["outline"]["issues"], 0);
    assert_eq!(out["outline"]["has_include"], false);
    assert_eq!(out["diagnostics"], json!([]));
    assert!(out.get("class").is_none(), "class is opt-in: {out}");
}

#[test]
fn get_tree_prefers_the_given_text_over_the_file() {
    let dir = tmp("text");
    let missing = dir.join("config.cpp");

    let out = dispatch(
        &common::core(),
        "config.get_tree",
        json!({ "path": missing, "text": "class Only { a = 1; };\n" }),
    );

    assert_eq!(out["outline"]["roots"][0]["name"], "Only", "{out}");
    assert_eq!(out["outline"]["roots"][0]["path"], json!(["Only"]));
}

#[test]
fn get_tree_returns_the_requested_class_view() {
    let dir = tmp("class");
    let cfg = write(
        &dir,
        "config.cpp",
        "class Root\n{\n    class Child\n    {\n        x = 1;\n    };\n};\n",
    );

    let out = dispatch(
        &common::core(),
        "config.get_tree",
        json!({ "path": cfg, "class": ["root", "child"] }),
    );

    assert_eq!(out["class"]["path"], json!(["root", "child"]), "{out}");
    assert_eq!(out["class"]["properties"][0]["name"], "x");
    assert_eq!(out["class"]["properties"][0]["display"], "1");
}

#[test]
fn a_missing_file_is_an_error_that_names_the_path() {
    let path = tmp("gone").join("config.cpp");
    let err = common::core()
        .dispatch(Actor::User, "config.get_tree", json!({ "path": path }))
        .expect_err("a missing file is an error");
    let CommandError::Failed(message) = err else {
        panic!("expected Failed, got {err:?}");
    };
    assert!(
        message.contains(path.to_str().unwrap()),
        "the message names the path: {message}"
    );
}

#[test]
fn get_tree_leaves_no_audit_row() {
    let dir = tmp("audit");
    let cfg = write(&dir, "config.cpp", "class A { x = 1; };\n");
    let broker = Broker::in_memory(common::core()).unwrap();

    broker
        .dispatch(Actor::User, "config.get_tree", json!({ "path": cfg }))
        .unwrap();

    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn validate_with_text_reports_the_text_not_the_file() {
    let dir = tmp("validate");
    let cfg = write(&dir, "config.cpp", VALID_MOD);
    std::fs::create_dir_all(dir.join("MyMod/Scripts/4_World")).unwrap();
    let core = common::core();

    let file = core
        .dispatch(Actor::User, "config.validate", json!({ "path": cfg }))
        .unwrap();
    let Outcome::Diagnostics { count, .. } = file else {
        panic!("expected Diagnostics, got {file:?}");
    };
    assert_eq!(count, 0, "the scaffold-shaped config is clean");

    let buffer = core
        .dispatch(
            Actor::User,
            "config.validate",
            json!({ "path": cfg, "text": "class A { x = 1 };" }),
        )
        .unwrap();
    let Outcome::Diagnostics { diagnostics, .. } = buffer else {
        panic!("expected Diagnostics, got {buffer:?}");
    };
    assert!(
        diagnostics.iter().any(|d| d.code == "MF-C101"),
        "{diagnostics:?}"
    );
}

#[test]
fn validate_with_text_uses_the_config_directory_as_the_source_root() {
    let dir = tmp("root");
    let cfg = dir.join("config.cpp");
    std::fs::create_dir_all(&dir).unwrap();

    let missing_dir = dispatch(
        &common::core(),
        "config.validate",
        json!({ "path": cfg, "text": VALID_MOD }),
    );
    assert!(
        missing_dir
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "MF-C320"),
        "{missing_dir}"
    );

    std::fs::create_dir_all(dir.join("MyMod/Scripts/4_World")).unwrap();
    let present = dispatch(
        &common::core(),
        "config.validate",
        json!({ "path": cfg, "text": VALID_MOD }),
    );
    assert!(
        !present
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "MF-C320"),
        "{present}"
    );
}

#[test]
fn validate_leaves_out_a_config_the_pboignore_leaves_out() {
    let broken = "class A { x = 1 };\n";
    let cases: [(&str, Option<&str>, &str, bool, bool); 8] = [
        ("no pboignore", None, "config.cpp", false, true),
        (
            "name rule",
            Some("config.cpp\n"),
            "config.cpp",
            false,
            false,
        ),
        (
            "anchored rule",
            Some("/config.cpp\n"),
            "config.cpp",
            false,
            false,
        ),
        (
            "re-included",
            Some("config.cpp\n!/config.cpp\n"),
            "config.cpp",
            false,
            true,
        ),
        ("other file", Some("*.md\n"), "config.cpp", false, true),
        (
            "rule on a parent",
            Some("/Addon/\n"),
            "Addon/config.cpp",
            false,
            false,
        ),
        (
            "text given",
            Some("config.cpp\n"),
            "config.cpp",
            true,
            false,
        ),
        (
            "default skipped folder",
            None,
            "tests/config.cpp",
            false,
            true,
        ),
    ];
    for (label, pboignore, rel, text_given, want_findings) in cases {
        let dir = tmp("validate-ignored");
        let cfg = dir.join(rel);
        if text_given {
            std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        } else {
            write(&dir, rel, broken);
        }
        if let Some(text) = pboignore {
            std::fs::write(dir.join(".pboignore"), text).unwrap();
        }
        let mut args = json!({ "path": cfg });
        if text_given {
            args["text"] = json!(broken);
        }
        let out = common::core()
            .dispatch(Actor::User, "config.validate", args)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let Outcome::Diagnostics { diagnostics, .. } = out else {
            panic!("{label}: expected Diagnostics, got {out:?}");
        };
        assert_eq!(!diagnostics.is_empty(), want_findings, "{label}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(unix)]
#[test]
fn validate_reads_the_pboignore_beside_a_linked_config_not_beside_its_target() {
    let dir = tmp("validate-ignored-link");
    let shared = tmp("validate-ignored-link-shared");
    write(&shared, "config.cpp", "class A { x = 1 };\n");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".pboignore"), "config.cpp\n").unwrap();
    std::os::unix::fs::symlink(shared.join("config.cpp"), dir.join("config.cpp")).unwrap();

    let out = common::core()
        .dispatch(
            Actor::User,
            "config.validate",
            json!({ "path": dir.join("config.cpp") }),
        )
        .unwrap();

    let Outcome::Diagnostics { diagnostics, .. } = out else {
        panic!("expected Diagnostics, got {out:?}");
    };
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&shared);
}

#[test]
fn validate_errors_on_a_config_the_pboignore_leaves_out_but_the_disk_lacks() {
    let dir = tmp("validate-ignored-gone");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".pboignore"), "config.cpp\n").unwrap();
    let err = common::core()
        .dispatch(
            Actor::User,
            "config.validate",
            json!({ "path": dir.join("config.cpp") }),
        )
        .expect_err("a missing file is an error even when it is left out");
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_tree_resolves_a_directory_to_its_config_cpp() {
    let dir = tmp("tree-dir");
    write(&dir, "config.cpp", "class Root : Base\n{\n    a = 1;\n};\n");

    let out = dispatch(&common::core(), "config.get_tree", json!({ "path": dir }));
    assert_eq!(out["outline"]["roots"][0]["name"], "Root", "{out}");
    assert_eq!(out["diagnostics"], json!([]), "{out}");
}
