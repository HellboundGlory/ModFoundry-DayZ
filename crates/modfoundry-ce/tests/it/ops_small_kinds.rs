//! Record ops over the small structured kinds — economy, globals,
//! messages and ignore list.

use crate::common::fixtures::{ops_err, ops_run};
use modfoundry_ce::edit::EditError;
use modfoundry_ce::ops::{plan_all as plan_ce, CeOp, RecordPosition, RecordRef};

const ECONOMY: &str = "<economy>\n    <dynamic init=\"1\" load=\"1\" respawn=\"1\" save=\"1\"/>\n    <animals init=\"1\" load=\"1\" respawn=\"1\" save=\"1\"/>\n    <zombies init=\"1\" load=\"1\" respawn=\"1\" save=\"1\"/>\n    <vehicles init=\"1\" load=\"1\" respawn=\"1\" save=\"0\"/>\n    <randoms init=\"0\" load=\"0\" respawn=\"0\" save=\"0\"/>\n    <custom init=\"0\" load=\"0\" respawn=\"0\" save=\"0\"/>\n    <building init=\"1\" load=\"1\" respawn=\"1\" save=\"1\"/>\n    <player init=\"0\" load=\"0\" respawn=\"0\" save=\"0\"/>\n</economy>\n";

const GLOBALS: &str = "<variables>\n    <var name=\"CleanupLifetimeDefault\" type=\"0\" value=\"45\"/>\n    <var name=\"ZombieMaxCount\" type=\"0\" value=\"1000\"/>\n</variables>\n";

const MESSAGES: &str = "<messages>\n    <message>\n        <delay>0</delay>\n        <repeat>0</repeat>\n        <deadline>5</deadline>\n        <onconnect>1</onconnect>\n        <shutdown>0</shutdown>\n        <text>Welcome</text>\n    </message>\n</messages>\n";

const ADDED_MESSAGE: &str = "<messages>\n    <message>\n        <delay>0</delay>\n        <repeat>0</repeat>\n        <deadline>5</deadline>\n        <onconnect>1</onconnect>\n        <shutdown>0</shutdown>\n        <text>Welcome</text>\n    </message>\n    <message>\n        <repeat>5</repeat>\n        <text></text>\n    </message>\n</messages>\n";

const IGNORE: &str = "<ignore>\n    <type name=\"CarScript\"/>\n</ignore>\n";
const IGNORE_TWO: &str =
    "<ignore>\n    <type name=\"CarScript\"/>\n    <type name=\"ZombieScript\"/>\n</ignore>\n";

fn set(section: Option<&str>, key: &str, field: &str, value: Option<&str>) -> CeOp {
    CeOp::SetField {
        section: section.map(str::to_string),
        record: RecordRef {
            key: key.to_string(),
            occurrence: 0,
        },
        field: field.to_string(),
        value: value.map(str::to_string),
    }
}

fn record(key: &str) -> RecordRef {
    RecordRef {
        key: key.to_string(),
        occurrence: 0,
    }
}

fn add(section: Option<&str>, key: &str, fields: &[(&str, &str)]) -> CeOp {
    CeOp::AddRecord {
        section: section.map(str::to_string),
        key: key.to_string(),
        element: None,
        fields: fields
            .iter()
            .map(|(id, value)| (id.to_string(), value.to_string()))
            .collect(),
        lists: Vec::new(),
        position: RecordPosition::default(),
    }
}

#[test]
fn flip_save_on_vehicles() {
    let out = ops_run(
        ECONOMY,
        "economy.xml",
        &set(None, "vehicles", "save", Some("1")),
    );
    assert_eq!(
        out,
        ECONOMY.replace("respawn=\"1\" save=\"0\"/>", "respawn=\"1\" save=\"1\"/>")
    );
}

#[test]
fn adding_an_economy_group_that_already_exists_is_refused() {
    assert_eq!(
        ops_err(ECONOMY, "economy.xml", &add(None, "dynamic", &[])),
        EditError::Exists("`dynamic` already has an entry.".into())
    );
}

#[test]
fn renaming_an_economy_group_is_refused() {
    let op = CeOp::RenameRecord {
        section: None,
        record: record("vehicles"),
        new_key: "animals".to_string(),
    };
    assert_eq!(
        ops_err(ECONOMY, "economy.xml", &op),
        EditError::Unsupported(
            "This kind names entries by their element, so rename isn't supported.".into()
        )
    );
}

#[test]
fn set_a_globals_value() {
    let out = ops_run(
        GLOBALS,
        "globals.xml",
        &set(None, "ZombieMaxCount", "value", Some("1500")),
    );
    assert_eq!(
        out,
        GLOBALS.replace(
            "name=\"ZombieMaxCount\" type=\"0\" value=\"1000\"",
            "name=\"ZombieMaxCount\" type=\"0\" value=\"1500\""
        )
    );
}

#[test]
fn adding_a_variable_writes_its_type_and_value() {
    let out = ops_run(
        GLOBALS,
        "globals.xml",
        &add(None, "X", &[("type", "0"), ("value", "5")]),
    );
    assert_eq!(
        out,
        GLOBALS.replace(
            "</variables>\n",
            "    <var name=\"X\" type=\"0\" value=\"5\"/>\n</variables>\n"
        )
    );
    assert!(matches!(
        ops_err(
            GLOBALS,
            "globals.xml",
            &add(None, "X", &[("type", "7"), ("value", "5")])
        ),
        EditError::InvalidValue(_)
    ));
}

#[test]
fn add_a_message_and_set_its_repeat() {
    let ops = [add(None, "#2", &[]), set(None, "#2", "repeat", Some("5"))];
    let out = plan_ce(MESSAGES, Some("messages.xml"), &ops).expect("plan");
    assert_eq!(out, ADDED_MESSAGE);
}

#[test]
fn adding_a_message_uses_the_text_template_only() {
    let out = ops_run(MESSAGES, "messages.xml", &add(None, "#2", &[]));
    assert_eq!(
        out,
        MESSAGES.replace(
            "</messages>\n",
            "    <message>\n        <text></text>\n    </message>\n</messages>\n"
        )
    );
}

#[test]
fn renaming_a_message_is_refused() {
    let op = CeOp::RenameRecord {
        section: None,
        record: record("#1"),
        new_key: "#2".to_string(),
    };
    assert_eq!(
        ops_err(MESSAGES, "messages.xml", &op),
        EditError::Unsupported(
            "This kind names entries by their element, so rename isn't supported.".into()
        )
    );
}

#[test]
fn add_an_ignore_list_type() {
    let out = ops_run(IGNORE, "cfgignorelist.xml", &add(None, "ZombieScript", &[]));
    assert_eq!(out, IGNORE_TWO);
}

#[test]
fn remove_an_ignore_list_type() {
    let out = ops_run(
        IGNORE_TWO,
        "cfgignorelist.xml",
        &CeOp::RemoveRecord {
            section: None,
            record: record("ZombieScript"),
        },
    );
    assert_eq!(out, IGNORE);
}
