//! An Unlisted item keeps its visibility through a read, a save and the upload prompt.

use crate::common::{self, tmp};
use modfoundry_core::{Actor, Outcome};
use serde_json::{json, Value};

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn new_mod(label: &str) -> std::path::PathBuf {
    let dir = tmp(label);
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dir
}

fn details(core: &modfoundry_core::Core, mod_root: &std::path::Path) -> Value {
    let Outcome::Data(view) = dispatch(core, "publish.details", json!({ "mod_root": mod_root }))
    else {
        panic!("expected Data");
    };
    view
}

#[test]
fn an_unlisted_item_reads_back_unlisted_after_a_save() {
    let dir = new_mod("publish-unlisted-save");
    let core = common::core();

    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit", "visibility": "unlisted" },
        }),
    );
    let view = details(&core, &dir);
    assert_eq!(view["item"]["visibility"], "unlisted");

    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": view["item"] }),
    );
    assert_eq!(details(&core, &dir)["item"]["visibility"], "unlisted");
}

#[test]
fn the_upload_prompt_names_unlisted() {
    let cases = [
        (
            json!({ "title": "Field Kit", "visibility": "unlisted" }),
            false,
            r#"Upload "Field Kit" as a new unlisted item from {root}"#,
        ),
        (
            json!({ "id": 7, "title": "Field Kit", "visibility": "unlisted" }),
            true,
            r#"Update item 7 "Field Kit" (unlisted), content only from {root}"#,
        ),
    ];
    for (item, content_only, expected) in cases {
        let dir = new_mod("publish-unlisted-prompt");
        let core = common::core();
        dispatch(
            &core,
            "publish.save_details",
            json!({ "mod_root": dir, "details": item }),
        );

        let summary = core
            .command("publish.upload")
            .unwrap()
            .confirm_summary(&json!({ "mod_root": dir, "content_only": content_only }));

        let expected = expected.replace("{root}", &dir.display().to_string());
        assert_eq!(summary.as_deref(), Some(expected.as_str()));
    }
}
