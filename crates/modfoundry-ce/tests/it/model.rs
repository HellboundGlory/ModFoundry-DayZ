//! The schema-driven record model — sections, records, fields and lists
//! read out of Central Economy XML.

use crate::common::fixtures::slice;
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::model::{read, record_at, Document};

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

const LIMITS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lists>
    <categories>
        <category name="tools"/>
        <category name="weapons"/>
    </categories>
    <tags>
        <tag name="floor"/>
    </tags>
    <usageflags>
        <usage name="Village"/>
        <usage name="Military"/>
        <usage name="Town"/>
    </usageflags>
    <valueflags>
        <value name="Tier1"/>
        <value name="Tier2"/>
    </valueflags>
</lists>
"#;

const LIMITS_NO_TAGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lists>
    <categories>
        <category name="tools"/>
    </categories>
    <usageflags>
        <usage name="Village"/>
    </usageflags>
    <valueflags>
        <value name="Tier1"/>
    </valueflags>
</lists>
"#;

const ECONOMY_CORE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<economycore>
    <classes>
        <rootclass name="CfgVehicles" act="car" reportMemoryLOD="no"/>
        <rootclass name="CfgNonAIVehicles" act="none" reportMemoryLOD="yes"/>
    </classes>
    <defaults>
        <default name="dyn" value="1"/>
    </defaults>
    <ce folder="mymod">
        <file name="types.xml" type="types"/>
        <file name="events.xml" type="events"/>
    </ce>
</economycore>
"#;

const SPARSE: &str = r#"<types>
    <type name="Empty">
        <lifetime>60</lifetime>
        <flags deloot="1"/>
    </type>
</types>
"#;

const INVALID: &str = r#"<types>
    <type name="Bad">
        <nominal>abc</nominal>
        <flags crafted="3"/>
    </type>
</types>
"#;

const TWO_A: &str = r#"<types>
    <type name="A"/>
    <type name="a"/>
</types>
"#;

const FRAGMENT: &str = r#"<type name="A"><nominal>1</nominal></type>
<type name="B"><nominal>2</nominal></type>
"#;

const SPAWNABLE_ROOT: &str = r#"<types>
    <type name="X"><cargo/></type>
</types>
"#;

const SPAWNABLE_TYPES: &str = r#"<spawnabletypes>
    <type name="Hoarder">
        <hoarder/>
    </type>
    <type name="Damage">
        <damage min="0.2" max="0.8"/>
    </type>
    <type name="CargoPreset">
        <cargo preset="SakhalCargo"/>
    </type>
    <type name="CargoItems">
        <cargo chance="0.5">
            <item name="Ammo_762x39" chance="40.00"/>
            <item name="Rope" chance="0.5"/>
        </cargo>
    </type>
    <type name="Attach">
        <attachments chance="1">
            <item name="Scope" chance="0.3"/>
        </attachments>
    </type>
</spawnabletypes>
"#;

const PRESETS: &str = r#"<randompresets>
    <cargo name="Ammo" chance="0.5">
        <item name="A" chance="1"/>
    </cargo>
    <attachments name="Scope" chance="0.3">
        <item name="B" chance="2"/>
    </attachments>
</randompresets>
"#;

const EVENTS: &str = r#"<events>
    <event name="StaticHeliCrash">
        <nominal>1</nominal>
        <min>0</min>
        <max>0</max>
        <lifetime>300</lifetime>
        <restock>0</restock>
        <saferadius>0</saferadius>
        <distanceradius>0</distanceradius>
        <cleanupradius>0</cleanupradius>
        <secondary>InfectedArmyConvoy</secondary>
        <flags deletable="0" init_random="0" remove_damaged="0"/>
        <position>fixed</position>
        <limit>child</limit>
        <active>0</active>
        <children>
            <child lootmax="0" lootmin="0" max="2" min="1" type="Wreck_UH1Y"/>
        </children>
    </event>
    <event name="Empty">
        <nominal>0</nominal>
        <flags deletable="0" init_random="0" remove_damaged="0"/>
        <position>player</position>
        <limit>mixed</limit>
        <active>0</active>
    </event>
</events>
"#;

const EVENT_SPAWNS: &str = r#"<eventposdef>
    <event name="StaticHeliCrash">
        <pos x="1.5" z="2.5" a="90" y="3.5" group="crash"/>
        <pos x="4" z="5"/>
        <zone smin="0" smax="1" dmin="2" dmax="3" r="4"/>
    </event>
</eventposdef>
"#;

const ECONOMY: &str = r#"<economy>
    <dynamic init="1" load="1" respawn="1" save="1"/>
    <animals init="1" load="1" respawn="1" save="1"/>
    <zombies init="1" load="1" respawn="1" save="1"/>
    <vehicles init="1" load="1" respawn="1" save="0"/>
    <randoms init="0" load="0" respawn="0" save="0"/>
    <custom init="0" load="0" respawn="0" save="0"/>
    <building init="1" load="1" respawn="1" save="1"/>
    <player init="0" load="0" respawn="0" save="0"/>
</economy>
"#;

const GLOBALS: &str = r#"<variables>
    <var name="CleanupLifetimeDefault" type="0" value="45"/>
    <var name="ZombieMaxCount" type="0" value="1000"/>
</variables>
"#;

const MESSAGES: &str = r#"<messages>
    <message>
        <delay>0</delay>
        <repeat>0</repeat>
        <deadline>5</deadline>
        <onconnect>1</onconnect>
        <shutdown>0</shutdown>
        <text>Welcome</text>
    </message>
</messages>
"#;

const IGNORE_LIST: &str = r#"<ignore>
    <type name="CarScript"/>
    <type name="ZombieBase"/>
</ignore>
"#;

const UNKNOWN_CHILD: &str = r#"<types>
    <type name="Odd">
        <nominal>5</nominal>
        <nominel>7</nominel>
    </type>
</types>
"#;

const BLOCKING: &str = r#"<types>
    <type name="A"><nominal>1</nominal></type>
"#;

fn line_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("fixture has no line with {needle:?}"))
        + 1
}

fn raw<'a>(document: &'a Document, section: &str, key: &str, field: &str) -> Option<&'a str> {
    document
        .find(section, key, 0)
        .unwrap()
        .field(field)
        .unwrap()
        .raw
        .as_deref()
}

#[test]
fn types_fixture_reads_kind_section_and_records() {
    let document = read(TYPES, Some("types.xml"));
    assert_eq!(document.kind, Some(CeKind::Types));
    assert!(!document.fragment);
    assert_eq!(document.root_mismatch, None);
    assert_eq!(document.issues, 0);
    assert!(!document.blocking);

    assert_eq!(document.sections.len(), 1);
    let section = document.section("types").unwrap();
    assert_eq!(section.id, "types");
    assert_eq!(section.label, "Types");
    assert_eq!(section.records.len(), 3);

    let keys: Vec<&str> = section.records.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        keys,
        ["FieldCanteen", "FilterCartridge", "FieldCanteen_Olive"]
    );
    for record in &section.records {
        assert_eq!(record.element, "type");
        assert_eq!(
            record.line,
            line_of(TYPES, &format!("name=\"{}\"", record.key))
        );
        assert!(record.unknown.is_empty());
    }
}

#[test]
fn every_field_has_its_raw_value_and_spans_slice_to_them() {
    let document = read(TYPES, Some("types.xml"));
    let record = document.find("types", "FieldCanteen", 0).unwrap();

    let expected = [
        ("nominal", "12"),
        ("min", "6"),
        ("lifetime", "14400"),
        ("restock", "1800"),
        ("quantmin", "0"),
        ("quantmax", "40"),
        ("cost", "100"),
        ("category", "tools"),
        ("count_in_cargo", "0"),
        ("count_in_hoarder", "0"),
        ("count_in_map", "1"),
        ("count_in_player", "0"),
        ("crafted", "1"),
        ("deloot", "0"),
    ];
    assert_eq!(record.fields.len(), expected.len());
    for (field, want) in expected {
        let value = record.field(field).unwrap();
        assert_eq!(value.raw.as_deref(), Some(want), "field {field}");
        assert!(value.valid, "field {field}");
        assert_eq!(slice(TYPES, value.span.unwrap()), want, "field {field}");
    }

    assert_eq!(slice(TYPES, record.key_span.unwrap()), "FieldCanteen");
    assert!(record.field("nope").is_none());

    let usage = record.list("usage").unwrap();
    assert_eq!(usage.items.len(), 2);
    let names: Vec<&str> = usage.items[0]
        .attrs
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["name"]);
    assert_eq!(usage.items[0].attrs[0].value, "Village");
    assert_eq!(
        slice(TYPES, usage.items[0].attrs[0].span),
        "Village",
        "attr span is the value"
    );
    assert!(usage.items[0].nested.is_empty());

    assert_eq!(record.names("usage"), ["Village", "Farm"]);
    assert_eq!(record.names("value"), ["Tier1", "Tier12"]);
    assert!(record.list("nope").is_none());
    assert!(record.field("nope").is_none());
}

#[test]
fn a_record_missing_fields_reads_them_as_absent_and_valid() {
    let document = read(SPARSE, None);
    let record = document.find("types", "Empty", 0).unwrap();
    assert_eq!(raw(&document, "types", "Empty", "lifetime"), Some("60"));
    assert_eq!(raw(&document, "types", "Empty", "deloot"), Some("1"));
    for field in ["nominal", "min", "restock", "quantmin", "quantmax", "cost"] {
        let value = record.field(field).unwrap();
        assert_eq!(value.raw, None, "field {field}");
        assert_eq!(value.span, None, "field {field}");
        assert!(value.valid, "field {field}");
    }
    // Flags exist as a child but the attribute is missing.
    let crafted = record.field("crafted").unwrap();
    assert_eq!(crafted.raw, None);
    assert!(crafted.valid);
}

#[test]
fn invalid_values_are_flagged() {
    let document = read(INVALID, None);
    let record = document.find("types", "Bad", 0).unwrap();

    let nominal = record.field("nominal").unwrap();
    assert_eq!(nominal.raw.as_deref(), Some("abc"));
    assert!(!nominal.valid);

    let crafted = record.field("crafted").unwrap();
    assert_eq!(crafted.raw.as_deref(), Some("3"));
    assert!(!crafted.valid);
}

#[test]
fn find_compares_keys_case_insensitively_by_occurrence() {
    let document = read(TWO_A, None);
    let section = document.section("types").unwrap();
    assert_eq!(section.records.len(), 2);
    assert_eq!(section.records[0].key, "A");
    assert_eq!(section.records[1].key, "a");

    assert_eq!(document.find("types", "A", 0).unwrap().key, "A");
    assert_eq!(document.find("types", "A", 1).unwrap().key, "a");
    assert_eq!(document.find("types", "a", 1).unwrap().key, "a");
    assert!(document.find("types", "A", 2).is_none());
    assert!(document.find("nope", "A", 0).is_none());
}

#[test]
fn a_fragment_has_top_level_records() {
    let document = read(FRAGMENT, None);
    assert_eq!(document.kind, Some(CeKind::Types));
    assert!(document.fragment);
    assert_eq!(document.root_mismatch, None);
    let section = document.section("types").unwrap();
    assert_eq!(section.records.len(), 2);
    let keys: Vec<&str> = section.records.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["A", "B"]);
}

#[test]
fn a_types_root_holding_spawnable_types_reports_a_mismatch_and_reads_them() {
    let document = read(SPAWNABLE_ROOT, None);
    assert_eq!(document.kind, Some(CeKind::SpawnableTypes));
    assert!(!document.fragment);
    assert_eq!(document.root_mismatch.as_deref(), Some("types"));
    let section = document.section("spawnabletypes").unwrap();
    assert_eq!(section.records.len(), 1);
    assert_eq!(section.records[0].key, "X");
    assert_eq!(section.records[0].list("cargo").unwrap().items.len(), 1);
}

#[test]
fn spawnable_types_read_every_field_list_and_nested_item() {
    let document = read(SPAWNABLE_TYPES, Some("cfgspawnabletypes.xml"));
    assert_eq!(document.kind, Some(CeKind::SpawnableTypes));
    let section = document.section("spawnabletypes").unwrap();
    assert_eq!(section.records.len(), 5);
    for record in &section.records {
        assert_eq!(record.element, "type");
        assert!(record.unknown.is_empty(), "record {}", record.key);
    }

    let hoarder = document.find("spawnabletypes", "Hoarder", 0).unwrap();
    assert_eq!(hoarder.field("hoarder").unwrap().raw.as_deref(), Some("1"));

    let damage = document.find("spawnabletypes", "Damage", 0).unwrap();
    assert_eq!(
        damage.field("damage_min").unwrap().raw.as_deref(),
        Some("0.2")
    );
    assert_eq!(
        damage.field("damage_max").unwrap().raw.as_deref(),
        Some("0.8")
    );

    let preset = document.find("spawnabletypes", "CargoPreset", 0).unwrap();
    let cargo = preset.list("cargo").unwrap();
    assert_eq!(cargo.items.len(), 1);
    assert!(cargo.items[0].nested.is_empty());
    assert_eq!(cargo.items[0].attrs[0].name, "preset");
    assert_eq!(cargo.items[0].attrs[0].value, "SakhalCargo");

    let items = document.find("spawnabletypes", "CargoItems", 0).unwrap();
    let cargo = items.list("cargo").unwrap();
    assert_eq!(cargo.items.len(), 1);
    assert_eq!(cargo.items[0].attrs[0].value, "0.5");
    let nested = &cargo.items[0].nested;
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0].attrs[0].name, "name");
    assert_eq!(nested[0].attrs[0].value, "Ammo_762x39");
    assert_eq!(nested[0].attrs[1].value, "40.00");
    assert_eq!(nested[1].attrs[0].value, "Rope");

    let attach = document.find("spawnabletypes", "Attach", 0).unwrap();
    let attachments = attach.list("attachments").unwrap();
    assert_eq!(attachments.items.len(), 1);
    assert_eq!(attachments.items[0].attrs[0].value, "1");
    assert_eq!(attachments.items[0].nested.len(), 1);
    assert_eq!(attachments.items[0].nested[0].attrs[0].value, "Scope");
}

#[test]
fn random_presets_read_both_record_elements() {
    let document = read(PRESETS, Some("cfgrandompresets.xml"));
    assert_eq!(document.kind, Some(CeKind::RandomPresets));
    let section = document.section("presets").unwrap();
    assert_eq!(section.records.len(), 2);
    assert_eq!(section.records[0].element, "cargo");
    assert_eq!(section.records[0].key, "Ammo");
    assert_eq!(
        section.records[0].field("chance").unwrap().raw.as_deref(),
        Some("0.5")
    );
    assert_eq!(section.records[1].element, "attachments");
    assert_eq!(section.records[1].key, "Scope");
    let items = section.records[1].list("item").unwrap();
    assert_eq!(items.items.len(), 1);
    assert_eq!(items.items[0].attrs[0].value, "B");
    assert_eq!(items.items[0].attrs[1].value, "2");
}

#[test]
fn events_read_every_field_and_their_children_list() {
    let document = read(EVENTS, Some("events.xml"));
    assert_eq!(document.kind, Some(CeKind::Events));
    assert!(!document.fragment);
    let section = document.section("events").unwrap();
    assert_eq!(section.records.len(), 2);
    for record in &section.records {
        assert_eq!(record.element, "event");
        assert!(record.unknown.is_empty(), "record {}", record.key);
    }

    let crash = document.find("events", "StaticHeliCrash", 0).unwrap();
    assert_eq!(crash.field("nominal").unwrap().raw.as_deref(), Some("1"));
    assert_eq!(
        crash.field("secondary").unwrap().raw.as_deref(),
        Some("InfectedArmyConvoy")
    );
    assert_eq!(
        crash.field("position").unwrap().raw.as_deref(),
        Some("fixed")
    );
    assert_eq!(crash.field("limit").unwrap().raw.as_deref(), Some("child"));
    assert_eq!(crash.field("deletable").unwrap().raw.as_deref(), Some("0"));
    let children = crash.list("children").unwrap();
    assert_eq!(children.items.len(), 1);
    assert_eq!(children.items[0].attrs.len(), 5);
    let child_type = children.items[0]
        .attrs
        .iter()
        .find(|attr| attr.name == "type")
        .unwrap();
    assert_eq!(child_type.value, "Wreck_UH1Y");

    let empty = document.find("events", "Empty", 0).unwrap();
    assert_eq!(empty.field("secondary").unwrap().raw, None);
    assert!(empty.list("children").unwrap().items.is_empty());
}

#[test]
fn event_spawns_read_optional_position_attributes_and_a_zone() {
    let document = read(EVENT_SPAWNS, Some("cfgeventspawns.xml"));
    assert_eq!(document.kind, Some(CeKind::EventSpawns));
    let section = document.section("events").unwrap();
    assert_eq!(section.records.len(), 1);
    let event = &section.records[0];
    assert_eq!(event.key, "StaticHeliCrash");
    assert_eq!(event.element, "event");

    let positions = event.list("pos").unwrap();
    assert_eq!(positions.items.len(), 2);
    let full = &positions.items[0];
    let names: Vec<&str> = full.attrs.iter().map(|attr| attr.name.as_str()).collect();
    assert_eq!(names, ["x", "z", "a", "y", "group"]);
    assert_eq!(full.attrs[2].value, "90");
    let bare = &positions.items[1];
    let names: Vec<&str> = bare.attrs.iter().map(|attr| attr.name.as_str()).collect();
    assert_eq!(names, ["x", "z"]);

    let zones = event.list("zone").unwrap();
    assert_eq!(zones.items.len(), 1);
    assert_eq!(zones.items[0].attrs.len(), 5);
    assert_eq!(zones.items[0].attrs[4].name, "r");
}

#[test]
fn limits_reads_four_sections_and_a_missing_container_is_empty() {
    let document = read(LIMITS, None);
    assert_eq!(document.kind, Some(CeKind::Limits));
    assert_eq!(document.sections.len(), 4);
    let counts: Vec<(&str, usize)> = document
        .sections
        .iter()
        .map(|s| (s.id.as_str(), s.records.len()))
        .collect();
    assert_eq!(
        counts,
        [
            ("categories", 2),
            ("tags", 1),
            ("usageflags", 3),
            ("valueflags", 2)
        ]
    );
    assert_eq!(document.sections[0].records[0].key, "tools");
    assert_eq!(document.sections[0].records[1].key, "weapons");

    let missing = read(LIMITS_NO_TAGS, None);
    assert_eq!(missing.section("tags").unwrap().records.len(), 0);
    assert_eq!(missing.section("categories").unwrap().records.len(), 1);
}

#[test]
fn economy_core_reads_classes_defaults_and_custom_folders() {
    let document = read(ECONOMY_CORE, None);
    assert_eq!(document.kind, Some(CeKind::EconomyCore));
    assert_eq!(
        document
            .sections
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["classes", "defaults", "ce"]
    );
    let classes = document.section("classes").unwrap();
    assert_eq!(classes.records.len(), 2);
    let rootclass = document.find("classes", "CfgVehicles", 0).unwrap();
    assert_eq!(rootclass.field("act").unwrap().raw.as_deref(), Some("car"));
    assert_eq!(
        rootclass.field("reportMemoryLOD").unwrap().raw.as_deref(),
        Some("no")
    );

    let ce = document.find("ce", "mymod", 0).unwrap();
    assert_eq!(ce.key, "mymod");
    assert_eq!(ce.element, "ce");
    let files = ce.list("file").unwrap();
    assert_eq!(files.items.len(), 2);
    assert_eq!(files.items[0].attrs[0].name, "name");
    assert_eq!(files.items[0].attrs[0].value, "types.xml");
    assert_eq!(files.items[1].attrs[0].value, "events.xml");
}

#[test]
fn economy_reads_every_group_by_element_name() {
    let document = read(ECONOMY, Some("economy.xml"));
    assert_eq!(document.kind, Some(CeKind::Economy));
    assert!(!document.fragment);
    let section = document.section("economy").unwrap();
    assert_eq!(section.records.len(), 8);
    let keys: Vec<&str> = section.records.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        keys,
        ["dynamic", "animals", "zombies", "vehicles", "randoms", "custom", "building", "player"]
    );
    for record in &section.records {
        assert_eq!(record.element, record.key);
        assert_eq!(record.key_span, None);
        assert!(record.unknown.is_empty(), "record {}", record.key);
    }
    let vehicles = document.find("economy", "vehicles", 0).unwrap();
    assert_eq!(vehicles.field("init").unwrap().raw.as_deref(), Some("1"));
    assert_eq!(vehicles.field("save").unwrap().raw.as_deref(), Some("0"));
}

#[test]
fn globals_read_variables_by_name() {
    let document = read(GLOBALS, Some("globals.xml"));
    assert_eq!(document.kind, Some(CeKind::Globals));
    let section = document.section("variables").unwrap();
    assert_eq!(section.records.len(), 2);
    let zombie = document.find("variables", "ZombieMaxCount", 0).unwrap();
    assert_eq!(zombie.element, "var");
    assert_eq!(zombie.field("type").unwrap().raw.as_deref(), Some("0"));
    assert_eq!(zombie.field("value").unwrap().raw.as_deref(), Some("1000"));
    assert!(zombie.field("value").unwrap().valid);
    assert_eq!(
        slice(GLOBALS, zombie.field("value").unwrap().span.unwrap()),
        "1000"
    );
}

#[test]
fn messages_read_children_and_index_keys() {
    let document = read(MESSAGES, Some("messages.xml"));
    assert_eq!(document.kind, Some(CeKind::Messages));
    let section = document.section("messages").unwrap();
    assert_eq!(section.records.len(), 1);
    let record = &section.records[0];
    assert_eq!(record.key, "#1");
    assert_eq!(record.element, "message");
    assert_eq!(record.field("delay").unwrap().raw.as_deref(), Some("0"));
    assert_eq!(record.field("deadline").unwrap().raw.as_deref(), Some("5"));
    assert_eq!(record.field("onconnect").unwrap().raw.as_deref(), Some("1"));
    assert_eq!(
        record.field("text").unwrap().raw.as_deref(),
        Some("Welcome")
    );
    assert_eq!(
        slice(MESSAGES, record.field("text").unwrap().span.unwrap()),
        "Welcome"
    );
}

#[test]
fn ignore_list_reads_ignored_types() {
    let document = read(IGNORE_LIST, Some("cfgignorelist.xml"));
    assert_eq!(document.kind, Some(CeKind::IgnoreList));
    let section = document.section("ignore").unwrap();
    assert_eq!(section.records.len(), 2);
    assert_eq!(section.records[0].key, "CarScript");
    assert_eq!(section.records[1].element, "type");
    assert!(section.records[0].fields.is_empty());
}

#[test]
fn an_unknown_child_is_listed_and_known_fields_still_read() {
    let document = read(UNKNOWN_CHILD, None);
    let record = document.find("types", "Odd", 0).unwrap();
    assert_eq!(record.unknown, ["nominel"]);
    assert_eq!(record.field("nominal").unwrap().raw.as_deref(), Some("5"));
}

#[test]
fn a_blocking_document_still_returns_its_records() {
    let document = read(BLOCKING, None);
    assert!(document.blocking);
    assert!(document.issues >= 1);
    assert_eq!(document.kind, Some(CeKind::Types));
    let section = document.section("types").unwrap();
    assert_eq!(section.records.len(), 1);
    assert_eq!(section.records[0].key, "A");
    assert_eq!(
        section.records[0]
            .field("nominal")
            .and_then(|field| field.raw.as_deref()),
        Some("1")
    );
}

fn zero_nodes(document: &mut Document) {
    fn zero(items: &mut [modfoundry_ce::model::ListItem]) {
        for item in items {
            item.node = 0;
            zero(&mut item.nested);
        }
    }
    for section in &mut document.sections {
        for record in &mut section.records {
            record.node = 0;
            for list in &mut record.lists {
                zero(&mut list.items);
            }
        }
    }
}

#[test]
fn json_round_trip_is_snake_case_without_node() {
    let document = read(TYPES, Some("types.xml"));
    let json = serde_json::to_string(&document).unwrap();
    assert!(!json.contains("\"node\""));
    assert!(json.contains("\"root_mismatch\""));
    assert!(json.contains("\"key_span\""));
    assert!(json.contains("\"kind\":\"types\""));
    assert!(json.contains("\"blocking\""));

    let back: Document = serde_json::from_str(&json).unwrap();
    let mut expected = document.clone();
    zero_nodes(&mut expected);
    assert_eq!(back, expected);
    assert_eq!(back.sections[0].records[0].node, 0);
}

#[test]
fn record_at_finds_the_record_holding_an_offset() {
    let document = read(TYPES, None);
    let first = &document.sections[0].records[0];
    let second = &document.sections[0].records[1];

    assert_eq!(record_at(&document, first.span.start), Some((0, 0)));
    assert_eq!(record_at(&document, first.span.end - 1), Some((0, 0)));
    assert_eq!(record_at(&document, first.span.end), None);
    assert_eq!(record_at(&document, second.span.start), Some((0, 1)));

    let usage = first.list("usage").unwrap();
    assert_eq!(
        record_at(&document, usage.items[0].span.start),
        Some((0, 0))
    );
}
