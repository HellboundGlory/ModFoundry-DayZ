//! The economy model — the vocabulary, the type index and the cross-file
//! lints MF-X130, MF-X131 and MF-X132.

use modfoundry_ce::economy::{
    cross_findings, include_findings, Economy, EconomySource, IncludeFact, Lookup, Origin, UserFlag,
};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::model::{read, Document};
use modfoundry_ce::schema::Vocab;
use modfoundry_ce::{Diagnostic, Severity};

const LIMITS: &str = r#"<lists>
    <categories>
        <category name="tools"/>
        <category name="weapons"/>
    </categories>
    <tags>
        <tag name="military"/>
    </tags>
    <usageflags>
        <usage name="Military"/>
        <usage name="Village"/>
    </usageflags>
    <valueflags>
        <value name="Tier1"/>
        <value name="Tier2"/>
        <value name="Tier3"/>
        <value name="Tier4"/>
    </valueflags>
</lists>
"#;

const USER_LIMITS: &str = r#"<user_lists>
    <usageflags>
        <user name="TownVillage">
            <usage name="Town"/>
            <usage name="Village"/>
        </user>
    </usageflags>
    <valueflags>
        <user name="Tier12">
            <value name="Tier1"/>
        </user>
    </valueflags>
</user_lists>
"#;

const WORKSPACE_LIMITS: &str = r#"<lists>
    <categories>
        <category name="tools"/>
        <category name="books"/>
    </categories>
</lists>
"#;

fn doc(text: &str, name: &str) -> Document {
    read(text, Some(name))
}

fn reference<'a>(id: &'a str, document: &'a Document) -> EconomySource<'a> {
    EconomySource {
        id,
        origin: Origin::Reference,
        document,
    }
}

fn vocabulary() -> (Economy, Document, Document) {
    let limits = doc(LIMITS, "cfglimitsdefinition.xml");
    let user = doc(USER_LIMITS, "cfglimitsdefinitionuser.xml");
    let economy = Economy::build(&[
        reference("mission:db/cfglimitsdefinition.xml", &limits),
        reference("mission:db/cfglimitsdefinitionuser.xml", &user),
    ]);
    (economy, limits, user)
}

fn findings(types_text: &str) -> Vec<Diagnostic> {
    let (economy, _limits, _user) = vocabulary();
    let document = doc(types_text, "types.xml");
    cross_findings(&document, &economy)
}

#[test]
fn build_reads_the_vocabulary_from_limits_and_user_limits() {
    let (economy, _limits, _user) = vocabulary();

    assert!(economy.has_limits);
    assert_eq!(economy.vocabulary.categories, ["tools", "weapons"]);
    assert_eq!(economy.vocabulary.tags, ["military"]);
    assert_eq!(economy.vocabulary.usage, ["Military", "Village"]);
    assert_eq!(
        economy.vocabulary.value,
        ["Tier1", "Tier2", "Tier3", "Tier4"]
    );
    assert_eq!(
        economy.vocabulary.user_usage,
        [UserFlag {
            name: "TownVillage".into(),
            names: vec!["Town".into(), "Village".into()],
        }]
    );
    assert_eq!(
        economy.vocabulary.user_value,
        [UserFlag {
            name: "Tier12".into(),
            names: vec!["Tier1".into()],
        }]
    );
}

#[test]
fn a_second_limits_document_adds_a_name_and_does_not_repeat_one() {
    let limits = doc(LIMITS, "cfglimitsdefinition.xml");
    let workspace = doc(WORKSPACE_LIMITS, "limits.xml");
    let economy = Economy::build(&[
        reference("mission:db/cfglimitsdefinition.xml", &limits),
        EconomySource {
            id: "proj/limits.xml",
            origin: Origin::Workspace,
            document: &workspace,
        },
    ]);

    assert_eq!(economy.vocabulary.categories, ["tools", "weapons", "books"]);
}

#[test]
fn has_limits_is_false_without_a_limits_document() {
    let types = doc(TYPES, "types.xml");
    let economy = Economy::build(&[reference("mission:db/types.xml", &types)]);
    assert!(!economy.has_limits);

    let limits = doc(LIMITS, "cfglimitsdefinition.xml");
    let economy = Economy::build(&[reference("mission:db/cfglimitsdefinition.xml", &limits)]);
    assert!(economy.has_limits);
}

const TYPES: &str = r#"<types>
    <type name="FieldCanteen"/>
    <type name="WaterBottle"/>
</types>
"#;

const REFERENCE_TYPES: &str = r#"<types>
    <type name="FieldCanteen"/>
</types>
"#;

const WORKSPACE_TYPES: &str = r#"<types>
    <type name="FieldCanteen"/>
    <type name="WaterBottle"/>
</types>
"#;

#[test]
fn has_type_folds_case_and_type_count_skips_repeats() {
    let reference_doc = doc(REFERENCE_TYPES, "types.xml");
    let workspace_doc = doc(WORKSPACE_TYPES, "types.xml");
    let economy = Economy::build(&[
        reference("mission:db/types.xml", &reference_doc),
        EconomySource {
            id: "proj/types.xml",
            origin: Origin::Workspace,
            document: &workspace_doc,
        },
    ]);

    assert!(economy.has_type("FIELDcanteen"));
    assert!(!economy.has_type("Waterpurifier"));
    assert_eq!(economy.type_count(), 2);
}

#[test]
fn a_record_with_an_empty_key_is_not_a_type() {
    let unnamed = doc(
        "<types>\n    <type/>\n    <type name=\"FieldCanteen\"/>\n</types>\n",
        "types.xml",
    );
    let economy = Economy::build(&[reference("mission:db/types.xml", &unnamed)]);
    assert_eq!(economy.type_count(), 1);
}

#[test]
fn find_is_exact_case_only_or_missing_with_a_nearest() {
    let (economy, _limits, _user) = vocabulary();
    let vocabulary = &economy.vocabulary;

    assert_eq!(vocabulary.find(Vocab::Usage, "Military"), Lookup::Exact);
    assert_eq!(
        vocabulary.find(Vocab::Usage, "military"),
        Lookup::CaseOnly("Military")
    );
    assert_eq!(
        vocabulary.find(Vocab::Usage, "Militry"),
        Lookup::Missing {
            nearest: Some("Military"),
        }
    );
    assert_eq!(
        vocabulary.find(Vocab::Usage, "zzzzzz"),
        Lookup::Missing { nearest: None }
    );
    assert_eq!(
        vocabulary.find(Vocab::UserUsage, "TownVillage"),
        Lookup::Exact
    );
    assert_eq!(
        vocabulary.find(Vocab::UserUsage, "townvillage"),
        Lookup::CaseOnly("TownVillage")
    );
    assert_eq!(
        vocabulary.find(Vocab::Event, "x"),
        Lookup::Missing { nearest: None }
    );
    assert_eq!(vocabulary.names(Vocab::Event), Vec::<&str>::new());
}

#[test]
fn a_clean_types_document_gives_nothing() {
    let found = findings(
        r#"<types>
    <type name="FieldCanteen">
        <category name="tools"/>
        <tag name="military"/>
        <usage name="Military"/>
        <value name="Tier1"/>
        <value user="Tier12"/>
    </type>
</types>
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unknown_usage_is_a_warning_with_the_nearest_name() {
    const TEXT: &str = r#"<types>
    <type name="FieldCanteen">
        <usage name="Militry"/>
    </type>
</types>
"#;
    let found = findings(TEXT);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X130");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`Militry` isn't a usage this mission defines. Did you mean `Military`?"
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "Militry");
}

#[test]
fn an_unknown_category_names_the_nearest() {
    const TEXT: &str = r#"<types>
    <type name="FieldCanteen">
        <category name="tool"/>
    </type>
</types>
"#;
    let found = findings(TEXT);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X130");
    assert_eq!(
        found[0].message,
        "`tool` isn't a category this mission defines. Did you mean `tools`?"
    );
}

#[test]
fn a_case_only_value_is_a_hint_that_names_the_definition() {
    const TEXT: &str = r#"<types>
    <type name="FieldCanteen">
        <value name="tier3"/>
    </type>
</types>
"#;
    let found = findings(TEXT);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X132");
    assert_eq!(found[0].severity, Severity::Hint);
    assert_eq!(found[0].message, "`tier3` is defined as `Tier3`.");
}

#[test]
fn a_known_user_flag_is_clean_and_an_unknown_one_is_a_warning() {
    let clean = findings(
        r#"<types>
    <type name="FieldCanteen">
        <value user="Tier12"/>
    </type>
</types>
"#,
    );
    assert!(clean.is_empty(), "{clean:?}");

    const TEXT: &str = r#"<types>
    <type name="FieldCanteen">
        <value user="Tier99"/>
    </type>
</types>
"#;
    let found = findings(TEXT);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X131");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`Tier99` isn't a user flag this mission defines. Did you mean `Tier12`?"
    );
}

#[test]
fn a_case_only_user_flag_is_a_hint() {
    let found = findings(
        r#"<types>
    <type name="FieldCanteen">
        <value user="tier12"/>
    </type>
</types>
"#,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X132");
    assert_eq!(found[0].message, "`tier12` is defined as `Tier12`.");
}

#[test]
fn an_empty_category_is_skipped() {
    let found = findings(
        r#"<types>
    <type name="FieldCanteen">
        <category name=""/>
    </type>
</types>
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn nothing_is_reported_without_a_limits_document() {
    let types = doc(REFERENCE_TYPES, "types.xml");
    let economy = Economy::build(&[reference("mission:db/types.xml", &types)]);
    let document = doc(
        r#"<types>
    <type name="FieldCanteen">
        <usage name="Militry"/>
    </type>
</types>
"#,
        "types.xml",
    );
    assert!(cross_findings(&document, &economy).is_empty());
}

#[test]
fn a_non_types_document_is_not_cross_checked() {
    let (economy, _limits, _user) = vocabulary();
    let document = doc(LIMITS, "cfglimitsdefinition.xml");
    assert!(cross_findings(&document, &economy).is_empty());
}

const PRESETS: &str = r#"<randompresets>
    <cargo name="foodArmy">
        <item name="TunaCan"/>
    </cargo>
    <attachments name="scopeArmy">
        <item name="AugOptic"/>
    </attachments>
</randompresets>
"#;

const SPAWNABLE_TYPES: &str = r#"<types>
    <type name="M4A1"/>
    <type name="TunaCan"/>
    <type name="AugOptic"/>
</types>
"#;

fn spawnable_economy(presets: Option<&str>, types: Option<&str>) -> Economy {
    let presets = presets.map(|text| doc(text, "cfgrandompresets.xml"));
    let types = types.map(|text| doc(text, "types.xml"));
    let mut sources = Vec::new();
    if let Some(document) = presets.as_ref() {
        sources.push(reference("mission:db/cfgrandompresets.xml", document));
    }
    if let Some(document) = types.as_ref() {
        sources.push(reference("mission:db/types.xml", document));
    }
    Economy::build(&sources)
}

fn spawnable_findings(economy: &Economy, text: &str) -> Vec<Diagnostic> {
    cross_findings(&doc(text, "cfgspawnabletypes.xml"), economy)
}

#[test]
fn preset_vocabularies_answer_from_the_random_presets_source() {
    let economy = spawnable_economy(Some(PRESETS), None);

    assert!(economy.has_presets);
    assert_eq!(economy.vocabulary.names(Vocab::CargoPreset), ["foodArmy"]);
    assert_eq!(
        economy.vocabulary.names(Vocab::AttachmentsPreset),
        ["scopeArmy"]
    );
    assert_eq!(
        economy.vocabulary.find(Vocab::CargoPreset, "foodArmy"),
        Lookup::Exact
    );
    assert_eq!(
        economy.vocabulary.find(Vocab::CargoPreset, "foodarmy"),
        Lookup::CaseOnly("foodArmy")
    );
}

#[test]
fn mf_x310_flags_a_misspelled_preset_and_suggests_the_defined_one() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo preset="foodArmyy"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    let found = spawnable_findings(&economy, TEXT);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X310");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`foodArmyy` isn't a cargo preset this mission defines. Did you mean `foodArmy`?"
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "foodArmyy");
}

#[test]
fn mf_x310_without_a_near_name_omits_the_question() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo preset="zzzzzz"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    let found = spawnable_findings(&economy, TEXT);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].message,
        "`zzzzzz` isn't a cargo preset this mission defines."
    );
}

#[test]
fn mf_x310_is_silent_without_a_presets_source() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo preset="foodArmyy"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(None, Some(SPAWNABLE_TYPES));

    assert!(!economy.has_presets);
    assert!(spawnable_findings(&economy, TEXT).is_empty());
}

#[test]
fn mf_x310_respects_the_element_a_preset_belongs_to() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo preset="scopeArmy"/>
    </type>
    <type name="M4A1">
        <attachments preset="foodArmy"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    let found = spawnable_findings(&economy, TEXT);

    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0].message,
        "`scopeArmy` isn't a cargo preset this mission defines."
    );
    assert_eq!(
        found[1].message,
        "`foodArmy` isn't an attachments preset this mission defines."
    );
}

#[test]
fn mf_x320_flags_a_spawnable_record_with_no_types_entry() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="MissingType">
        <cargo preset="foodArmy"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    let found = spawnable_findings(&economy, TEXT);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X320");
    assert_eq!(found[0].severity, Severity::Hint);
    assert_eq!(
        found[0].message,
        "`MissingType` has no types entry in this mission."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "MissingType");
}

#[test]
fn mf_x320_flags_a_nested_item_with_no_types_entry() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo chance="0.5">
            <item name="Ammo_762x39"/>
        </cargo>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    let found = spawnable_findings(&economy, TEXT);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X320");
    assert_eq!(
        found[0].message,
        "`Ammo_762x39` has no types entry in this mission."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "Ammo_762x39");
}

#[test]
fn mf_x320_flags_a_preset_item_with_no_types_entry() {
    const PRESET_ITEM_MISSING: &str = r#"<randompresets>
    <cargo name="foodArmy">
        <item name="SantaHat"/>
    </cargo>
</randompresets>
"#;
    let economy = spawnable_economy(Some(PRESET_ITEM_MISSING), Some(SPAWNABLE_TYPES));
    let document = doc(PRESET_ITEM_MISSING, "cfgrandompresets.xml");
    let found = cross_findings(&document, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X320");
    assert_eq!(
        found[0].message,
        "`SantaHat` has no types entry in this mission."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&PRESET_ITEM_MISSING[span.start..span.end], "SantaHat");
}

#[test]
fn mf_x320_is_silent_when_the_economy_has_no_types() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="MissingType"/>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), None);

    assert_eq!(economy.type_count(), 0);
    assert!(spawnable_findings(&economy, TEXT).is_empty());
}

#[test]
fn a_clean_spawnable_document_gives_nothing() {
    const TEXT: &str = r#"<spawnabletypes>
    <type name="M4A1">
        <cargo preset="foodArmy"/>
        <attachments preset="scopeArmy"/>
    </type>
</spawnabletypes>
"#;
    let economy = spawnable_economy(Some(PRESETS), Some(SPAWNABLE_TYPES));
    assert!(
        spawnable_findings(&economy, TEXT).is_empty(),
        "{:?}",
        spawnable_findings(&economy, TEXT)
    );
}

const EVENTS: &str = r#"<events>
    <event name="VehicleTransitBus">
        <children>
            <child type="M4A1" min="1" max="1"/>
        </children>
    </event>
</events>
"#;

const EVENT_ONE: &str = r#"<events>
    <event name="VehicleTransitBus"/>
</events>
"#;

const EVENT_MISSING_CHILD: &str = r#"<events>
    <event name="VehicleTransitBus">
        <children>
            <child type="Wreck_Mi9" min="1" max="1"/>
        </children>
    </event>
</events>
"#;

const EVENT_SPAWNS: &str = r#"<eventposdef>
    <event name="VehicleTransitBuss">
        <pos x="1" z="2"/>
    </event>
</eventposdef>
"#;

fn event_economy(events: Option<&str>, types: Option<&str>) -> Economy {
    let events = events.map(|text| doc(text, "events.xml"));
    let types = types.map(|text| doc(text, "types.xml"));
    let mut sources = Vec::new();
    if let Some(document) = events.as_ref() {
        sources.push(reference("mission:db/events.xml", document));
    }
    if let Some(document) = types.as_ref() {
        sources.push(reference("mission:db/types.xml", document));
    }
    Economy::build(&sources)
}

#[test]
fn vocabulary_names_answer_from_the_events_source() {
    let economy = event_economy(Some(EVENTS), None);

    assert!(economy.has_events);
    assert_eq!(
        economy.vocabulary.names(Vocab::Event),
        ["VehicleTransitBus"]
    );
    assert_eq!(
        economy.vocabulary.find(Vocab::Event, "VehicleTransitBus"),
        Lookup::Exact
    );
    assert_eq!(
        economy.vocabulary.find(Vocab::Event, "vehicletransitbus"),
        Lookup::CaseOnly("VehicleTransitBus")
    );
    assert_eq!(
        economy.vocabulary.find(Vocab::Event, "VehicleTransitBuss"),
        Lookup::Missing {
            nearest: Some("VehicleTransitBus"),
        }
    );
}

#[test]
fn mf_x210_flags_a_child_type_with_no_types_entry() {
    let economy = event_economy(Some(EVENT_MISSING_CHILD), Some(SPAWNABLE_TYPES));
    let document = doc(EVENT_MISSING_CHILD, "events.xml");
    let found = cross_findings(&document, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X210");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`Wreck_Mi9` has no types entry, so this event can't spawn it."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&EVENT_MISSING_CHILD[span.start..span.end], "Wreck_Mi9");
}

#[test]
fn mf_x210_is_silent_when_the_child_type_exists() {
    let economy = event_economy(Some(EVENTS), Some(SPAWNABLE_TYPES));
    let document = doc(EVENTS, "events.xml");
    assert_eq!(economy.type_count(), 3);
    assert!(cross_findings(&document, &economy).is_empty());
}

#[test]
fn mf_x210_is_silent_without_a_types_source() {
    let economy = event_economy(Some(EVENT_MISSING_CHILD), None);
    assert_eq!(economy.type_count(), 0);
    let document = doc(EVENT_MISSING_CHILD, "events.xml");
    assert!(cross_findings(&document, &economy).is_empty());
}

#[test]
fn mf_x220_flags_spawns_for_an_event_no_events_file_defines() {
    let economy = event_economy(Some(EVENT_ONE), Some(SPAWNABLE_TYPES));
    let document = doc(EVENT_SPAWNS, "cfgeventspawns.xml");
    let found = cross_findings(&document, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X220");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`VehicleTransitBuss` has spawn positions but no event. Did you mean `VehicleTransitBus`?"
    );
    let span = found[0].span.unwrap();
    assert_eq!(&EVENT_SPAWNS[span.start..span.end], "VehicleTransitBuss");
}

#[test]
fn mf_x220_without_a_near_name_omits_the_question() {
    const TEXT: &str = r#"<eventposdef>
    <event name="zzzzzz">
        <pos x="1" z="2"/>
    </event>
</eventposdef>
"#;
    let economy = event_economy(Some(EVENT_ONE), Some(SPAWNABLE_TYPES));
    let document = doc(TEXT, "cfgeventspawns.xml");
    let found = cross_findings(&document, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].message,
        "`zzzzzz` has spawn positions but no event."
    );
}

#[test]
fn mf_x220_is_silent_without_an_events_source() {
    let economy = event_economy(None, Some(SPAWNABLE_TYPES));
    assert!(!economy.has_events);
    let document = doc(EVENT_SPAWNS, "cfgeventspawns.xml");
    assert!(cross_findings(&document, &economy).is_empty());
}

#[test]
fn an_event_whose_secondary_names_no_event_raises_nothing() {
    const TEXT: &str = r#"<events>
    <event name="VehicleTransitBus">
        <secondary>NotAnEvent</secondary>
        <children>
            <child type="M4A1" min="1" max="1"/>
        </children>
    </event>
</events>
"#;
    let economy = event_economy(Some(TEXT), Some(SPAWNABLE_TYPES));
    let document = doc(TEXT, "events.xml");
    assert!(cross_findings(&document, &economy).is_empty());
}

const X410_LIMITS: &str = r#"<lists>
    <usageflags>
        <usage name="Town"/>
        <usage name="Village"/>
    </usageflags>
    <valueflags>
        <value name="Tier1"/>
    </valueflags>
</lists>
"#;

fn x410_economy(user_text: &str) -> (Economy, Document) {
    let limits = doc(X410_LIMITS, "cfglimitsdefinition.xml");
    let user = doc(user_text, "cfglimitsdefinitionuser.xml");
    let economy = Economy::build(&[
        reference("mission:db/cfglimitsdefinition.xml", &limits),
        reference("mission:db/cfglimitsdefinitionuser.xml", &user),
    ]);
    (economy, user)
}

#[test]
fn mf_x410_flags_a_listed_usage_the_definitions_lack() {
    const TEXT: &str = r#"<user_lists>
    <usageflags>
        <user name="TownVillage">
            <usage name="Twon"/>
        </user>
    </usageflags>
</user_lists>
"#;
    let (economy, user) = x410_economy(TEXT);
    let found = cross_findings(&user, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X410");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`Twon` isn't a usage this mission defines. Did you mean `Town`?"
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "Twon");
}

#[test]
fn mf_x410_without_a_near_name_omits_the_question() {
    const TEXT: &str = r#"<user_lists>
    <usageflags>
        <user name="TownVillage">
            <usage name="Zzzzzz"/>
        </user>
    </usageflags>
</user_lists>
"#;
    let (economy, user) = x410_economy(TEXT);
    let found = cross_findings(&user, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].message,
        "`Zzzzzz` isn't a usage this mission defines."
    );
}

#[test]
fn mf_x410_covers_value_flags_and_names_them_a_value() {
    const TEXT: &str = r#"<user_lists>
    <valueflags>
        <user name="TierOne">
            <value name="Zzzzzz"/>
        </user>
    </valueflags>
</user_lists>
"#;
    let (economy, user) = x410_economy(TEXT);
    let found = cross_findings(&user, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X410");
    assert_eq!(
        found[0].message,
        "`Zzzzzz` isn't a value this mission defines."
    );
}

#[test]
fn a_case_only_usage_in_a_user_flag_is_mf_x132_not_x410() {
    const TEXT: &str = r#"<user_lists>
    <usageflags>
        <user name="TownVillage">
            <usage name="town"/>
        </user>
    </usageflags>
</user_lists>
"#;
    let (economy, user) = x410_economy(TEXT);
    let found = cross_findings(&user, &economy);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X132");
    assert_eq!(found[0].severity, Severity::Hint);
    assert_eq!(found[0].message, "`town` is defined as `Town`.");
}

#[test]
fn mf_x410_is_silent_without_a_limits_document() {
    const TEXT: &str = r#"<user_lists>
    <usageflags>
        <user name="TownVillage">
            <usage name="Twon"/>
        </user>
    </usageflags>
</user_lists>
"#;
    let user = doc(TEXT, "cfglimitsdefinitionuser.xml");
    let economy = Economy::build(&[reference("mission:db/cfglimitsdefinitionuser.xml", &user)]);

    assert!(!economy.has_limits);
    assert!(cross_findings(&user, &economy).is_empty());
}

/// A fact for a file in a folder that is on disk.
fn fact(folder: &str, name: &str, exists: bool, kind: Option<CeKind>) -> IncludeFact {
    IncludeFact {
        folder: folder.to_string(),
        name: name.to_string(),
        folder_exists: true,
        exists,
        kind,
    }
}

fn fact_in_a_missing_folder(folder: &str, name: &str, kind: Option<CeKind>) -> IncludeFact {
    IncludeFact {
        folder: folder.to_string(),
        name: name.to_string(),
        folder_exists: false,
        exists: false,
        kind,
    }
}

const INCLUDES: &str = r#"<economycore>
    <ce folder="custom">
        <file name="present.xml" type="types"/>
        <file name="missing.xml" type="types"/>
    </ce>
</economycore>
"#;

#[test]
fn mf_x500_flags_a_missing_file_with_its_folder_path() {
    let document = doc(INCLUDES, "cfgeconomycore.xml");
    let facts = [
        fact("custom", "present.xml", true, Some(CeKind::Types)),
        fact("custom", "missing.xml", false, None),
    ];
    let found = include_findings(&document, &facts);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X500");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(found[0].message, "`custom/missing.xml` isn't there.");
    let span = found[0].span.unwrap();
    assert_eq!(&INCLUDES[span.start..span.end], "missing.xml");
}

#[test]
fn mf_x500_reports_a_missing_folder_once_on_the_folder() {
    const TEXT: &str = r#"<economycore>
    <ce folder="gone">
        <file name="a.xml" type="types"/>
        <file name="b.xml" type="types"/>
    </ce>
</economycore>
"#;
    let document = doc(TEXT, "cfgeconomycore.xml");
    let facts = [
        fact_in_a_missing_folder("gone", "a.xml", None),
        fact_in_a_missing_folder("gone", "b.xml", None),
    ];
    let found = include_findings(&document, &facts);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X500");
    assert_eq!(found[0].message, "The folder `gone` isn't there.");
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "gone");
}

#[test]
fn mf_x500_names_the_files_when_the_folder_exists() {
    const TEXT: &str = r#"<economycore>
    <ce folder="custom">
        <file name="gone.xml" type="types"/>
    </ce>
</economycore>
"#;
    let document = doc(TEXT, "cfgeconomycore.xml");
    let facts = [fact("custom", "gone.xml", false, None)];
    let found = include_findings(&document, &facts);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X500");
    assert_eq!(found[0].message, "`custom/gone.xml` isn't there.");
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "gone.xml");
}

#[test]
fn mf_x510_flags_a_file_whose_content_is_another_kind() {
    const TEXT: &str = r#"<economycore>
    <ce folder="custom">
        <file name="my_types.xml" type="types"/>
    </ce>
</economycore>
"#;
    let document = doc(TEXT, "cfgeconomycore.xml");
    let facts = [fact(
        "custom",
        "my_types.xml",
        true,
        Some(CeKind::SpawnableTypes),
    )];
    let found = include_findings(&document, &facts);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X510");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`my_types.xml` holds spawnable types, but its type says `types`. Write `spawnabletypes`."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "types");
}

#[test]
fn mf_x510_on_a_kind_economy_core_cannot_load_offers_no_type() {
    const TEXT: &str = r#"<economycore>
    <ce folder="custom">
        <file name="cfglimitsdefinition.xml" type="types"/>
    </ce>
</economycore>
"#;
    let document = doc(TEXT, "cfgeconomycore.xml");
    let facts = [fact(
        "custom",
        "cfglimitsdefinition.xml",
        true,
        Some(CeKind::Limits),
    )];
    let found = include_findings(&document, &facts);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X510");
    assert_eq!(
        found[0].message,
        "`cfglimitsdefinition.xml` holds limits, which economy core can't load as a `<file>`."
    );
}

#[test]
fn a_file_whose_kind_matches_its_type_is_clean() {
    let document = doc(INCLUDES, "cfgeconomycore.xml");
    let facts = [
        fact("custom", "present.xml", true, Some(CeKind::Types)),
        fact("custom", "missing.xml", true, Some(CeKind::Types)),
    ];
    assert!(include_findings(&document, &facts).is_empty());
}

#[test]
fn a_file_with_no_detected_kind_gets_neither_finding() {
    const TEXT: &str = r#"<economycore>
    <ce folder="custom">
        <file name="present.xml" type="types"/>
    </ce>
</economycore>
"#;
    let document = doc(TEXT, "cfgeconomycore.xml");
    let facts = [fact("custom", "present.xml", true, None)];
    assert!(include_findings(&document, &facts).is_empty());
}

#[test]
fn include_findings_ignores_a_non_economy_core_document() {
    let document = doc(LIMITS, "cfglimitsdefinition.xml");
    let facts = [fact("custom", "missing.xml", false, None)];
    assert!(include_findings(&document, &facts).is_empty());
}
