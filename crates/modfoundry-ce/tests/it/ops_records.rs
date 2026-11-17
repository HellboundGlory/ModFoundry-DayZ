//! Record-level ops, part 2 — add, remove, rename and duplicate.
//!
//! Each test compiles a [`CeOp`] against a file and applies it through
//! `edit::plan_all`, comparing the whole text with a golden string.

use crate::common::fixtures::{crlf_tabs, ops_err, ops_run};
use modfoundry_ce::edit::EditError;
use modfoundry_ce::ops::{CeOp, NewItem, NewList, RecordPosition, RecordRef};

const WITH_ONE: &str =
    "<types>\n    <type name=\"Existing\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";
const EMPTY_TYPES: &str = "<types>\n</types>\n";
const SELF_CLOSING: &str = "<types/>\n";
const FRAGMENT: &str = "<type name=\"Alpha\">\n    <nominal>1</nominal>\n</type>\n";
const SORTED: &str = "<types>\n    <type name=\"Alpha\">\n        <nominal>1</nominal>\n    </type>\n    <type name=\"Charlie\">\n        <nominal>3</nominal>\n    </type>\n</types>\n";
const UNSORTED: &str = "<types>\n    <type name=\"Charlie\">\n        <nominal>3</nominal>\n    </type>\n    <type name=\"Alpha\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";
const COMMENTED: &str = "<types>\n    <!-- keep me -->\n    <type name=\"Alpha\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";
const TWO: &str = "<types>\n    <type name=\"Alpha\">\n        <nominal>1</nominal>\n    </type>\n    <type name=\"Bravo\">\n        <nominal>2</nominal>\n    </type>\n</types>\n";
const SOURCE: &str = "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n        <lifetime>14400</lifetime>\n        <restock>1800</restock>\n        <min>6</min>\n        <quantmin>0</quantmin>\n        <quantmax>40</quantmax>\n        <cost>100</cost>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"1\" deloot=\"0\"/>\n        <category name=\"tools\"/>\n        <usage name=\"Village\"/>\n        <value name=\"Tier1\"/>\n        <value user=\"Tier12\"/>\n    </type>\n</types>\n";
const LIMITS: &str = "<lists>\n    <categories>\n        <category name=\"food\"/>\n    </categories>\n    <usageflags>\n        <usage name=\"Military\"/>\n    </usageflags>\n    <valueflags/>\n</lists>\n";
const LIMITS_NO_USAGE: &str = "<lists>\n    <categories>\n        <category name=\"food\"/>\n    </categories>\n    <valueflags/>\n</lists>\n";

fn add(
    section: Option<&str>,
    key: &str,
    fields: &[(&str, &str)],
    lists: Vec<NewList>,
    position: RecordPosition,
) -> CeOp {
    CeOp::AddRecord {
        section: section.map(str::to_string),
        key: key.to_string(),
        element: None,
        fields: fields
            .iter()
            .map(|(id, value)| (id.to_string(), value.to_string()))
            .collect(),
        lists,
        position,
    }
}

fn remove(section: Option<&str>, key: &str) -> CeOp {
    CeOp::RemoveRecord {
        section: section.map(str::to_string),
        record: RecordRef {
            key: key.to_string(),
            occurrence: 0,
        },
    }
}

fn rename(section: Option<&str>, key: &str, new_key: &str) -> CeOp {
    CeOp::RenameRecord {
        section: section.map(str::to_string),
        record: RecordRef {
            key: key.to_string(),
            occurrence: 0,
        },
        new_key: new_key.to_string(),
    }
}

fn duplicate(section: Option<&str>, key: &str, new_key: &str) -> CeOp {
    CeOp::DuplicateRecord {
        section: section.map(str::to_string),
        record: RecordRef {
            key: key.to_string(),
            occurrence: 0,
        },
        new_key: new_key.to_string(),
    }
}

fn item(attrs: &[(&str, &str)]) -> NewItem {
    NewItem {
        attrs: attrs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        nested: Vec::new(),
    }
}

fn list(id: &str, items: Vec<NewItem>) -> NewList {
    NewList {
        list: id.to_string(),
        items,
    }
}

fn new_type(indent: &str, key: &str) -> String {
    format!(
        "{i}<type name=\"{key}\">\n\
         {i}    <nominal>0</nominal>\n\
         {i}    <lifetime>14400</lifetime>\n\
         {i}    <restock>0</restock>\n\
         {i}    <min>0</min>\n\
         {i}    <quantmin>-1</quantmin>\n\
         {i}    <quantmax>-1</quantmax>\n\
         {i}    <cost>100</cost>\n\
         {i}    <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n\
         {i}</type>\n",
        i = indent,
    )
}

#[test]
fn add_without_fields_gives_the_dormant_template() {
    let out = ops_run(
        WITH_ONE,
        "types.xml",
        &add(None, "NewItem", &[], Vec::new(), RecordPosition::default()),
    );
    assert_eq!(
        out,
        format!("<types>\n    <type name=\"Existing\">\n        <nominal>1</nominal>\n    </type>\n{}</types>\n", new_type("    ", "NewItem"))
    );
}

#[test]
fn add_dormant_template_crlf_tab_style() {
    let text = crlf_tabs(WITH_ONE);
    let out = ops_run(
        &text,
        "types.xml",
        &add(None, "NewItem", &[], Vec::new(), RecordPosition::default()),
    );
    let expected = format!("<types>\n    <type name=\"Existing\">\n        <nominal>1</nominal>\n    </type>\n{}</types>\n", new_type("    ", "NewItem"));
    assert_eq!(out, crlf_tabs(&expected));
}

#[test]
fn add_with_fields_and_lists() {
    let lists = vec![
        list(
            "usage",
            vec![item(&[("name", "Village")]), item(&[("name", "Farm")])],
        ),
        list("value", vec![item(&[("user", "Tier12")])]),
    ];
    let out = ops_run(
        EMPTY_TYPES,
        "types.xml",
        &add(
            None,
            "Custom",
            &[("nominal", "12"), ("category", "tools")],
            lists,
            RecordPosition::default(),
        ),
    );
    assert_eq!(
        out,
        "<types>\n    <type name=\"Custom\">\n        <nominal>12</nominal>\n        <lifetime>14400</lifetime>\n        <restock>0</restock>\n        <min>0</min>\n        <quantmin>-1</quantmin>\n        <quantmax>-1</quantmax>\n        <cost>100</cost>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n        <category name=\"tools\"/>\n        <usage name=\"Village\"/>\n        <usage name=\"Farm\"/>\n        <value user=\"Tier12\"/>\n    </type>\n</types>\n"
    );
}

#[test]
fn add_sorted_lands_in_place() {
    let out = ops_run(
        SORTED,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::Sorted),
    );
    assert_eq!(
        out,
        SORTED.replacen(
            "    <type name=\"Charlie\">\n",
            &format!("{}    <type name=\"Charlie\">\n", new_type("    ", "Bravo")),
            1
        )
    );
}

#[test]
fn add_sorted_into_unsorted_goes_last() {
    let out = ops_run(
        UNSORTED,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::Sorted),
    );
    assert_eq!(
        out,
        UNSORTED.replacen(
            "</types>\n",
            &format!("{}</types>\n", new_type("    ", "Bravo")),
            1
        )
    );
}

#[test]
fn add_first() {
    let out = ops_run(
        WITH_ONE,
        "types.xml",
        &add(None, "Alpha", &[], Vec::new(), RecordPosition::First),
    );
    assert_eq!(
        out,
        WITH_ONE.replacen(
            "    <type name=\"Existing\">\n",
            &format!(
                "{}    <type name=\"Existing\">\n",
                new_type("    ", "Alpha")
            ),
            1
        )
    );
}

#[test]
fn add_after_a_record() {
    let out = ops_run(
        SORTED,
        "types.xml",
        &add(
            None,
            "Bravo",
            &[],
            Vec::new(),
            RecordPosition::After(RecordRef {
                key: "Alpha".to_string(),
                occurrence: 0,
            }),
        ),
    );
    assert_eq!(
        out,
        SORTED.replacen(
            "    <type name=\"Charlie\">\n",
            &format!("{}    <type name=\"Charlie\">\n", new_type("    ", "Bravo")),
            1
        )
    );
}

#[test]
fn add_into_a_fragment_is_top_level() {
    let out = ops_run(
        FRAGMENT,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::default()),
    );
    assert_eq!(out, format!("{FRAGMENT}{}", new_type("", "Bravo")));
}

#[test]
fn add_into_empty_root() {
    let out = ops_run(
        EMPTY_TYPES,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::default()),
    );
    assert_eq!(
        out,
        format!("<types>\n{}</types>\n", new_type("    ", "Bravo"))
    );
}

#[test]
fn add_into_self_closing_root() {
    let out = ops_run(
        SELF_CLOSING,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::default()),
    );
    assert_eq!(
        out,
        format!("<types>\n{}</types>\n", new_type("    ", "Bravo"))
    );
}

#[test]
fn add_into_empty_file_is_unsupported() {
    assert_eq!(
        ops_err(
            "",
            "types.xml",
            &add(None, "Bravo", &[], Vec::new(), RecordPosition::default())
        ),
        EditError::Unsupported("Add the <types> root first.".into())
    );
}

#[test]
fn exists_on_an_existing_key_and_by_case() {
    assert_eq!(
        ops_err(
            WITH_ONE,
            "types.xml",
            &add(None, "Existing", &[], Vec::new(), RecordPosition::default())
        ),
        EditError::Exists("`Existing` already has an entry.".into())
    );
    assert_eq!(
        ops_err(
            WITH_ONE,
            "types.xml",
            &add(None, "existing", &[], Vec::new(), RecordPosition::default())
        ),
        EditError::Exists("`existing` already has an entry.".into())
    );
}

#[test]
fn invalid_class_name() {
    assert_eq!(
        ops_err(
            WITH_ONE,
            "types.xml",
            &add(None, "Bad Name", &[], Vec::new(), RecordPosition::default())
        ),
        EditError::InvalidName("A class name has letters, digits and `_` only.".into())
    );
}

#[test]
fn invalid_field_value_and_unknown_ids() {
    assert_eq!(
        ops_err(
            EMPTY_TYPES,
            "types.xml",
            &add(
                None,
                "Bravo",
                &[("nominal", "abc")],
                Vec::new(),
                RecordPosition::default()
            )
        ),
        EditError::InvalidValue("Nominal must be a whole number.".into())
    );
    assert_eq!(
        ops_err(
            EMPTY_TYPES,
            "types.xml",
            &add(
                None,
                "Bravo",
                &[("nope", "1")],
                Vec::new(),
                RecordPosition::default()
            )
        ),
        EditError::NotFound("No field named `nope` here.".into())
    );
    assert_eq!(
        ops_err(
            EMPTY_TYPES,
            "types.xml",
            &add(
                None,
                "Bravo",
                &[],
                vec![list("nope", Vec::new())],
                RecordPosition::default()
            )
        ),
        EditError::NotFound("No list named `nope` here.".into())
    );
}

#[test]
fn remove_takes_only_the_records_lines() {
    let out = ops_run(COMMENTED, "types.xml", &remove(None, "Alpha"));
    assert_eq!(out, "<types>\n    <!-- keep me -->\n</types>\n");
}

#[test]
fn rename_changes_only_the_name() {
    let out = ops_run(
        SOURCE,
        "types.xml",
        &rename(None, "FieldCanteen", "FieldCanteen_Base"),
    );
    assert_eq!(
        out,
        SOURCE.replace(
            "<type name=\"FieldCanteen\">",
            "<type name=\"FieldCanteen_Base\">"
        )
    );
}

#[test]
fn rename_clash_is_exists() {
    assert_eq!(
        ops_err(TWO, "types.xml", &rename(None, "Alpha", "Bravo")),
        EditError::Exists("`Bravo` already has an entry.".into())
    );
    assert_eq!(
        ops_err(TWO, "types.xml", &rename(None, "Alpha", "BRAVO")),
        EditError::Exists("`BRAVO` already has an entry.".into())
    );
}

#[test]
fn rename_to_a_different_casing_of_itself_works() {
    let out = ops_run(TWO, "types.xml", &rename(None, "Alpha", "alpha"));
    assert_eq!(
        out,
        TWO.replace("<type name=\"Alpha\">", "<type name=\"alpha\">")
    );
}

#[test]
fn duplicate_follows_the_source_byte_for_byte() {
    let out = ops_run(
        SOURCE,
        "types.xml",
        &duplicate(None, "FieldCanteen", "FieldCanteen_Copy"),
    );
    let block = SOURCE
        .strip_prefix("<types>\n")
        .unwrap()
        .strip_suffix("</types>\n")
        .unwrap();
    let copy = block.replace("name=\"FieldCanteen\"", "name=\"FieldCanteen_Copy\"");
    assert_eq!(out, format!("<types>\n{block}{copy}</types>\n"));
}

#[test]
fn duplicating_a_preset_keeps_its_chance() {
    const PRESETS: &str = "<randompresets>\n    <cargo chance=\"0.5\" name=\"foodHermit\">\n        <item name=\"Apple\" chance=\"0.3\"/>\n    </cargo>\n</randompresets>\n";
    let out = ops_run(
        PRESETS,
        "cfgrandompresets.xml",
        &duplicate(Some("presets"), "foodHermit", "foodHermit_Copy"),
    );
    assert_eq!(
        out,
        "<randompresets>\n    <cargo chance=\"0.5\" name=\"foodHermit\">\n        <item name=\"Apple\" chance=\"0.3\"/>\n    </cargo>\n    <cargo name=\"foodHermit_Copy\" chance=\"0.5\">\n        <item name=\"Apple\" chance=\"0.3\"/>\n    </cargo>\n</randompresets>\n"
    );
}

#[test]
fn adding_the_other_elements_preset_is_allowed() {
    const PRESETS: &str = "<randompresets>\n    <cargo name=\"x\"/>\n</randompresets>\n";
    let out = ops_run(
        PRESETS,
        "cfgrandompresets.xml",
        &CeOp::AddRecord {
            section: Some("presets".to_string()),
            key: "x".to_string(),
            element: Some("attachments".to_string()),
            fields: Vec::new(),
            lists: Vec::new(),
            position: RecordPosition::default(),
        },
    );
    assert_eq!(
        out,
        PRESETS.replace(
            "<cargo name=\"x\"/>",
            "<cargo name=\"x\"/>\n    <attachments name=\"x\"/>"
        )
    );
}

#[test]
fn limits_add_record_to_a_section() {
    let out = ops_run(
        LIMITS,
        "cfglimitsdefinition.xml",
        &add(
            Some("usageflags"),
            "Bunker",
            &[],
            Vec::new(),
            RecordPosition::default(),
        ),
    );
    assert_eq!(
        out,
        LIMITS.replace(
            "        <usage name=\"Military\"/>\n",
            "        <usage name=\"Military\"/>\n        <usage name=\"Bunker\"/>\n"
        )
    );
}

#[test]
fn limits_add_creates_the_missing_section_element() {
    let out = ops_run(
        LIMITS_NO_USAGE,
        "cfglimitsdefinition.xml",
        &add(
            Some("usageflags"),
            "Bunker",
            &[],
            Vec::new(),
            RecordPosition::default(),
        ),
    );
    assert_eq!(
        out,
        "<lists>\n    <categories>\n        <category name=\"food\"/>\n    </categories>\n    <usageflags>\n        <usage name=\"Bunker\"/>\n    </usageflags>\n    <valueflags/>\n</lists>\n"
    );
}

#[test]
fn add_then_remove_restores_the_input() {
    let added = ops_run(
        EMPTY_TYPES,
        "types.xml",
        &add(None, "Bravo", &[], Vec::new(), RecordPosition::default()),
    );
    let removed = ops_run(&added, "types.xml", &remove(None, "Bravo"));
    assert_eq!(removed, EMPTY_TYPES);
}

#[test]
fn json_round_trip_defaults_position_to_last() {
    let json = r#"{"add_record":{"key":"Bunker"}}"#;
    let op: CeOp = serde_json::from_str(json).unwrap();
    assert_eq!(
        op,
        add(None, "Bunker", &[], Vec::new(), RecordPosition::Last)
    );
    assert_eq!(
        serde_json::to_string(&op).unwrap(),
        r#"{"add_record":{"key":"Bunker","position":"last"}}"#
    );

    let op = CeOp::AddRecord {
        section: Some("usageflags".to_string()),
        key: "Bunker".to_string(),
        element: None,
        fields: vec![("name".to_string(), "Bunker".to_string())],
        lists: Vec::new(),
        position: RecordPosition::After(RecordRef {
            key: "Military".to_string(),
            occurrence: 1,
        }),
    };
    let json = serde_json::to_string(&op).unwrap();
    assert_eq!(
        json,
        r#"{"add_record":{"section":"usageflags","key":"Bunker","fields":[["name","Bunker"]],"position":{"after":{"key":"Military","occurrence":1}}}}"#
    );
    assert_eq!(serde_json::from_str::<CeOp>(&json).unwrap(), op);
}
