//! Record-level ops, part 3 — list items, `WrapRoot`, `Xml` and `plan_all`.
//! Each test compiles a [`CeOp`] against a file and applies it through
//! `ops::plan_all`, comparing the whole text with a golden string.

use crate::common::fixtures::{crlf_tabs, ops_err};
use modfoundry_ce::edit::{EditError, Pick, Step, XmlOp, XmlPath};
use modfoundry_ce::ops::{plan_all, CeOp, RecordRef};

const WITH_USAGES: &str = "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n        <lifetime>14400</lifetime>\n        <restock>1800</restock>\n        <min>6</min>\n        <quantmin>0</quantmin>\n        <quantmax>40</quantmax>\n        <cost>100</cost>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"1\" deloot=\"0\"/>\n        <category name=\"tools\"/>\n        <usage name=\"Village\"/>\n        <usage name=\"Farm\"/>\n        <value name=\"Tier1\"/>\n    </type>\n</types>\n";

const NO_USAGE: &str = "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n        <lifetime>14400</lifetime>\n        <restock>1800</restock>\n        <min>6</min>\n        <quantmin>0</quantmin>\n        <quantmax>40</quantmax>\n        <cost>100</cost>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"1\" deloot=\"0\"/>\n        <category name=\"tools\"/>\n        <value name=\"Tier1\"/>\n    </type>\n</types>\n";

const ONLY_LIFETIME_FLAGS: &str = "<types>\n    <type name=\"FieldCanteen\">\n        <lifetime>14400</lifetime>\n        <flags deloot=\"1\"/>\n    </type>\n</types>\n";

const ECONOMY_CORE: &str = "<economycore>\n    <ce folder=\"custom\"/>\n</economycore>\n";

const FRAGMENT3: &str = "<type name=\"Alpha\">\n    <nominal>1</nominal>\n</type>\n<type name=\"Bravo\">\n    <nominal>2</nominal>\n</type>\n<type name=\"Charlie\">\n    <nominal>3</nominal>\n</type>\n";

const ROOTED: &str = "<types>\n    <type name=\"Alpha\"/>\n</types>\n";

const EMPTY_TYPES: &str = "<types>\n</types>\n";

fn reference(key: &str) -> RecordRef {
    RecordRef {
        key: key.to_string(),
        occurrence: 0,
    }
}

fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn add_item(
    section: Option<&str>,
    key: &str,
    list: &str,
    pairs: &[(&str, &str)],
    index: Option<usize>,
) -> CeOp {
    CeOp::AddListItem {
        section: section.map(str::to_string),
        record: reference(key),
        list: list.to_string(),
        attrs: attrs(pairs),
        index,
    }
}

fn remove_item(section: Option<&str>, key: &str, list: &str, index: usize) -> CeOp {
    CeOp::RemoveListItem {
        section: section.map(str::to_string),
        record: reference(key),
        list: list.to_string(),
        index,
    }
}

fn set_item(
    section: Option<&str>,
    key: &str,
    list: &str,
    index: usize,
    attr: &str,
    value: Option<&str>,
) -> CeOp {
    CeOp::SetListItem {
        section: section.map(str::to_string),
        record: reference(key),
        list: list.to_string(),
        index,
        attr: attr.to_string(),
        value: value.map(str::to_string),
    }
}

fn run(text: &str, file_name: &str, op: &CeOp) -> String {
    plan_all(text, Some(file_name), std::slice::from_ref(op)).expect("plan")
}

#[test]
fn add_usage_after_the_last_one() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &add_item(None, "FieldCanteen", "usage", &[("name", "Hunting")], None),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen(
            "        <usage name=\"Farm\"/>\n",
            "        <usage name=\"Farm\"/>\n        <usage name=\"Hunting\"/>\n",
            1
        )
    );
}

#[test]
fn add_usage_crlf_tab_style() {
    let text = crlf_tabs(WITH_USAGES);
    let out = run(
        &text,
        "types.xml",
        &add_item(None, "FieldCanteen", "usage", &[("name", "Hunting")], None),
    );
    let expected = WITH_USAGES.replacen(
        "        <usage name=\"Farm\"/>\n",
        "        <usage name=\"Farm\"/>\n        <usage name=\"Hunting\"/>\n",
        1,
    );
    assert_eq!(out, crlf_tabs(&expected));
}

#[test]
fn add_usage_lands_before_the_value_when_there_are_none() {
    let out = run(
        NO_USAGE,
        "types.xml",
        &add_item(None, "FieldCanteen", "usage", &[("name", "Hunting")], None),
    );
    assert_eq!(
        out,
        NO_USAGE.replacen(
            "        <value name=\"Tier1\"/>",
            "        <usage name=\"Hunting\"/>\n        <value name=\"Tier1\"/>",
            1
        )
    );
}

#[test]
fn add_usage_after_flags_when_only_lifetime_and_flags() {
    let out = run(
        ONLY_LIFETIME_FLAGS,
        "types.xml",
        &add_item(None, "FieldCanteen", "usage", &[("name", "Hunting")], None),
    );
    assert_eq!(
        out,
        ONLY_LIFETIME_FLAGS.replacen(
            "    </type>",
            "        <usage name=\"Hunting\"/>\n    </type>",
            1
        )
    );
}

#[test]
fn add_value_with_a_user_flag() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &add_item(None, "FieldCanteen", "value", &[("user", "Tier12")], None),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen(
            "        <value name=\"Tier1\"/>\n",
            "        <value name=\"Tier1\"/>\n        <value user=\"Tier12\"/>\n",
            1
        )
    );
}

#[test]
fn add_tag_lands_before_usage() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &add_item(None, "FieldCanteen", "tag", &[("name", "Hunter")], None),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen(
            "        <usage name=\"Village\"/>",
            "        <tag name=\"Hunter\"/>\n        <usage name=\"Village\"/>",
            1
        )
    );
}

#[test]
fn add_usage_at_an_index_goes_before_that_item() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &add_item(
            None,
            "FieldCanteen",
            "usage",
            &[("name", "Bunker")],
            Some(1),
        ),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen(
            "        <usage name=\"Farm\"/>",
            "        <usage name=\"Bunker\"/>\n        <usage name=\"Farm\"/>",
            1
        )
    );
}

#[test]
fn add_duplicate_usage_is_exists() {
    assert_eq!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &add_item(None, "FieldCanteen", "usage", &[("name", "Village")], None),
        ),
        EditError::Exists("`Village` is already listed.".into())
    );
}

#[test]
fn add_duplicate_usage_by_case_is_exists() {
    assert_eq!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &add_item(None, "FieldCanteen", "usage", &[("name", "village")], None),
        ),
        EditError::Exists("`Village` is already listed.".into())
    );
}

#[test]
fn add_unknown_attribute_is_not_found() {
    assert!(matches!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &add_item(None, "FieldCanteen", "usage", &[("nope", "x")], None),
        ),
        EditError::NotFound(_)
    ));
}

#[test]
fn remove_the_middle_usage_takes_its_line() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &remove_item(None, "FieldCanteen", "usage", 1),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen("        <usage name=\"Farm\"/>\n", "", 1)
    );
}

#[test]
fn remove_index_out_of_range() {
    assert_eq!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &remove_item(None, "FieldCanteen", "usage", 5),
        ),
        EditError::IndexOutOfRange { len: 2, index: 5 }
    );
}

#[test]
fn set_renames_a_usage() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &set_item(None, "FieldCanteen", "usage", 1, "name", Some("Meadow")),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen("<usage name=\"Farm\"/>", "<usage name=\"Meadow\"/>", 1)
    );
}

#[test]
fn set_onto_an_existing_usage_is_exists() {
    assert_eq!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &set_item(None, "FieldCanteen", "usage", 1, "name", Some("Village")),
        ),
        EditError::Exists("`Village` is already listed.".into())
    );
}

#[test]
fn set_bad_value_is_invalid() {
    assert!(matches!(
        ops_err(
            WITH_USAGES,
            "types.xml",
            &set_item(None, "FieldCanteen", "usage", 1, "nope", Some("x")),
        ),
        EditError::NotFound(_)
    ));
}

#[test]
fn set_none_removes_the_attribute() {
    let out = run(
        WITH_USAGES,
        "types.xml",
        &set_item(None, "FieldCanteen", "usage", 1, "name", None),
    );
    assert_eq!(
        out,
        WITH_USAGES.replacen("<usage name=\"Farm\"/>", "<usage/>", 1)
    );
}

#[test]
fn add_a_file_item_to_a_ce_record() {
    let out = run(
        ECONOMY_CORE,
        "cfgeconomycore.xml",
        &add_item(
            Some("ce"),
            "custom",
            "file",
            &[("name", "types.xml"), ("type", "types")],
            None,
        ),
    );
    assert_eq!(
        out,
        "<economycore>\n    <ce folder=\"custom\">\n        <file name=\"types.xml\" type=\"types\"/>\n    </ce>\n</economycore>\n"
    );
}

#[test]
fn add_a_file_item_with_a_bad_type_is_invalid() {
    assert!(matches!(
        ops_err(
            ECONOMY_CORE,
            "cfgeconomycore.xml",
            &add_item(
                Some("ce"),
                "custom",
                "file",
                &[("name", "types.xml"), ("type", "nope")],
                None,
            ),
        ),
        EditError::InvalidValue(_)
    ));
}

#[test]
fn wrap_root_wraps_a_fragment() {
    let out = run(FRAGMENT3, "types.xml", &CeOp::WrapRoot);
    let want = format!("<types>\n{}\n</types>\n", FRAGMENT3.trim_end_matches('\n'));
    assert_eq!(out, want);
}

#[test]
fn wrap_root_on_a_rooted_file_is_exists() {
    assert_eq!(
        ops_err(ROOTED, "types.xml", &CeOp::WrapRoot),
        EditError::Exists("This file already has its <types> root.".into())
    );
}

#[test]
fn xml_renames_through_unchanged() {
    let op = XmlOp::RenameElement {
        element: XmlPath(vec![
            Step {
                name: "types".to_string(),
                pick: Pick::Nth(0),
            },
            Step {
                name: "type".to_string(),
                pick: Pick::Nth(0),
            },
        ]),
        name: "typo".to_string(),
    };
    let ce = CeOp::Xml(op);
    let out = run(ROOTED, "types.xml", &ce);
    assert_eq!(out, ROOTED.replacen("    <type", "    <typo", 1));
}

#[test]
fn plan_all_applies_three_ops_in_sequence() {
    let ops = vec![
        CeOp::AddRecord {
            section: None,
            key: "Custom".to_string(),
            element: None,
            fields: Vec::new(),
            lists: Vec::new(),
            position: Default::default(),
        },
        CeOp::SetField {
            section: None,
            record: reference("Custom"),
            field: "nominal".to_string(),
            value: Some("12".to_string()),
        },
        add_item(None, "Custom", "usage", &[("name", "Village")], None),
    ];
    let out = plan_all(EMPTY_TYPES, Some("types.xml"), &ops).unwrap();
    let want = "<types>\n    <type name=\"Custom\">\n        <nominal>12</nominal>\n        <lifetime>14400</lifetime>\n        <restock>0</restock>\n        <min>0</min>\n        <quantmin>-1</quantmin>\n        <quantmax>-1</quantmax>\n        <cost>100</cost>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n        <usage name=\"Village\"/>\n    </type>\n</types>\n";
    assert_eq!(out, want);
}

#[test]
fn plan_all_returns_the_first_error() {
    let ops = vec![
        CeOp::AddRecord {
            section: None,
            key: "Custom".to_string(),
            element: None,
            fields: Vec::new(),
            lists: Vec::new(),
            position: Default::default(),
        },
        CeOp::SetField {
            section: None,
            record: reference("Custom"),
            field: "nominal".to_string(),
            value: Some("abc".to_string()),
        },
    ];
    assert_eq!(
        plan_all(EMPTY_TYPES, Some("types.xml"), &ops).unwrap_err(),
        EditError::InvalidValue("Nominal must be a whole number.".into())
    );
}
