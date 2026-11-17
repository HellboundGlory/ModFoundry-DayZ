//! List items nested inside a list item — the `<item>`s of a spawnable
//! type's `cargo` or `attachments`. Each test plans [`CeOp`]s over a file and
//! compares the whole text with a golden string.

use modfoundry_ce::edit::EditError;
use modfoundry_ce::ops::{plan_all, CeOp, NewItem, NewList, RecordPosition, RecordRef};

const SPAWNABLE: &str = "<spawnabletypes>\n    <type name=\"CargoItems\">\n        <cargo chance=\"0.5\">\n            <item name=\"Ammo_762x39\" chance=\"40.00\"/>\n            <item name=\"Rope\" chance=\"0.5\"/>\n        </cargo>\n    </type>\n    <type name=\"Hoarder\">\n        <hoarder/>\n    </type>\n    <type name=\"Blank\"/>\n</spawnabletypes>\n";

const PRESETS_EMPTY: &str = "<randompresets>\n</randompresets>\n";

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

fn add_nested(
    key: &str,
    list: &str,
    index: usize,
    pairs: &[(&str, &str)],
    nested_index: Option<usize>,
) -> CeOp {
    CeOp::AddNestedItem {
        section: None,
        record: reference(key),
        list: list.to_string(),
        index,
        attrs: attrs(pairs),
        nested_index,
    }
}

fn remove_nested(key: &str, list: &str, index: usize, nested_index: usize) -> CeOp {
    CeOp::RemoveNestedItem {
        section: None,
        record: reference(key),
        list: list.to_string(),
        index,
        nested_index,
    }
}

fn set_nested(
    key: &str,
    list: &str,
    index: usize,
    nested_index: usize,
    attr: &str,
    value: Option<&str>,
) -> CeOp {
    CeOp::SetNestedItem {
        section: None,
        record: reference(key),
        list: list.to_string(),
        index,
        nested_index,
        attr: attr.to_string(),
        value: value.map(str::to_string),
    }
}

fn set_field(key: &str, field: &str, value: Option<&str>) -> CeOp {
    CeOp::SetField {
        section: None,
        record: reference(key),
        field: field.to_string(),
        value: value.map(str::to_string),
    }
}

fn add_list(key: &str, list: &str, pairs: &[(&str, &str)]) -> CeOp {
    CeOp::AddListItem {
        section: None,
        record: reference(key),
        list: list.to_string(),
        attrs: attrs(pairs),
        index: None,
    }
}

fn run(text: &str, file_name: &str, op: &CeOp) -> String {
    plan_all(text, Some(file_name), std::slice::from_ref(op)).expect("plan")
}

fn err(text: &str, file_name: &str, op: &CeOp) -> EditError {
    plan_all(text, Some(file_name), std::slice::from_ref(op)).expect_err("expected an error")
}

#[test]
fn add_an_item_to_an_inline_cargo() {
    let out = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &add_nested(
            "CargoItems",
            "cargo",
            0,
            &[("name", "Battery"), ("chance", "1")],
            None,
        ),
    );
    assert_eq!(
        out,
        SPAWNABLE.replacen(
            "            <item name=\"Rope\" chance=\"0.5\"/>\n",
            "            <item name=\"Rope\" chance=\"0.5\"/>\n            <item name=\"Battery\" chance=\"1\"/>\n",
            1
        )
    );
}

#[test]
fn a_new_attachments_with_one_item_through_two_ops() {
    let ops = vec![
        add_list("Blank", "attachments", &[("chance", "0.5")]),
        add_nested(
            "Blank",
            "attachments",
            0,
            &[("name", "Scope"), ("chance", "0.5")],
            None,
        ),
    ];
    let out = plan_all(SPAWNABLE, Some("cfgspawnabletypes.xml"), &ops).unwrap();
    assert_eq!(
        out,
        SPAWNABLE.replacen(
            "    <type name=\"Blank\"/>",
            "    <type name=\"Blank\">\n        <attachments chance=\"0.5\">\n            <item name=\"Scope\" chance=\"0.5\"/>\n        </attachments>\n    </type>",
            1
        )
    );
}

#[test]
fn set_an_items_chance() {
    let out = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &set_nested("CargoItems", "cargo", 0, 0, "chance", Some("0.25")),
    );
    assert_eq!(
        out,
        SPAWNABLE.replacen(
            "<item name=\"Ammo_762x39\" chance=\"40.00\"/>",
            "<item name=\"Ammo_762x39\" chance=\"0.25\"/>",
            1
        )
    );
}

#[test]
fn remove_an_item_takes_its_line() {
    let out = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &remove_nested("CargoItems", "cargo", 0, 0),
    );
    assert_eq!(
        out,
        SPAWNABLE.replacen(
            "            <item name=\"Ammo_762x39\" chance=\"40.00\"/>\n",
            "",
            1
        )
    );
}

#[test]
fn set_hoarder_on_and_off() {
    let on = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &set_field("Blank", "hoarder", Some("1")),
    );
    assert_eq!(
        on,
        SPAWNABLE.replacen(
            "    <type name=\"Blank\"/>",
            "    <type name=\"Blank\">\n        <hoarder/>\n    </type>",
            1
        )
    );

    let off = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &set_field("Hoarder", "hoarder", Some("0")),
    );
    assert_eq!(off, SPAWNABLE.replacen("        <hoarder/>\n", "", 1));
}

#[test]
fn set_damage_min_writes_only_min() {
    let out = run(
        SPAWNABLE,
        "cfgspawnabletypes.xml",
        &set_field("Blank", "damage_min", Some("0.3")),
    );
    assert_eq!(
        out,
        SPAWNABLE.replacen(
            "    <type name=\"Blank\"/>",
            "    <type name=\"Blank\">\n        <damage min=\"0.3\"/>\n    </type>",
            1
        )
    );
}

#[test]
fn index_out_of_range_on_the_outer_index() {
    assert_eq!(
        err(
            SPAWNABLE,
            "cfgspawnabletypes.xml",
            &remove_nested("CargoItems", "cargo", 5, 0),
        ),
        EditError::IndexOutOfRange { len: 1, index: 5 }
    );
}

#[test]
fn index_out_of_range_on_the_nested_index() {
    assert_eq!(
        err(
            SPAWNABLE,
            "cfgspawnabletypes.xml",
            &remove_nested("CargoItems", "cargo", 0, 7),
        ),
        EditError::IndexOutOfRange { len: 2, index: 7 }
    );
    assert_eq!(
        err(
            SPAWNABLE,
            "cfgspawnabletypes.xml",
            &add_nested("CargoItems", "cargo", 0, &[("name", "Battery")], Some(9)),
        ),
        EditError::IndexOutOfRange { len: 2, index: 9 }
    );
}

#[test]
fn add_a_preset_with_an_item() {
    let op = CeOp::AddRecord {
        section: None,
        key: "Scope".to_string(),
        element: Some("attachments".to_string()),
        fields: Vec::new(),
        lists: vec![NewList {
            list: "item".to_string(),
            items: vec![NewItem {
                attrs: attrs(&[("name", "B"), ("chance", "1")]),
                nested: Vec::new(),
            }],
        }],
        position: RecordPosition::Last,
    };
    assert_eq!(
        run(PRESETS_EMPTY, "cfgrandompresets.xml", &op),
        "<randompresets>\n    <attachments name=\"Scope\">\n        <item name=\"B\" chance=\"1\"/>\n    </attachments>\n</randompresets>\n"
    );
}
