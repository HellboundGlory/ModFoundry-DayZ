//! `ce.get_tree` and `ce.validate` over the registry.

use std::fs;

use crate::common::{self, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_core::{Actor, CommandError};
use serde_json::{json, Value};

const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<types>
    <type name="FieldCanteen">
        <nominal>12</nominal>
        <min>6</min>
    </type>
    <type name="FilterCartridge">
        <nominal>20</nominal>
        <min>24</min>
    </type>
    <type name="FieldCanteen_Olive">
        <nominal>6</nominal>
        <min>3</min>
    </type>
</types>
"#;

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

fn field<'a>(record: &'a Value, id: &str) -> &'a Value {
    record["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap_or_else(|| panic!("no field `{id}` in {record}"))
}

/// A one-record types file whose `min` is above its `nominal`, so it raises MF-X110.
fn bad_types(name: &str) -> String {
    format!(
        "<types>\n    <type name=\"{name}\">\n        <nominal>20</nominal>\n        <min>24</min>\n    </type>\n</types>\n"
    )
}

#[test]
fn get_tree_reads_a_types_file() {
    let dir = tmp("tree-types");
    let file = write(&dir, "types.xml", SAMPLE);
    let core = common::core();
    let tree = dispatch(&core, "ce.get_tree", json!({ "path": file }));

    assert_eq!(tree["kind"], "types");
    assert_eq!(tree["label"], "Types");
    assert_eq!(tree["structured"], true);
    assert_eq!(tree["fragment"], false);
    assert_eq!(tree["root_mismatch"], Value::Null);
    assert_eq!(tree["diagnostics"], json!([]));

    let sections = tree["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0]["id"], "types");
    let records = sections[0]["records"].as_array().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["key"], "FieldCanteen");
    assert_eq!(field(&records[0], "nominal")["raw"], "12");
    assert_eq!(field(&records[0], "min")["raw"], "6");
    assert_eq!(records[1]["key"], "FilterCartridge");
    assert_eq!(field(&records[1], "nominal")["raw"], "20");
    assert_eq!(field(&records[1], "min")["raw"], "24");
    assert_eq!(records[2]["key"], "FieldCanteen_Olive");
    assert_eq!(field(&records[2], "nominal")["raw"], "6");
}

#[test]
fn get_tree_reads_text_without_the_file() {
    let core = common::core();
    let path = tmp("tree-text").join("missing.xml");
    let tree = dispatch(
        &core,
        "ce.get_tree",
        json!({ "path": path, "text": SAMPLE }),
    );
    assert_eq!(tree["kind"], "types");
    assert_eq!(tree["sections"][0]["records"].as_array().unwrap().len(), 3);
}

#[test]
fn get_tree_marks_a_fragment() {
    let core = common::core();
    let text = "<type name=\"A\">\n    <nominal>1</nominal>\n</type>\n";
    let tree = dispatch(
        &core,
        "ce.get_tree",
        json!({ "path": "/tmp/fragment.xml", "text": text }),
    );
    assert_eq!(tree["kind"], "types");
    assert_eq!(tree["fragment"], true);
}

#[test]
fn get_tree_reports_records_and_the_xml_issue_for_an_unclosed_tag() {
    let core = common::core();
    let text =
        "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n    </type>\n";
    let tree = dispatch(
        &core,
        "ce.get_tree",
        json!({ "path": "/tmp/broken.xml", "text": text }),
    );
    assert_eq!(tree["kind"], "types");
    let records = tree["sections"][0]["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["key"], "FieldCanteen");
    let diags = tree["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["code"], "MF-X001");
}

#[test]
fn get_tree_returns_no_kind_for_other_xml() {
    let core = common::core();
    let tree = dispatch(
        &core,
        "ce.get_tree",
        json!({ "path": "/tmp/trader.xml", "text": "<Trader>\n    <a name=\"x\"/>\n</Trader>\n" }),
    );
    assert_eq!(tree["kind"], Value::Null);
    assert_eq!(tree["label"], Value::Null);
    assert_eq!(tree["structured"], false);
    assert_eq!(tree["sections"], json!([]));
}

#[test]
fn get_tree_on_a_missing_file_names_the_path() {
    let core = common::core();
    let path = tmp("tree-missing").join("gone.xml");
    let err = core
        .dispatch(Actor::User, "ce.get_tree", json!({ "path": path }))
        .unwrap_err();
    assert!(format!("{err}").contains(path.to_str().unwrap()), "{err}");
}

#[test]
fn validate_reports_the_file_finding_with_its_path() {
    let dir = tmp("validate-file");
    let file = write(&dir, "types.xml", SAMPLE);
    let core = common::core();
    let out = dispatch(&core, "ce.validate", json!({ "path": file }));
    let findings = out.as_array().unwrap();
    assert_eq!(findings.len(), 1, "{out}");
    assert_eq!(findings[0]["code"], "MF-X110");
    assert_eq!(findings[0]["path"], file.to_str().unwrap());
}

#[test]
fn validate_on_text_uses_the_text_not_the_file() {
    let dir = tmp("validate-text");
    let file = write(&dir, "types.xml", SAMPLE);
    let core = common::core();
    let clean = "<types>\n    <type name=\"A\">\n        <nominal>1</nominal>\n        <min>1</min>\n    </type>\n</types>\n";
    let out = dispatch(&core, "ce.validate", json!({ "path": file, "text": clean }));
    assert_eq!(out, json!([]));
}

#[test]
fn validate_walks_a_directory_for_ce_files_only() {
    let dir = tmp("validate-dir");
    let a = write(&dir, "a.xml", &bad_types("FieldCanteen"));
    let b = write(
        &dir,
        "nested/deep/deeper/b.xml",
        &bad_types("FilterCartridge"),
    );
    write(
        &dir,
        "trader.xml",
        "<Trader>\n    <a name=\"x\"/>\n</Trader>\n",
    );
    write(&dir, "builds/x.xml", &bad_types("Built"));
    write(&dir, ".git/y.xml", &bad_types("Hidden"));

    let core = common::core();
    let out = dispatch(&core, "ce.validate", json!({ "path": dir }));
    let findings = out.as_array().unwrap();
    let paths: Vec<&str> = findings
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, [a.to_str().unwrap(), b.to_str().unwrap()], "{out}");
    assert!(findings.iter().all(|f| f["code"] == "MF-X110"));
}

#[cfg(unix)]
#[test]
fn validate_skips_an_unreadable_file_in_a_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("validate-unreadable");
    let good = write(&dir, "a.xml", &bad_types("FieldCanteen"));
    let locked = write(&dir, "b.xml", &bad_types("FilterCartridge"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&locked).is_ok() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    let core = common::core();
    let out = dispatch(&core, "ce.validate", json!({ "path": dir }));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();

    let findings = out.as_array().unwrap();
    assert_eq!(findings.len(), 1, "{out}");
    assert_eq!(findings[0]["path"], good.to_str().unwrap());
}

#[test]
fn validate_rejects_text_with_a_directory() {
    let dir = tmp("validate-dir-text");
    std::fs::create_dir_all(&dir).unwrap();
    let core = common::core();
    let err = core
        .dispatch(
            Actor::User,
            "ce.validate",
            json!({ "path": dir, "text": "<types/>" }),
        )
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err}");
}

#[test]
fn ce_commands_leave_no_audit_row() {
    let dir = tmp("audit");
    let file = write(&dir, "types.xml", SAMPLE);
    let broker = Broker::in_memory(common::core()).unwrap();
    broker
        .dispatch(Actor::User, "ce.get_tree", json!({ "path": file }))
        .unwrap();
    broker
        .dispatch(Actor::User, "ce.validate", json!({ "path": file }))
        .unwrap();
    assert!(broker.audit(None, 10).unwrap().is_empty());
}
