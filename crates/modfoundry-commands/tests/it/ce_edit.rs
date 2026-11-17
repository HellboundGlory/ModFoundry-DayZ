//! `ce.edit` contract tests.
//!
//! `ce.edit` is `config.edit`'s Central Economy twin: the caller sends
//! record-level ops instead of the whole new text, and gets the same
//! preview-first change-set, the same `expected_sha256` base-revision guard and
//! the same `workspace.undo` inverse. The ops' serde JSON shape is pinned here
//! because it is the wire contract an MCP tool or the GUI sends.

use std::fs;
use std::path::Path;

use crate::common::{self, tmp, write};
use modfoundry_ce::ops::{plan_all, CeOp, RecordPosition, RecordRef};
use modfoundry_core::{Actor, ChangeSummary, CommandError, Core, EffectKind, Outcome};
use serde_json::{json, Value};

/// A types file with a comment and a usage list, so an edit that keeps them is
/// distinguishable from one that reformats the file.
const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<types>\n    <!-- keep this comment -->\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n        <min>6</min>\n        <usage name=\"Military\"/>\n    </type>\n    <type name=\"FilterCartridge\">\n        <nominal>20</nominal>\n        <min>24</min>\n    </type>\n</types>\n";

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

fn ref_of(key: &str) -> RecordRef {
    RecordRef {
        key: key.to_string(),
        occurrence: 0,
    }
}

fn args(path: &Path, ops: &[CeOp], preview: bool) -> Value {
    json!({
        "path": path,
        "ops": serde_json::to_value(ops).unwrap(),
        "preview": preview,
    })
}

/// The SHA-256 of `text` as the registry itself computes it: a dry-run
/// `text.edit` reports the digest of the content it would write.
fn digest_of(core: &Core, text: &str) -> Value {
    let probe = tmp("digest").join("probe.xml");
    let Outcome::Preview(plan) = dispatch(
        core,
        "text.edit",
        json!({ "path": probe, "text": text, "preview": true }),
    ) else {
        panic!("expected a preview");
    };
    plan["changes"][0]["after_sha256"].clone()
}

fn set_nominal(key: &str, value: &str) -> CeOp {
    CeOp::SetField {
        section: None,
        record: ref_of(key),
        field: "nominal".to_string(),
        value: Some(value.to_string()),
    }
}

#[test]
fn a_preview_plans_the_write_and_touches_nothing() {
    let core = common::core();
    let dir = tmp("preview");
    let file = write(&dir, "types.xml", SAMPLE);
    let ops = [set_nominal("FieldCanteen", "30")];
    let expected = plan_all(SAMPLE, Some("types.xml"), &ops).unwrap();
    assert!(
        expected.contains("<!-- keep this comment -->"),
        "{expected}"
    );

    let Outcome::Preview(plan) = dispatch(&core, "ce.edit", args(&file, &ops, true)) else {
        panic!("expected a preview");
    };

    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["op"], "update");
    assert!(changes[0]["path"].as_str().unwrap().ends_with("types.xml"));
    assert_eq!(
        changes[0]["after_sha256"],
        digest_of(&core, &expected),
        "the preview plans exactly the bytes `plan_all` produces"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        SAMPLE,
        "a preview writes nothing"
    );
}

#[test]
fn an_apply_reports_its_change_set_and_undo_restores_the_bytes() {
    let core = common::core();
    let dir = tmp("apply");
    let file = write(&dir, "types.xml", SAMPLE);
    let ops = [set_nominal("FieldCanteen", "30")];
    let expected = plan_all(SAMPLE, Some("types.xml"), &ops).unwrap();

    let out = dispatch(&core, "ce.edit", args(&file, &ops, false));
    let (summary, change_set) = changed(&out);
    assert_eq!(summary.kind, EffectKind::Mutate);
    assert_eq!(summary.files_changed, 1);
    assert!(
        summary.description.contains("1") && summary.description.contains("types.xml"),
        "the summary names the op count and the file: {}",
        summary.description
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), expected);

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&file).unwrap(), SAMPLE);
}

#[test]
fn a_stale_expected_sha256_refuses_the_preview_and_the_apply() {
    let core = common::core();
    let dir = tmp("stale");
    let file = write(&dir, "types.xml", SAMPLE);
    let ops = [set_nominal("FieldCanteen", "30")];

    let out = dispatch(&core, "text.edit", json!({ "path": file, "text": SAMPLE }));
    let (_summary, change_set) = changed(&out);
    let digest = change_set["changes"][0]["after_sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    let other = SAMPLE.replace("Military", "Coast");
    fs::write(&file, &other).unwrap();

    let mut preview = args(&file, &ops, true);
    preview["expected_sha256"] = json!(digest);
    let err = core
        .dispatch(Actor::CliSession, "ce.edit", preview)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert!(err.to_string().contains("stale"), "{err}");

    let mut apply = args(&file, &ops, false);
    apply["expected_sha256"] = json!(digest);
    let err = core
        .dispatch(Actor::CliSession, "ce.edit", apply)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        other,
        "a stale base revision must never clobber the edit it did not see"
    );
}

#[test]
fn an_invalid_op_is_an_argument_error_that_writes_nothing() {
    let core = common::core();
    let dir = tmp("invalid");
    let file = write(&dir, "types.xml", SAMPLE);

    let unknown = [set_nominal("Nope", "1")];
    let err = core
        .dispatch(Actor::CliSession, "ce.edit", args(&file, &unknown, false))
        .unwrap_err();
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert!(message.contains("No type named `Nope` here."), "{message}");

    let bad_value = [set_nominal("FieldCanteen", "abc")];
    let err = core
        .dispatch(Actor::CliSession, "ce.edit", args(&file, &bad_value, true))
        .unwrap_err();
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert_eq!(message, "Nominal must be a whole number.");

    assert_eq!(fs::read_to_string(&file).unwrap(), SAMPLE);
}

#[test]
fn multiple_ops_apply_in_order() {
    let core = common::core();
    let dir = tmp("ordered");
    let file = write(&dir, "types.xml", SAMPLE);
    let ops = [
        CeOp::AddRecord {
            section: None,
            key: "NewGun".to_string(),
            element: None,
            fields: vec![("nominal".to_string(), "7".to_string())],
            lists: Vec::new(),
            position: RecordPosition::Last,
        },
        set_nominal("NewGun", "9"),
        CeOp::AddListItem {
            section: None,
            record: ref_of("NewGun"),
            list: "usage".to_string(),
            attrs: vec![("name".to_string(), "Military".to_string())],
            index: None,
        },
    ];

    let out = dispatch(&core, "ce.edit", args(&file, &ops, false));
    let (_summary, change_set) = changed(&out);

    let after = fs::read_to_string(&file).unwrap();
    assert_eq!(after, plan_all(SAMPLE, Some("types.xml"), &ops).unwrap());
    let added = after.find("NewGun").expect("the record was added");
    let nominal = after[added..]
        .find("<nominal>9</nominal>")
        .expect("the field");
    let usage = after[added..]
        .find("<usage name=\"Military\"/>")
        .expect("the usage was added");
    assert!(nominal < usage, "{after}");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&file).unwrap(), SAMPLE);
}

#[test]
fn a_fragment_file_wrap_root_adds_the_root() {
    let core = common::core();
    let dir = tmp("fragment");
    let fragment = "<type name=\"Loose\">\n    <nominal>3</nominal>\n</type>\n";
    let file = write(&dir, "types.xml", fragment);
    let expected = plan_all(fragment, Some("types.xml"), &[CeOp::WrapRoot]).unwrap();
    assert!(expected.contains("<types>"), "{expected}");

    // `wrap_root` serializes as the bare tag string but both JSON forms load.
    let wire = json!([{ "wrap_root": null }]);
    assert_eq!(
        serde_json::from_value::<Vec<CeOp>>(wire).unwrap(),
        vec![CeOp::WrapRoot]
    );

    let Outcome::Preview(plan) = dispatch(
        &core,
        "ce.edit",
        json!({ "path": file, "ops": [{ "wrap_root": null }], "preview": true }),
    ) else {
        panic!("expected a preview");
    };
    assert_eq!(
        plan["changes"][0]["after_sha256"],
        digest_of(&core, &expected)
    );

    dispatch(
        &core,
        "ce.edit",
        json!({ "path": file, "ops": [{ "wrap_root": null }] }),
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), expected);
}

#[test]
fn every_ce_op_variant_round_trips_through_its_documented_json() {
    let record = ref_of("FieldCanteen");
    let cases: Vec<(CeOp, Value)> = vec![
        (
            set_nominal("FieldCanteen", "12"),
            json!({ "set_field": { "record": { "key": "FieldCanteen" }, "field": "nominal", "value": "12" } }),
        ),
        (
            CeOp::SetField {
                section: None,
                record: record.clone(),
                field: "nominal".to_string(),
                value: None,
            },
            json!({ "set_field": { "record": { "key": "FieldCanteen" }, "field": "nominal", "value": null } }),
        ),
        (
            CeOp::SetField {
                section: Some("types".to_string()),
                record: record.clone(),
                field: "nominal".to_string(),
                value: Some("12".to_string()),
            },
            json!({ "set_field": { "section": "types", "record": { "key": "FieldCanteen" }, "field": "nominal", "value": "12" } }),
        ),
        (
            CeOp::AddRecord {
                section: None,
                key: "NewGun".to_string(),
                element: None,
                fields: vec![("nominal".to_string(), "7".to_string())],
                lists: Vec::new(),
                position: RecordPosition::Last,
            },
            json!({ "add_record": { "key": "NewGun", "fields": [["nominal", "7"]], "position": "last" } }),
        ),
        (
            CeOp::AddRecord {
                section: None,
                key: "NewGun".to_string(),
                element: None,
                fields: Vec::new(),
                lists: Vec::new(),
                position: RecordPosition::After(record.clone()),
            },
            json!({ "add_record": { "key": "NewGun", "position": { "after": { "key": "FieldCanteen" } } } }),
        ),
        (
            CeOp::RemoveRecord {
                section: None,
                record: RecordRef {
                    key: "FieldCanteen".to_string(),
                    occurrence: 1,
                },
            },
            json!({ "remove_record": { "record": { "key": "FieldCanteen", "occurrence": 1 } } }),
        ),
        (
            CeOp::RenameRecord {
                section: None,
                record: record.clone(),
                new_key: "FieldBottle".to_string(),
            },
            json!({ "rename_record": { "record": { "key": "FieldCanteen" }, "new_key": "FieldBottle" } }),
        ),
        (
            CeOp::DuplicateRecord {
                section: None,
                record: record.clone(),
                new_key: "FieldCanteenCopy".to_string(),
            },
            json!({ "duplicate_record": { "record": { "key": "FieldCanteen" }, "new_key": "FieldCanteenCopy" } }),
        ),
        (
            CeOp::AddListItem {
                section: None,
                record: record.clone(),
                list: "usage".to_string(),
                attrs: vec![("name".to_string(), "Military".to_string())],
                index: None,
            },
            json!({ "add_list_item": { "record": { "key": "FieldCanteen" }, "list": "usage", "attrs": [["name", "Military"]] } }),
        ),
        (
            CeOp::RemoveListItem {
                section: None,
                record: record.clone(),
                list: "usage".to_string(),
                index: 0,
            },
            json!({ "remove_list_item": { "record": { "key": "FieldCanteen" }, "list": "usage", "index": 0 } }),
        ),
        (
            CeOp::SetListItem {
                section: None,
                record: record.clone(),
                list: "usage".to_string(),
                index: 0,
                attr: "name".to_string(),
                value: Some("Coast".to_string()),
            },
            json!({ "set_list_item": { "record": { "key": "FieldCanteen" }, "list": "usage", "index": 0, "attr": "name", "value": "Coast" } }),
        ),
        (
            CeOp::SetListItem {
                section: None,
                record: record.clone(),
                list: "usage".to_string(),
                index: 0,
                attr: "name".to_string(),
                value: None,
            },
            json!({ "set_list_item": { "record": { "key": "FieldCanteen" }, "list": "usage", "index": 0, "attr": "name", "value": null } }),
        ),
        (
            CeOp::AddNestedItem {
                section: None,
                record: record.clone(),
                list: "spawnableTypes".to_string(),
                index: 0,
                attrs: vec![("name".to_string(), "Door".to_string())],
                nested_index: None,
            },
            json!({ "add_nested_item": { "record": { "key": "FieldCanteen" }, "list": "spawnableTypes", "index": 0, "attrs": [["name", "Door"]] } }),
        ),
        (
            CeOp::RemoveNestedItem {
                section: None,
                record: record.clone(),
                list: "spawnableTypes".to_string(),
                index: 0,
                nested_index: 1,
            },
            json!({ "remove_nested_item": { "record": { "key": "FieldCanteen" }, "list": "spawnableTypes", "index": 0, "nested_index": 1 } }),
        ),
        (
            CeOp::SetNestedItem {
                section: None,
                record: record.clone(),
                list: "spawnableTypes".to_string(),
                index: 0,
                nested_index: 1,
                attr: "name".to_string(),
                value: Some("Coast".to_string()),
            },
            json!({ "set_nested_item": { "record": { "key": "FieldCanteen" }, "list": "spawnableTypes", "index": 0, "nested_index": 1, "attr": "name", "value": "Coast" } }),
        ),
        (
            CeOp::Xml(modfoundry_ce::edit::XmlOp::SetAttr {
                element: modfoundry_ce::edit::XmlPath(vec![modfoundry_ce::edit::Step {
                    name: "type".to_string(),
                    pick: modfoundry_ce::edit::Pick::Nth(0),
                }]),
                name: "name".to_string(),
                value: "FieldCanteen".to_string(),
            }),
            json!({ "xml": { "set_attr": { "element": [{ "name": "type", "pick": { "nth": 0 } }], "name": "name", "value": "FieldCanteen" } } }),
        ),
    ];

    let schema = common::core().command("ce.edit").unwrap().args_schema();
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
            serde_json::from_value::<CeOp>(wire.clone()).unwrap(),
            *op,
            "{wire}"
        );
        tags.push(wire.as_object().unwrap().keys().next().unwrap().as_str());
    }
    // `wrap_root` is a unit op: a bare tag when serialized, and it also loads
    // from the documented `{"wrap_root": null}` form.
    assert_eq!(
        serde_json::to_value(CeOp::WrapRoot).unwrap(),
        json!("wrap_root")
    );
    tags.push("wrap_root");
    tags.sort();
    tags.dedup();
    assert_eq!(documented, tags, "the schema documents every op variant");
    assert_eq!(schema["properties"]["preview"]["type"], "boolean");
}

fn tag_of(op: &CeOp) -> &'static str {
    match op {
        CeOp::SetField { .. } => "set_field",
        CeOp::AddRecord { .. } => "add_record",
        CeOp::RemoveRecord { .. } => "remove_record",
        CeOp::RenameRecord { .. } => "rename_record",
        CeOp::DuplicateRecord { .. } => "duplicate_record",
        CeOp::AddListItem { .. } => "add_list_item",
        CeOp::RemoveListItem { .. } => "remove_list_item",
        CeOp::SetListItem { .. } => "set_list_item",
        CeOp::AddNestedItem { .. } => "add_nested_item",
        CeOp::RemoveNestedItem { .. } => "remove_nested_item",
        CeOp::SetNestedItem { .. } => "set_nested_item",
        CeOp::WrapRoot => "wrap_root",
        CeOp::Xml(_) => "xml",
    }
}

#[test]
fn the_ce_edit_schema_names_every_op() {
    let record = ref_of("FieldCanteen");
    let samples = [
        CeOp::SetField {
            section: None,
            record: record.clone(),
            field: "nominal".to_string(),
            value: Some("12".to_string()),
        },
        CeOp::AddRecord {
            section: None,
            key: "NewGun".to_string(),
            element: None,
            fields: Vec::new(),
            lists: Vec::new(),
            position: RecordPosition::Last,
        },
        CeOp::RemoveRecord {
            section: None,
            record: record.clone(),
        },
        CeOp::RenameRecord {
            section: None,
            record: record.clone(),
            new_key: "FieldBottle".to_string(),
        },
        CeOp::DuplicateRecord {
            section: None,
            record: record.clone(),
            new_key: "FieldCanteenCopy".to_string(),
        },
        CeOp::AddListItem {
            section: None,
            record: record.clone(),
            list: "usage".to_string(),
            attrs: Vec::new(),
            index: None,
        },
        CeOp::RemoveListItem {
            section: None,
            record: record.clone(),
            list: "usage".to_string(),
            index: 0,
        },
        CeOp::SetListItem {
            section: None,
            record: record.clone(),
            list: "usage".to_string(),
            index: 0,
            attr: "name".to_string(),
            value: None,
        },
        CeOp::AddNestedItem {
            section: None,
            record: record.clone(),
            list: "spawnableTypes".to_string(),
            index: 0,
            attrs: vec![("name".to_string(), "Door".to_string())],
            nested_index: None,
        },
        CeOp::RemoveNestedItem {
            section: None,
            record: record.clone(),
            list: "spawnableTypes".to_string(),
            index: 0,
            nested_index: 0,
        },
        CeOp::SetNestedItem {
            section: None,
            record: record.clone(),
            list: "spawnableTypes".to_string(),
            index: 0,
            nested_index: 0,
            attr: "name".to_string(),
            value: None,
        },
        CeOp::WrapRoot,
        CeOp::Xml(modfoundry_ce::edit::XmlOp::RemoveElement {
            element: modfoundry_ce::edit::XmlPath(Vec::new()),
        }),
    ];

    let schema = common::core().command("ce.edit").unwrap().args_schema();
    let documented: Vec<&str> = schema["properties"]["ops"]["items"]["oneOf"]
        .as_array()
        .expect("the ops schema is a oneOf")
        .iter()
        .map(|branch| branch["required"][0].as_str().unwrap())
        .collect();

    for op in &samples {
        let wire = serde_json::to_value(op).unwrap();
        let tag = match &wire {
            Value::String(tag) => tag.as_str(),
            Value::Object(fields) => fields.keys().next().unwrap().as_str(),
            other => panic!("unexpected op json: {other}"),
        };
        assert_eq!(tag, tag_of(op), "{tag} does not match its variant");
        assert!(
            documented.contains(&tag),
            "{tag} is missing from the schema: {documented:?}"
        );
    }
    assert_eq!(
        documented.len(),
        samples.len(),
        "one schema branch per op: {documented:?}"
    );
}
