//! `publish.save_details` against the revision `publish.details` answered, and dropping a saved id.

use std::path::{Path, PathBuf};

use crate::common::tmp;
use modfoundry_core::{Actor, CommandError, Core, Outcome};
use serde_json::{json, Value};

fn details(core: &Core, dir: &Path) -> Value {
    let outcome = core
        .dispatch(
            Actor::CliSession,
            "publish.details",
            json!({ "mod_root": dir }),
        )
        .unwrap();
    let Outcome::Data(view) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    view
}

fn save(core: &Core, dir: &Path, extra: Value) -> Result<Outcome, CommandError> {
    let mut args = json!({ "mod_root": dir, "details": { "title": "Field Kit" } });
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    core.dispatch(Actor::CliSession, "publish.save_details", args)
}

fn scaffold(label: &str) -> PathBuf {
    let dir = tmp(label);
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dir
}

#[test]
fn details_answer_a_revision_that_changes_with_a_save() {
    let dir = scaffold("save-revision");
    let core = crate::common::core();

    let first = details(&core, &dir);
    let revision = first["revision"].as_str().expect("a revision").to_owned();
    assert_eq!(revision.len(), 64, "{revision}");
    assert!(
        revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{revision}"
    );
    assert_eq!(details(&core, &dir)["revision"], revision.as_str());

    save(&core, &dir, json!({ "details": { "title": "Another" } })).unwrap();

    assert_ne!(details(&core, &dir)["revision"], revision.as_str());
}

#[test]
fn a_save_with_the_revision_that_was_read_lands() {
    let dir = scaffold("save-expected");
    let core = crate::common::core();
    let revision = details(&core, &dir)["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let outcome = save(
        &core,
        &dir,
        json!({ "expected_sha256": revision, "details": { "title": "Field Kit" } }),
    );

    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(details(&core, &dir)["item"]["title"], "Field Kit");
}

#[test]
fn a_stale_revision_refuses_the_save_and_the_preview_and_writes_nothing() {
    let cases = [
        (
            "item",
            "workshop/item.json",
            "{\"title\":\"Hand Edited\"}\n",
        ),
        (
            "description",
            "workshop/description.txt",
            "written out of band",
        ),
    ];
    for (label, changed, text) in cases {
        let dir = scaffold(&format!("save-stale-{label}"));
        let core = crate::common::core();
        let revision = details(&core, &dir)["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        std::fs::create_dir_all(dir.join("workshop")).unwrap();
        std::fs::write(dir.join(changed), text).unwrap();

        for preview in [false, true] {
            let extra = json!({ "expected_sha256": revision, "preview": preview });
            let error = save(&core, &dir, extra).expect_err(label);
            let CommandError::Failed(message) = error else {
                panic!("{label}: expected Failed, got {error:?}");
            };
            assert!(
                message.starts_with("stale Workshop details: "),
                "{label}: {message}"
            );
            assert_eq!(std::fs::read_to_string(dir.join(changed)).unwrap(), text);
        }
    }
}

#[test]
fn clear_id_removes_the_saved_id() {
    let dir = scaffold("save-clear-id");
    let core = crate::common::core();
    save(
        &core,
        &dir,
        json!({ "details": { "id": 7, "title": "Field Kit" } }),
    )
    .unwrap();

    save(
        &core,
        &dir,
        json!({ "clear_id": true, "details": { "title": "Field Kit" } }),
    )
    .unwrap();

    assert!(details(&core, &dir)["item"]["id"].is_null());
    let item = std::fs::read_to_string(dir.join("workshop/item.json")).unwrap();
    assert!(!item.contains("\"id\""), "{item}");

    save(
        &core,
        &dir,
        json!({ "details": { "id": 7, "title": "Field Kit" } }),
    )
    .unwrap();
    save(&core, &dir, json!({ "details": { "title": "Field Kit" } })).unwrap();
    assert_eq!(details(&core, &dir)["item"]["id"], 7);
}

#[test]
fn clear_id_with_an_id_is_an_argument_error() {
    let dir = scaffold("save-clear-id-conflict");
    let core = crate::common::core();

    for preview in [false, true] {
        let error = save(
            &core,
            &dir,
            json!({ "clear_id": true, "preview": preview, "details": { "id": 3, "title": "Field Kit" } }),
        )
        .expect_err("clear_id with an id");
        assert!(
            matches!(error, CommandError::InvalidArguments(_)),
            "{error:?}"
        );
    }
    assert!(!dir.join("workshop").exists());
}

#[test]
fn clear_id_is_refused_while_meta_cpp_names_an_item() {
    let dir = scaffold("save-clear-id-meta");
    let core = crate::common::core();

    std::fs::write(dir.join("meta.cpp"), "publishedid = 123;\r\n").unwrap();
    let error = save(
        &core,
        &dir,
        json!({ "clear_id": true, "details": { "title": "Field Kit" } }),
    )
    .expect_err("meta.cpp names an item");
    let CommandError::Failed(message) = error else {
        panic!("expected Failed, got {error:?}");
    };
    assert!(message.contains("meta.cpp"), "{message}");

    std::fs::write(dir.join("meta.cpp"), "publishedid = 0;\r\n").unwrap();
    save(
        &core,
        &dir,
        json!({ "clear_id": true, "details": { "title": "Field Kit" } }),
    )
    .unwrap();
    assert!(details(&core, &dir)["item"]["id"].is_null());
}

#[test]
fn clear_id_with_no_saved_id_saves_the_rest() {
    let dir = scaffold("save-clear-no-id");
    let core = crate::common::core();

    save(
        &core,
        &dir,
        json!({ "clear_id": true, "details": { "title": "Renamed" } }),
    )
    .unwrap();

    let view = details(&core, &dir);
    assert!(view["item"]["id"].is_null(), "{view}");
    assert_eq!(view["item"]["title"], "Renamed");
}
