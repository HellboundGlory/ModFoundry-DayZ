//! Record-level ops, part 1 — addressing and `set_field`. Each test
//! compiles a `CeOp` against a file and applies it through `edit::plan_all`,
//! comparing the whole text with a golden string.

use crate::common::fixtures::{crlf_tabs, ops_err, ops_run};
use modfoundry_ce::edit::EditError;
use modfoundry_ce::ops::{CeOp, RecordRef};

const TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<types>
    <type name="FieldCanteen">
        <nominal>12</nominal>
        <lifetime>14400</lifetime>
        <restock>1800</restock>
        <min>6</min>
        <quantmin>0</quantmin>
        <quantmax>40</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="0" count_in_hoarder="0" count_in_map="1" count_in_player="0" crafted="1" deloot="0"/>
        <category name="tools"/>
        <usage name="Village"/>
        <usage name="Farm"/>
        <value name="Tier1"/>
        <value user="Tier12"/>
    </type>
    <type name="FilterCartridge">
        <nominal>20</nominal>
        <lifetime>14400</lifetime>
        <restock>0</restock>
        <min>24</min>
        <quantmin>-1</quantmin>
        <quantmax>-1</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="1" count_in_hoarder="1" count_in_map="1" count_in_player="0" crafted="0" deloot="0"/>
        <category name="tools"/>
        <usage name="Military"/>
        <value name="Tier2"/>
        <value name="Tier3"/>
    </type>
    <type name="FieldCanteen_Olive">
        <nominal>6</nominal>
        <lifetime>14400</lifetime>
        <restock>1800</restock>
        <min>3</min>
        <quantmin>0</quantmin>
        <quantmax>40</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="0" count_in_hoarder="0" count_in_map="0" count_in_player="0" crafted="0" deloot="0"/>
        <category name="tools"/>
        <usage name="Military"/>
        <value name="Tier2"/>
        <value name="Tier4"/>
    </type>
</types>
"#;

const LIFETIME_FLAGS: &str = "<types>\n    <type name=\"Sparse\">\n        <lifetime>60</lifetime>\n        <flags deloot=\"1\"/>\n    </type>\n</types>\n";

const NO_FLAGS: &str =
    "<types>\n    <type name=\"NoFlags\">\n        <nominal>8</nominal>\n    </type>\n</types>\n";

const NO_CATEGORY: &str = "<types>\n    <type name=\"NoCategory\">\n        <nominal>4</nominal>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n        <tag name=\"floor\"/>\n        <usage name=\"Village\"/>\n        <value name=\"Tier1\"/>\n    </type>\n</types>\n";

const BARE: &str =
    "<types>\n    <type name=\"Bare\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";

const DUP: &str = "<types>\n    <type name=\"Dup\">\n        <nominal>1</nominal>\n    </type>\n    <type name=\"Dup\">\n        <nominal>2</nominal>\n    </type>\n</types>\n";

const ECONOMY_CORE: &str = "<economycore>\n    <defaults>\n        <default name=\"log_ce_loop\" value=\"0\"/>\n    </defaults>\n</economycore>\n";

const UNPARSEABLE: &str = "<types>\n    <type name=\"A\"><nominal>1</nominal></type>\n";

const NO_KIND: &str = "<nothing>\n    <x/>\n</nothing>\n";

fn set(
    section: Option<&str>,
    key: &str,
    occurrence: usize,
    field: &str,
    value: Option<&str>,
) -> CeOp {
    CeOp::SetField {
        section: section.map(str::to_string),
        record: RecordRef {
            key: key.into(),
            occurrence,
        },
        field: field.into(),
        value: value.map(str::to_string),
    }
}

#[test]
fn change_nominal() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "FieldCanteen", 0, "nominal", Some("20")),
    );
    assert_eq!(
        out,
        TYPES.replace("<nominal>12</nominal>", "<nominal>20</nominal>")
    );
}

#[test]
fn change_count_in_cargo() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "FieldCanteen", 0, "count_in_cargo", Some("1")),
    );
    assert_eq!(
        out,
        TYPES.replacen("count_in_cargo=\"0\"", "count_in_cargo=\"1\"", 1)
    );
}

#[test]
fn change_category() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "FieldCanteen", 0, "category", Some("weapons")),
    );
    assert_eq!(
        out,
        TYPES.replacen(
            "<category name=\"tools\"/>",
            "<category name=\"weapons\"/>",
            1
        )
    );
}

#[test]
fn add_min_lands_between_lifetime_and_flags() {
    let out = ops_run(
        LIFETIME_FLAGS,
        "types.xml",
        &set(None, "Sparse", 0, "min", Some("5")),
    );
    assert_eq!(
        out,
        "<types>\n    <type name=\"Sparse\">\n        <lifetime>60</lifetime>\n        <min>5</min>\n        <flags deloot=\"1\"/>\n    </type>\n</types>\n"
    );
}

#[test]
fn set_flag_builds_a_complete_flags_element() {
    let out = ops_run(
        NO_FLAGS,
        "types.xml",
        &set(None, "NoFlags", 0, "count_in_map", Some("0")),
    );
    assert_eq!(
        out,
        "<types>\n    <type name=\"NoFlags\">\n        <nominal>8</nominal>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"0\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n    </type>\n</types>\n"
    );
}

#[test]
fn add_category_lands_after_flags_before_tag() {
    let out = ops_run(
        NO_CATEGORY,
        "types.xml",
        &set(None, "NoCategory", 0, "category", Some("tools")),
    );
    assert_eq!(
        out,
        NO_CATEGORY.replace(
            "        <tag name=\"floor\"/>",
            "        <category name=\"tools\"/>\n        <tag name=\"floor\"/>"
        )
    );
}

#[test]
fn remove_restock_takes_its_line() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "FieldCanteen", 0, "restock", None),
    );
    assert_eq!(
        out,
        TYPES.replacen("        <restock>1800</restock>\n", "", 1)
    );
}

#[test]
fn remove_category_takes_its_line() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "FieldCanteen", 0, "category", None),
    );
    assert_eq!(
        out,
        TYPES.replacen("        <category name=\"tools\"/>\n", "", 1)
    );
}

#[test]
fn remove_absent_field_changes_nothing() {
    let out = ops_run(BARE, "types.xml", &set(None, "Bare", 0, "restock", None));
    assert_eq!(out, BARE);
}

#[test]
fn invalid_value_messages() {
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "FieldCanteen", 0, "nominal", Some("abc"))
        ),
        EditError::InvalidValue("Nominal must be a whole number.".into())
    );
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "FieldCanteen", 0, "quantmin", Some("101"))
        ),
        EditError::InvalidValue("Quantity min must be a percentage from -1 to 100.".into())
    );
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "FieldCanteen", 0, "count_in_map", Some("2"))
        ),
        EditError::InvalidValue("count_in_map must be 0 or 1.".into())
    );
}

#[test]
fn not_found_names_the_thing() {
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "Foo", 0, "nominal", Some("1"))
        ),
        EditError::NotFound("No type named `Foo` here.".into())
    );
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "FieldCanteen", 0, "nope", Some("1"))
        ),
        EditError::NotFound("No field named `nope` here.".into())
    );
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(Some("nope"), "FieldCanteen", 0, "nominal", Some("1"))
        ),
        EditError::NotFound("No section named `nope` here.".into())
    );
}

#[test]
fn unparseable_on_unclosed_tag() {
    assert_eq!(
        ops_err(
            UNPARSEABLE,
            "types.xml",
            &set(None, "A", 0, "nominal", Some("1"))
        ),
        EditError::Unparseable
    );
}

#[test]
fn unsupported_without_a_kind() {
    assert_eq!(
        ops_err(
            NO_KIND,
            "unknown.xml",
            &set(None, "x", 0, "nominal", Some("1"))
        ),
        EditError::Unsupported("This file has no table view.".into())
    );
}

#[test]
fn occurrence_picks_the_nth_same_named_record() {
    let out = ops_run(DUP, "types.xml", &set(None, "Dup", 1, "nominal", Some("9")));
    assert_eq!(
        out,
        DUP.replace("<nominal>2</nominal>", "<nominal>9</nominal>")
    );
}

#[test]
fn key_match_is_case_insensitive() {
    let out = ops_run(
        TYPES,
        "types.xml",
        &set(None, "fieldcanteen", 0, "nominal", Some("20")),
    );
    assert_eq!(
        out,
        TYPES.replace("<nominal>12</nominal>", "<nominal>20</nominal>")
    );
}

#[test]
fn set_record_attribute_field() {
    let out = ops_run(
        ECONOMY_CORE,
        "cfgeconomycore.xml",
        &set(Some("defaults"), "log_ce_loop", 0, "value", Some("1")),
    );
    assert_eq!(out, ECONOMY_CORE.replace("value=\"0\"", "value=\"1\""));
}

#[test]
fn renaming_the_key_attribute_is_refused() {
    assert_eq!(
        ops_err(
            TYPES,
            "types.xml",
            &set(None, "FieldCanteen", 0, "name", Some("Foo"))
        ),
        EditError::Unsupported("Rename the entry instead.".into())
    );
}

#[test]
fn crlf_tab_style_is_preserved() {
    let text = crlf_tabs(TYPES);
    let out = ops_run(
        &text,
        "types.xml",
        &set(None, "FieldCanteen", 0, "nominal", Some("20")),
    );
    assert_eq!(
        out,
        crlf_tabs(TYPES).replace("<nominal>12</nominal>", "<nominal>20</nominal>")
    );
}

#[test]
fn crlf_tab_insert_uses_the_files_style() {
    let text = crlf_tabs(NO_FLAGS);
    let out = ops_run(
        &text,
        "types.xml",
        &set(None, "NoFlags", 0, "count_in_map", Some("0")),
    );
    assert_eq!(
        out,
        crlf_tabs(
            "<types>\n    <type name=\"NoFlags\">\n        <nominal>8</nominal>\n        <flags count_in_cargo=\"0\" count_in_hoarder=\"0\" count_in_map=\"0\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n    </type>\n</types>\n"
        )
    );
}

#[test]
fn ce_op_json_shape() {
    let op = set(None, "FieldCanteen", 0, "nominal", Some("12"));
    let json = serde_json::to_string(&op).unwrap();
    assert_eq!(
        json,
        r#"{"set_field":{"record":{"key":"FieldCanteen"},"field":"nominal","value":"12"}}"#
    );
    assert_eq!(serde_json::from_str::<CeOp>(&json).unwrap(), op);

    let op = set(Some("defaults"), "log_ce_loop", 1, "value", None);
    let json = serde_json::to_string(&op).unwrap();
    assert_eq!(
        json,
        r#"{"set_field":{"section":"defaults","record":{"key":"log_ce_loop","occurrence":1},"field":"value","value":null}}"#
    );
    assert_eq!(serde_json::from_str::<CeOp>(&json).unwrap(), op);
}
