//! Class facts — MF-X121, MF-X122, MF-X123, the spawnable-loot coverage and
//! MF-X120.

use modfoundry_ce::classes::{
    class_findings, class_kind, coverage, coverage_findings, ClassFact, ClassKind, ClassLookup,
    Coverage, WorkspaceClass,
};
use modfoundry_ce::economy::{Economy, EconomySource, Origin};
use modfoundry_ce::model::{read, Document};
use modfoundry_ce::{Severity, Span};
use std::collections::HashMap;

struct Facts {
    classes: HashMap<String, ClassFact>,
    suggestions: HashMap<String, String>,
}

impl Facts {
    fn new() -> Self {
        Self {
            classes: HashMap::new(),
            suggestions: HashMap::new(),
        }
    }

    fn with(mut self, fact: ClassFact) -> Self {
        self.classes.insert(fact.name.to_ascii_lowercase(), fact);
        self
    }

    fn suggesting(mut self, name: &str, suggestion: &str) -> Self {
        self.suggestions
            .insert(name.to_ascii_lowercase(), suggestion.to_string());
        self
    }
}

impl ClassLookup for Facts {
    fn class(&self, name: &str) -> Option<ClassFact> {
        self.classes.get(&name.to_ascii_lowercase()).cloned()
    }

    fn nearest(&self, name: &str) -> Option<String> {
        self.suggestions.get(&name.to_ascii_lowercase()).cloned()
    }
}

fn fact(name: &str, scope: Option<i64>, kind: ClassKind) -> ClassFact {
    ClassFact {
        name: name.to_string(),
        scope,
        kind,
    }
}

fn chain(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn document(text: &str, name: &str) -> Document {
    read(text, Some(name))
}

#[test]
fn class_kind_reads_the_root_of_a_chain() {
    assert_eq!(
        class_kind(&chain(&[
            "FieldCanteen",
            "Canteen",
            "Bottle_Base",
            "Inventory_Base",
        ])),
        ClassKind::Loot
    );
    assert_eq!(
        class_kind(&chain(&["MyHouse", "HouseNoDestruct"])),
        ClassKind::House
    );
    assert_eq!(
        class_kind(&chain(&["Offroad", "CarScript"])),
        ClassKind::Vehicle
    );
    assert_eq!(
        class_kind(&chain(&["ZmbM_Citizen", "DZ_LightAI"])),
        ClassKind::Character
    );
    assert_eq!(class_kind(&chain(&["Odd", "Thing"])), ClassKind::Other);
}

#[test]
fn class_kind_knows_every_root_class() {
    assert_eq!(
        class_kind(&chain(&["AKM", "DefaultWeapon"])),
        ClassKind::Loot
    );
    assert_eq!(
        class_kind(&chain(&["Mag_AKM", "DefaultMagazine"])),
        ClassKind::Loot
    );
    assert_eq!(
        class_kind(&chain(&["Canoe", "BoatScript"])),
        ClassKind::Vehicle
    );
    assert_eq!(
        class_kind(&chain(&["SurvivorM_Base", "SurvivorBase"])),
        ClassKind::Character
    );
}

#[test]
fn class_kind_ignores_case() {
    assert_eq!(
        class_kind(&chain(&["x", "INVENTORY_BASE"])),
        ClassKind::Loot
    );
    assert_eq!(
        class_kind(&chain(&["x", "housenodestruct"])),
        ClassKind::House
    );
    assert_eq!(class_kind(&chain(&["x", "CARSCRIPT"])), ClassKind::Vehicle);
    assert_eq!(
        class_kind(&chain(&["x", "dz_lightai"])),
        ClassKind::Character
    );
}

#[test]
fn class_kind_takes_the_first_match_in_the_chain() {
    assert_eq!(
        class_kind(&chain(&["x", "CarScript", "Inventory_Base"])),
        ClassKind::Vehicle
    );
}

#[test]
fn a_known_public_class_is_clean() {
    let facts = Facts::new().with(fact("FieldCanteen", Some(2), ClassKind::Loot));
    let found = class_findings(
        &document(
            "<types>\n    <type name=\"FieldCanteen\"/>\n</types>\n",
            "types.xml",
        ),
        &facts,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unknown_class_is_a_warning_that_spans_the_name() {
    const TEXT: &str = "<types>\n    <type name=\"FeildCanteen\"/>\n</types>\n";
    let facts = Facts::new();
    let found = class_findings(&document(TEXT, "types.xml"), &facts);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X121");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`FeildCanteen` isn't a class in any loaded config, so the server ignores this entry."
    );
    let span = found[0].span.unwrap();
    assert_eq!(&TEXT[span.start..span.end], "FeildCanteen");
}

#[test]
fn an_unknown_class_suggests_the_nearest_when_there_is_one() {
    let facts = Facts::new()
        .with(fact("FieldCanteen", Some(2), ClassKind::Loot))
        .suggesting("FeildCanteen", "FieldCanteen");
    let found = class_findings(
        &document(
            "<types>\n    <type name=\"FeildCanteen\"/>\n</types>\n",
            "types.xml",
        ),
        &facts,
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X121");
    assert_eq!(
        found[0].message,
        "`FeildCanteen` isn't a class in any loaded config, so the server ignores this entry. Did you mean `FieldCanteen`?"
    );
}

#[test]
fn a_non_public_class_is_a_warning_naming_its_scope() {
    const TEXT: &str = "<types>\n    <type name=\"Hidden\"/>\n</types>\n";

    let one = Facts::new().with(fact("Hidden", Some(1), ClassKind::Loot));
    let found = class_findings(&document(TEXT, "types.xml"), &one);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X122");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(
        found[0].message,
        "`Hidden` isn't public (scope 1), so the server ignores this entry."
    );

    let zero = Facts::new().with(fact("Hidden", Some(0), ClassKind::Loot));
    let found = class_findings(&document(TEXT, "types.xml"), &zero);
    assert_eq!(
        found[0].message,
        "`Hidden` isn't public (scope 0), so the server ignores this entry."
    );
}

#[test]
fn a_class_without_a_resolved_scope_gets_nothing() {
    let facts = Facts::new().with(fact("Mystery", None, ClassKind::Other));
    let found = class_findings(
        &document(
            "<types>\n    <type name=\"Mystery\"/>\n</types>\n",
            "types.xml",
        ),
        &facts,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_case_different_key_is_a_hint_that_names_the_declaration() {
    let facts = Facts::new().with(fact("Bandana_BlackPattern", Some(2), ClassKind::Loot));
    let found = class_findings(
        &document(
            "<types>\n    <type name=\"bandana_blackpattern\"/>\n</types>\n",
            "types.xml",
        ),
        &facts,
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "MF-X123");
    assert_eq!(found[0].severity, Severity::Hint);
    assert_eq!(
        found[0].message,
        "`bandana_blackpattern` is declared as `Bandana_BlackPattern`."
    );
}

#[test]
fn a_case_different_private_name_gets_both_findings() {
    let facts = Facts::new().with(fact("HouseNoDestruct", Some(1), ClassKind::House));
    let found = class_findings(
        &document(
            "<types>\n    <type name=\"housenodestruct\"/>\n</types>\n",
            "types.xml",
        ),
        &facts,
    );

    let codes: Vec<&str> = found.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["MF-X122", "MF-X123"]);
}

#[test]
fn a_non_types_document_gives_nothing() {
    let facts = Facts::new().with(fact("FieldCanteen", Some(2), ClassKind::Loot));
    let found = class_findings(
        &document(
            "<events>\n    <event name=\"FeildCanteen\"/>\n</events>\n",
            "events.xml",
        ),
        &facts,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_record_with_an_empty_key_is_skipped() {
    let facts = Facts::new();
    let found = class_findings(
        &document("<types>\n    <type/>\n</types>\n", "types.xml"),
        &facts,
    );
    assert!(found.is_empty(), "{found:?}");
}

fn workspace(
    name: &str,
    scope: i64,
    kind: ClassKind,
    file: &str,
    span: Span,
    line: usize,
) -> WorkspaceClass {
    WorkspaceClass {
        name: name.to_string(),
        scope: Some(scope),
        kind,
        file: file.to_string(),
        span,
        line,
    }
}

fn economy_with(types: &str) -> (Economy, Document) {
    let document = document(types, "types.xml");
    let economy = Economy::build(&[EconomySource {
        id: "mission:db/types.xml",
        origin: Origin::Reference,
        document: &document,
    }]);
    (economy, document)
}

#[test]
fn coverage_keeps_only_public_loot_classes() {
    let (economy, _document) =
        economy_with("<types>\n    <type name=\"WaterPurifier\"/>\n</types>\n");
    let classes = vec![
        workspace(
            "Canteen",
            2,
            ClassKind::Loot,
            "config.cpp",
            Span::new(10, 17),
            3,
        ),
        workspace(
            "WaterPurifier",
            2,
            ClassKind::Loot,
            "config.cpp",
            Span::new(30, 43),
            5,
        ),
        workspace(
            "Hidden",
            1,
            ClassKind::Loot,
            "config.cpp",
            Span::new(50, 56),
            7,
        ),
        workspace(
            "House",
            2,
            ClassKind::House,
            "config.cpp",
            Span::new(60, 65),
            9,
        ),
    ];

    let coverage = coverage(&classes, &economy);

    assert_eq!(coverage.classes.len(), 2);
    assert_eq!(coverage.covered, 1);
    assert_eq!(coverage.missing, 1);
    assert_eq!(coverage.classes[0].class.name, "Canteen");
    assert!(!coverage.classes[0].has_types);
    assert_eq!(coverage.classes[1].class.name, "WaterPurifier");
    assert!(coverage.classes[1].has_types);
}

#[test]
fn coverage_matches_a_type_entry_case_insensitively() {
    let (economy, _document) =
        economy_with("<types>\n    <type name=\"waterpurifier\"/>\n</types>\n");
    let classes = vec![workspace(
        "WaterPurifier",
        2,
        ClassKind::Loot,
        "config.cpp",
        Span::new(0, 13),
        1,
    )];

    let coverage = coverage(&classes, &economy);

    assert_eq!(coverage.covered, 1);
    assert_eq!(coverage.missing, 0);
    assert!(coverage.classes[0].has_types);
}

#[test]
fn coverage_findings_pair_a_missing_class_with_its_file() {
    let (economy, _document) =
        economy_with("<types>\n    <type name=\"WaterPurifier\"/>\n</types>\n");
    let classes = vec![
        workspace(
            "WaterPurifier",
            2,
            ClassKind::Loot,
            "config.cpp",
            Span::new(30, 43),
            5,
        ),
        workspace(
            "FieldCanteen",
            2,
            ClassKind::Loot,
            "canteen.cpp",
            Span::new(4, 16),
            1,
        ),
    ];

    let findings = coverage_findings(&coverage(&classes, &economy));

    assert_eq!(findings.len(), 1);
    let (file, finding) = &findings[0];
    assert_eq!(file, "canteen.cpp");
    assert_eq!(finding.code, "MF-X120");
    assert_eq!(finding.severity, Severity::Hint);
    assert_eq!(finding.span, Some(Span::new(4, 16)));
    assert_eq!(
        finding.message,
        "`FieldCanteen` is spawnable (scope 2) but has no types entry, so it never spawns as loot."
    );
}

#[test]
fn coverage_round_trips_through_json_with_flattened_classes() {
    let (economy, _document) =
        economy_with("<types>\n    <type name=\"WaterPurifier\"/>\n</types>\n");
    let classes = vec![workspace(
        "Canteen",
        2,
        ClassKind::Loot,
        "config.cpp",
        Span::new(10, 17),
        3,
    )];
    let coverage = coverage(&classes, &economy);

    let json = serde_json::to_value(&coverage).unwrap();
    let entry = &json["classes"][0];
    assert_eq!(entry["name"], "Canteen");
    assert_eq!(entry["file"], "config.cpp");
    assert_eq!(entry["line"], 3);
    assert_eq!(entry["has_types"], false);

    let back: Coverage = serde_json::from_value(json).unwrap();
    assert_eq!(back, coverage);
}
