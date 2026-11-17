//! `config.edit` contract tests.
//!
//! `config.edit` is `text.edit`'s structured twin: the caller sends edit ops
//! instead of the whole new text, and gets the same preview-first change-set,
//! the same `expected_sha256` base-revision guard and the same `workspace.undo`
//! inverse. The ops' serde JSON shape is pinned here because it is the wire
//! contract both the MCP tool and the GUI send.

use std::fs;
use std::path::Path;

use crate::common::{self, tmp, write};
use modfoundry_config::edit::{plan_all, ConfigPath, EditOp, NewValue, Position, Target};
use modfoundry_core::{Actor, ChangeSummary, CommandError, Core, EffectKind, Outcome};
use serde_json::{json, Value};

fn dispatch(core: &Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn changed(out: &Outcome) -> (&ChangeSummary, Value) {
    match out {
        Outcome::Changed {
            summary,
            change_set: Some(change_set),
        } => (summary, change_set.clone()),
        other => panic!("expected a recorded change, got {other:?}"),
    }
}

/// A config with a comment and non-default formatting, so an edit that keeps
/// them is distinguishable from one that reformats the file.
const CONFIG: &str = "class CfgPatches\n{\n    class MyMod\n    {\n        // keep this comment\n        displayName = \"MyMod\";\n        requiredAddons[] = { \"DZ_Data\" };\n    };\n};\n";

fn add_property(class: &[&str], name: &str, value: NewValue) -> EditOp {
    EditOp::AddProperty {
        class: ConfigPath::of(class),
        name: name.into(),
        value,
        position: Position::Last,
    }
}

fn edit_args(path: &Path, ops: &[EditOp], preview: bool) -> Value {
    json!({
        "path": path,
        "ops": serde_json::to_value(ops).unwrap(),
        "preview": preview,
    })
}

/// The SHA-256 of `text` as the registry itself computes it: a dry-run
/// `text.edit` reports the digest of the content it would write.
fn digest_of(core: &Core, text: &str) -> Value {
    let probe = tmp("digest").join("probe.c");
    let Outcome::Preview(plan) = dispatch(
        core,
        "text.edit",
        json!({ "path": probe, "text": text, "preview": true }),
    ) else {
        panic!("expected a preview");
    };
    plan["changes"][0]["after_sha256"].clone()
}

#[test]
fn a_preview_plans_the_write_and_touches_nothing() {
    let core = common::core();
    let dir = tmp("preview");
    let cfg = write(&dir, "config.cpp", CONFIG);
    let ops = [add_property(
        &["CfgPatches", "MyMod"],
        "units",
        NewValue::Array(vec![]),
    )];
    let expected = plan_all(CONFIG, &ops).unwrap();
    assert!(expected.contains("// keep this comment"), "{expected}");

    let Outcome::Preview(plan) = dispatch(&core, "config.edit", edit_args(&cfg, &ops, true)) else {
        panic!("expected a preview");
    };

    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["op"], "update");
    assert!(changes[0]["path"].as_str().unwrap().ends_with("config.cpp"));
    assert_eq!(
        changes[0]["after_sha256"],
        digest_of(&core, &expected),
        "the preview plans exactly the bytes `plan_all` produces"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        CONFIG,
        "a preview writes nothing"
    );
}

#[test]
fn an_apply_reports_its_change_set_and_undo_restores_the_bytes() {
    let core = common::core();
    let dir = tmp("apply");
    let cfg = write(&dir, "config.cpp", CONFIG);
    let ops = [add_property(
        &["CfgPatches", "MyMod"],
        "units",
        NewValue::Array(vec![]),
    )];
    let expected = plan_all(CONFIG, &ops).unwrap();

    let out = dispatch(&core, "config.edit", edit_args(&cfg, &ops, false));
    let (summary, change_set) = changed(&out);
    assert_eq!(summary.kind, EffectKind::Mutate);
    assert_eq!(summary.files_changed, 1);
    assert!(
        summary.description.contains("1") && summary.description.contains("config.cpp"),
        "the summary names the ops count and the file: {}",
        summary.description
    );
    assert_eq!(fs::read_to_string(&cfg).unwrap(), expected);

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&cfg).unwrap(), CONFIG);
}

#[test]
fn a_stale_expected_sha256_refuses_the_preview_and_the_apply() {
    let core = common::core();
    let dir = tmp("stale");
    let cfg = write(&dir, "config.cpp", CONFIG);
    let ops = [add_property(
        &["CfgPatches", "MyMod"],
        "units",
        NewValue::Array(vec![]),
    )];

    let out = dispatch(&core, "text.edit", json!({ "path": cfg, "text": CONFIG }));
    let (_summary, change_set) = changed(&out);
    let digest = change_set["changes"][0]["after_sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    let other = CONFIG.replace("DZ_Data", "DZ_Gear");
    fs::write(&cfg, &other).unwrap();

    let mut args = edit_args(&cfg, &ops, true);
    args["expected_sha256"] = json!(digest);
    let err = core
        .dispatch(Actor::CliSession, "config.edit", args)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert!(err.to_string().contains("stale"), "{err}");

    let mut args = edit_args(&cfg, &ops, false);
    args["expected_sha256"] = json!(digest);
    let err = core
        .dispatch(Actor::CliSession, "config.edit", args)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        other,
        "a stale base revision must never clobber the edit it did not see"
    );
}

#[test]
fn an_invalid_op_is_an_argument_error_that_writes_nothing() {
    let core = common::core();
    let dir = tmp("invalid");
    let cfg = write(&dir, "config.cpp", CONFIG);

    let unknown = [add_property(&["Nope"], "x", NewValue::Number("1".into()))];
    let err = core
        .dispatch(
            Actor::CliSession,
            "config.edit",
            edit_args(&cfg, &unknown, false),
        )
        .unwrap_err();
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(message.contains("was not found"), "{message}");

    let bad_string = [EditOp::SetValue {
        property: Target::Property {
            class: ConfigPath::of(&["CfgPatches", "MyMod"]),
            name: "displayName".into(),
        },
        value: NewValue::Str("a\"b".into()),
    }];
    let err = core
        .dispatch(
            Actor::CliSession,
            "config.edit",
            edit_args(&cfg, &bad_string, false),
        )
        .unwrap_err();
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(message.contains("Strings can't contain"), "{message}");

    assert_eq!(fs::read_to_string(&cfg).unwrap(), CONFIG);
}

#[test]
fn multiple_ops_apply_in_order() {
    let core = common::core();
    let dir = tmp("ordered");
    let cfg = write(&dir, "config.cpp", CONFIG);
    let ops = [
        EditOp::AddClass {
            parent: ConfigPath::of(&[]),
            name: "CfgExtra".into(),
            base: None,
            forward: false,
            position: Position::Last,
        },
        add_property(&["CfgExtra"], "enabled", NewValue::Ident("true".into())),
    ];

    let out = dispatch(&core, "config.edit", edit_args(&cfg, &ops, false));
    let (_summary, change_set) = changed(&out);

    let after = fs::read_to_string(&cfg).unwrap();
    assert_eq!(after, plan_all(CONFIG, &ops).unwrap());
    let class_at = after.find("class CfgExtra").expect("the class was added");
    let property_at = after
        .find("enabled = true;")
        .expect("the property was added");
    assert!(class_at < property_at, "{after}");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&cfg).unwrap(), CONFIG);
}

#[test]
fn every_edit_op_variant_round_trips_through_its_documented_json() {
    let class = ConfigPath::of(&["CfgVehicles", "Item"]);
    let property = Target::Property {
        class: class.clone(),
        name: "itemSize".into(),
    };
    let cases: Vec<(EditOp, Value)> = vec![
        (
            EditOp::SetValue {
                property: property.clone(),
                value: NewValue::Number("1".into()),
            },
            json!({ "set_value": { "property": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "value": { "number": "1" } } }),
        ),
        (
            add_property(
                &["CfgVehicles", "Item"],
                "scope",
                NewValue::Ident("public".into()),
            ),
            json!({ "add_property": { "class": ["CfgVehicles", "Item"], "name": "scope", "value": { "ident": "public" }, "position": "last" } }),
        ),
        (
            EditOp::Remove {
                target: Target::Class(class.clone()),
            },
            json!({ "remove": { "target": { "class": ["CfgVehicles", "Item"] } } }),
        ),
        (
            EditOp::Rename {
                target: property.clone(),
                new_name: "sizes".into(),
            },
            json!({ "rename": { "target": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "new_name": "sizes" } }),
        ),
        (
            EditOp::AddClass {
                parent: class.clone(),
                name: "Sub".into(),
                base: Some("Base".into()),
                forward: false,
                position: Position::After("itemSize".into()),
            },
            json!({ "add_class": { "parent": ["CfgVehicles", "Item"], "name": "Sub", "base": "Base", "forward": false, "position": { "after": "itemSize" } } }),
        ),
        (
            EditOp::SetBase {
                class: class.clone(),
                base: Some("Base".into()),
            },
            json!({ "set_base": { "class": ["CfgVehicles", "Item"], "base": "Base" } }),
        ),
        (
            EditOp::ArrayInsert {
                property: property.clone(),
                index: 0,
                value: NewValue::Str("x".into()),
            },
            json!({ "array_insert": { "property": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "index": 0, "value": { "str": "x" } } }),
        ),
        (
            EditOp::ArrayRemove {
                property: property.clone(),
                index: 1,
            },
            json!({ "array_remove": { "property": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "index": 1 } }),
        ),
        (
            EditOp::ArraySet {
                property: property.clone(),
                index: 0,
                value: NewValue::Number("2".into()),
            },
            json!({ "array_set": { "property": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "index": 0, "value": { "number": "2" } } }),
        ),
        (
            EditOp::ArrayMove {
                property: property.clone(),
                from: 0,
                to: 1,
            },
            json!({ "array_move": { "property": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "from": 0, "to": 1 } }),
        ),
        (
            EditOp::SetArray {
                target: property.clone(),
                array: true,
            },
            json!({ "set_array": { "target": { "property": { "class": ["CfgVehicles", "Item"], "name": "itemSize" } }, "array": true } }),
        ),
        (
            EditOp::RemoveAt {
                class: class.clone(),
                name: "itemSize".into(),
                occurrence: 1,
            },
            json!({ "remove_at": { "class": ["CfgVehicles", "Item"], "name": "itemSize", "occurrence": 1 } }),
        ),
    ];

    let schema = common::core().command("config.edit").unwrap().args_schema();
    let branches = schema["properties"]["ops"]["items"]["oneOf"]
        .as_array()
        .expect("the ops schema is a oneOf")
        .clone();
    let mut documented: Vec<&str> = branches
        .iter()
        .map(|b| b["required"][0].as_str().unwrap())
        .collect();
    documented.sort();

    let mut tags = Vec::new();
    for (op, wire) in &cases {
        assert_eq!(&serde_json::to_value(op).unwrap(), wire, "{op:?}");
        assert_eq!(
            serde_json::from_value::<EditOp>(wire.clone()).unwrap(),
            *op,
            "{wire}"
        );
        tags.push(wire.as_object().unwrap().keys().next().unwrap().as_str());
    }
    tags.sort();
    assert_eq!(documented, tags, "the schema documents every op variant");
    assert_eq!(schema["properties"]["preview"]["type"], "boolean");
}
