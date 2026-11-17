//! Record-level ops on lists that live in a container element — the
//! event `children`, and the event-spawns lists. Each test compiles a [`CeOp`]
//! against a file and applies it through `ops::plan_all`, comparing the whole
//! text with a golden string.

use modfoundry_ce::edit::EditError;
use modfoundry_ce::ops::{plan_all, CeOp, RecordRef};

const EVENT_WITH_CHILDREN: &str = "<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n            <child type=\"A\"/>\n        </children>\n    </event>\n</events>\n";

const EVENT_EMPTY_CHILDREN: &str = "<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children/>\n    </event>\n</events>\n";

const EVENT_NO_CHILDREN: &str = "<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n    </event>\n</events>\n";

const EVENT_SPAWNS: &str = "<eventposdef>\n    <event name=\"Alpha\">\n        <pos x=\"1.5\" z=\"2.5\"/>\n    </event>\n</eventposdef>\n";

const EMPTY_EVENTS: &str = "<events>\n</events>\n";

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

fn add_item(key: &str, list: &str, pairs: &[(&str, &str)]) -> CeOp {
    CeOp::AddListItem {
        section: None,
        record: reference(key),
        list: list.to_string(),
        attrs: attrs(pairs),
        index: None,
    }
}

fn set_field(key: &str, field: &str, value: &str) -> CeOp {
    CeOp::SetField {
        section: None,
        record: reference(key),
        field: field.to_string(),
        value: Some(value.to_string()),
    }
}

#[test]
fn add_child_to_an_event_with_children_appends_inside() {
    let op = add_item("Alpha", "children", &[("type", "B")]);
    assert_eq!(
        plan_all(EVENT_WITH_CHILDREN, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n            <child type=\"A\"/>\n            <child type=\"B\"/>\n        </children>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn add_child_to_an_empty_children_container_fills_it() {
    let op = add_item("Alpha", "children", &[("type", "B")]);
    assert_eq!(
        plan_all(EVENT_EMPTY_CHILDREN, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n            <child type=\"B\"/>\n        </children>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn add_child_to_an_event_with_no_children_creates_the_container_last() {
    let op = add_item("Alpha", "children", &[("type", "B")]);
    assert_eq!(
        plan_all(EVENT_NO_CHILDREN, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n            <child type=\"B\"/>\n        </children>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn remove_the_only_child_leaves_the_empty_container() {
    let op = CeOp::RemoveListItem {
        section: None,
        record: reference("Alpha"),
        list: "children".to_string(),
        index: 0,
    };
    assert_eq!(
        plan_all(EVENT_WITH_CHILDREN, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n        </children>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn set_position_to_player() {
    let op = set_field("Alpha", "position", "player");
    assert_eq!(
        plan_all(EVENT_NO_CHILDREN, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"Alpha\">\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>player</position>\n        <limit>child</limit>\n        <active>0</active>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn a_bad_limit_is_refused_with_the_value_list() {
    let op = set_field("Alpha", "limit", "nope");
    assert_eq!(
        plan_all(EVENT_NO_CHILDREN, Some("events.xml"), &[op]),
        Err(EditError::InvalidValue(
            "Limit must be one of `child`, `parent`, `mixed`, `custom`.".to_string()
        ))
    );
}

#[test]
fn add_pos_to_an_event_spawns_record() {
    let op = add_item("Alpha", "pos", &[("x", "10"), ("z", "20"), ("a", "90")]);
    assert_eq!(
        plan_all(EVENT_SPAWNS, Some("cfgeventspawns.xml"), &[op]),
        Ok("<eventposdef>\n    <event name=\"Alpha\">\n        <pos x=\"1.5\" z=\"2.5\"/>\n        <pos x=\"10\" z=\"20\" a=\"90\"/>\n    </event>\n</eventposdef>\n".to_string())
    );
}

#[test]
fn add_record_of_an_event_writes_a_children_container() {
    let op = CeOp::AddRecord {
        section: None,
        key: "NewEvent".to_string(),
        element: None,
        fields: Vec::new(),
        lists: vec![modfoundry_ce::ops::NewList {
            list: "children".to_string(),
            items: vec![modfoundry_ce::ops::NewItem {
                attrs: attrs(&[
                    ("lootmax", "0"),
                    ("lootmin", "0"),
                    ("max", "2"),
                    ("min", "1"),
                    ("type", "Wreck"),
                ]),
                nested: Vec::new(),
            }],
        }],
        position: Default::default(),
    };
    assert_eq!(
        plan_all(EMPTY_EVENTS, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"NewEvent\">\n        <nominal>0</nominal>\n        <min>0</min>\n        <max>0</max>\n        <lifetime>300</lifetime>\n        <restock>0</restock>\n        <saferadius>0</saferadius>\n        <distanceradius>0</distanceradius>\n        <cleanupradius>0</cleanupradius>\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n        <children>\n            <child lootmax=\"0\" lootmin=\"0\" max=\"2\" min=\"1\" type=\"Wreck\"/>\n        </children>\n    </event>\n</events>\n".to_string())
    );
}

#[test]
fn add_record_of_an_event_gives_the_template_in_vanilla_order() {
    let op = CeOp::AddRecord {
        section: None,
        key: "NewEvent".to_string(),
        element: None,
        fields: Vec::new(),
        lists: Vec::new(),
        position: Default::default(),
    };
    assert_eq!(
        plan_all(EMPTY_EVENTS, Some("events.xml"), &[op]),
        Ok("<events>\n    <event name=\"NewEvent\">\n        <nominal>0</nominal>\n        <min>0</min>\n        <max>0</max>\n        <lifetime>300</lifetime>\n        <restock>0</restock>\n        <saferadius>0</saferadius>\n        <distanceradius>0</distanceradius>\n        <cleanupradius>0</cleanupradius>\n        <flags deletable=\"0\" init_random=\"0\" remove_damaged=\"0\"/>\n        <position>fixed</position>\n        <limit>child</limit>\n        <active>0</active>\n    </event>\n</events>\n".to_string())
    );
}
