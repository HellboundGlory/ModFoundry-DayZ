//! Quick fixes — one diagnostic in, a titled op list out.

use crate::common::fixtures::slice;
use modfoundry_ce::classes::{class_findings, ClassFact, ClassKind, ClassLookup};
use modfoundry_ce::economy::{
    cross_findings, include_findings, Economy, EconomySource, IncludeFact, Origin,
};
use modfoundry_ce::fixes::{fixable_codes, fixes_for, CeFix};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::lints::validate_text;
use modfoundry_ce::model::read;
use modfoundry_ce::ops::plan_all;
use modfoundry_ce::{Diagnostic, Severity};
use std::collections::HashMap;

fn apply(text: &str, file_name: &str, fix: &CeFix) -> String {
    plan_all(text, Some(file_name), &fix.ops).expect("the fix plans")
}

fn finding(text: &str, file_name: &str, code: &str) -> Diagnostic {
    validate_text(text, Some(file_name))
        .into_iter()
        .find(|diagnostic| diagnostic.code == code)
        .unwrap_or_else(|| panic!("no {code} in {text:?}"))
}

fn assert_clears(_text: &str, file_name: &str, code: &str, fixed: &str) {
    let left = validate_text(fixed, Some(file_name));
    assert!(
        !left.iter().any(|diagnostic| diagnostic.code == code),
        "{code} survives: {left:?}\n{fixed}"
    );
}

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

fn class_finding(text: &str, facts: &Facts) -> Diagnostic {
    class_findings(&read(text, Some("types.xml")), facts)
        .into_iter()
        .next()
        .expect("a class finding")
}

const LIMITS: &str = r#"<lists>
    <categories>
        <category name="tools"/>
    </categories>
    <tags>
        <tag name="military"/>
    </tags>
    <usageflags>
        <usage name="Military"/>
    </usageflags>
    <valueflags>
        <value name="Tier1"/>
    </valueflags>
</lists>
"#;

fn economy() -> Economy {
    let limits = read(LIMITS, Some("cfglimitsdefinition.xml"));
    Economy::build(&[EconomySource {
        id: "mission:db/cfglimitsdefinition.xml",
        origin: Origin::Reference,
        document: &limits,
    }])
}

fn economy_findings(text: &str) -> Vec<Diagnostic> {
    cross_findings(&read(text, Some("types.xml")), &economy())
}

fn economy_finding(text: &str, code: &str) -> Diagnostic {
    economy_findings(text)
        .into_iter()
        .find(|diagnostic| diagnostic.code == code)
        .unwrap_or_else(|| panic!("no {code} in {text:?}"))
}

const FRAGMENT: &str = "<type name=\"X\">\n    <nominal>5</nominal>\n</type>";

#[test]
fn x020_fragment_wraps_in_the_kind_root() {
    let diagnostic = finding(FRAGMENT, "types.xml", "MF-X020");
    let fixes = fixes_for(FRAGMENT, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Wrap in `<types>`");
    let fixed = apply(FRAGMENT, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n<type name=\"X\">\n    <nominal>5</nominal>\n</type>\n</types>"
    );
    assert_clears(FRAGMENT, "types.xml", "MF-X020", &fixed);
}

const MISMATCH: &str =
    "<types>\n    <type name=\"X\">\n        <attachments/>\n    </type>\n</types>\n";

#[test]
fn x020_root_mismatch_renames_the_root() {
    let diagnostic = finding(MISMATCH, "cfgspawnabletypes.xml", "MF-X020");
    let fixes = fixes_for(MISMATCH, Some("cfgspawnabletypes.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Rename the root to `<spawnabletypes>`");
    let fixed = apply(MISMATCH, "cfgspawnabletypes.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<spawnabletypes>\n    <type name=\"X\">\n        <attachments/>\n    </type>\n</spawnabletypes>\n"
    );
    assert_clears(MISMATCH, "cfgspawnabletypes.xml", "MF-X020", &fixed);
}

const UNKNOWN_CHILD: &str = "<types>\n    <type name=\"X\">\n        <nominal>5</nominal>\n        <categorie name=\"tools\"/>\n    </type>\n</types>\n";

#[test]
fn x021_renames_the_element_to_the_suggestion() {
    let diagnostic = finding(UNKNOWN_CHILD, "types.xml", "MF-X021");
    let fixes = fixes_for(UNKNOWN_CHILD, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Rename to `<category>`");
    let fixed = apply(UNKNOWN_CHILD, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <nominal>5</nominal>\n        <category name=\"tools\"/>\n    </type>\n</types>\n"
    );
    assert_clears(UNKNOWN_CHILD, "types.xml", "MF-X021", &fixed);
}

#[test]
fn a_finding_without_a_suggestion_has_no_fix() {
    {
        let text = "<types>\n    <type name=\"X\">\n        <nominal>5</nominal>\n        <zzzzzzzz/>\n    </type>\n</types>\n";
        let diagnostic = finding(text, "types.xml", "MF-X021");
        assert!(fixes_for(text, Some("types.xml"), &diagnostic).is_empty());
    }
    {
        let facts = Facts::new();
        let diagnostic = class_finding(MISSING_CLASS, &facts);
        assert!(fixes_for(MISSING_CLASS, Some("types.xml"), &diagnostic).is_empty());
    }
    {
        let text = "<types>\n    <type name=\"X\">\n        <category name=\"Zzzzzz\"/>\n    </type>\n</types>\n";
        let diagnostic = economy_finding(text, "MF-X130");
        assert!(fixes_for(text, Some("types.xml"), &diagnostic).is_empty());
    }
    {
        let text = "<user_lists>\n    <usageflags>\n        <user name=\"TownVillage\">\n            <usage name=\"Zzzzzz\"/>\n        </user>\n    </usageflags>\n</user_lists>\n";
        let diagnostic = x410_finding(text);
        assert!(fixes_for(text, Some("cfglimitsdefinitionuser.xml"), &diagnostic).is_empty());
    }
}

const FLAG_TWO: &str = "<types>\n    <type name=\"X\">\n        <flags count_in_cargo=\"2\" count_in_hoarder=\"0\" count_in_map=\"1\" count_in_player=\"0\" crafted=\"0\" deloot=\"0\"/>\n    </type>\n</types>\n";

#[test]
fn x030_flag_other_whole_number_writes_one() {
    let diagnostic = finding(FLAG_TWO, "types.xml", "MF-X030");
    assert_eq!(slice(FLAG_TWO, diagnostic.span.unwrap()), "2");
    let fixes = fixes_for(FLAG_TWO, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Write 1");
    let fixed = apply(FLAG_TWO, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        FLAG_TWO.replace("count_in_cargo=\"2\"", "count_in_cargo=\"1\"")
    );
    assert_clears(FLAG_TWO, "types.xml", "MF-X030", &fixed);
}

#[test]
fn x030_flag_zero_like_value_writes_zero() {
    let text = FLAG_TWO.replace("count_in_cargo=\"2\"", "count_in_cargo=\"00\"");
    let diagnostic = finding(&text, "types.xml", "MF-X030");
    let fixes = fixes_for(&text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Write 0");
    let fixed = apply(&text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        text.replace("count_in_cargo=\"00\"", "count_in_cargo=\"0\"")
    );
    assert_clears(&text, "types.xml", "MF-X030", &fixed);
}

#[test]
fn x030_on_a_non_flag_field_has_no_fix() {
    let text =
        "<types>\n    <type name=\"X\">\n        <nominal>abc</nominal>\n    </type>\n</types>\n";
    let diagnostic = finding(text, "types.xml", "MF-X030");
    assert!(fixes_for(text, Some("types.xml"), &diagnostic).is_empty());
}

const DUPLICATE: &str = r#"<types>
    <type name="X">
        <nominal>5</nominal>
    </type>
    <type name="X">
        <nominal>7</nominal>
    </type>
</types>
"#;

#[test]
fn x040_offers_two_fixes_that_remove_different_records() {
    let diagnostic = finding(DUPLICATE, "types.xml", "MF-X040");
    let fixes = fixes_for(DUPLICATE, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 2, "{fixes:?}");
    assert_eq!(fixes[0].title, "Remove this entry (line 5)");
    assert_eq!(fixes[1].title, "Remove the other entry (line 2)");

    let first = apply(DUPLICATE, "types.xml", &fixes[0]);
    let other = apply(DUPLICATE, "types.xml", &fixes[1]);
    assert_ne!(first, other);
    assert_eq!(
        first,
        "<types>\n    <type name=\"X\">\n        <nominal>5</nominal>\n    </type>\n</types>\n"
    );
    assert_eq!(
        other,
        "<types>\n    <type name=\"X\">\n        <nominal>7</nominal>\n    </type>\n</types>\n"
    );
    assert_clears(DUPLICATE, "types.xml", "MF-X040", &first);
    assert_clears(DUPLICATE, "types.xml", "MF-X040", &other);
}

#[test]
fn the_duplicate_fix_never_removes_the_other_elements_preset() {
    const PRESETS: &str = r#"<randompresets>
    <attachments name="x"/>
    <cargo name="x"/>
    <cargo name="x"/>
</randompresets>
"#;
    let diagnostic = finding(PRESETS, "cfgrandompresets.xml", "MF-X040");
    let fixes = fixes_for(PRESETS, Some("cfgrandompresets.xml"), &diagnostic);
    assert_eq!(fixes.len(), 2, "{fixes:?}");
    for fix in &fixes {
        let fixed = apply(PRESETS, "cfgrandompresets.xml", fix);
        assert!(
            fixed.contains("<attachments name=\"x\"/>"),
            "{}: {fixed}",
            fix.title
        );
        assert_clears(PRESETS, "cfgrandompresets.xml", "MF-X040", &fixed);
    }
}

const MIN_ABOVE: &str = "<types>\n    <type name=\"X\">\n        <nominal>20</nominal>\n        <min>24</min>\n    </type>\n</types>\n";

#[test]
fn x110_offers_set_min_and_set_nominal() {
    let diagnostic = finding(MIN_ABOVE, "types.xml", "MF-X110");
    let fixes = fixes_for(MIN_ABOVE, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 2, "{fixes:?}");
    assert_eq!(fixes[0].title, "Set min to 20");
    assert_eq!(fixes[1].title, "Set nominal to 24");

    let lowered = apply(MIN_ABOVE, "types.xml", &fixes[0]);
    assert_eq!(lowered, MIN_ABOVE.replace("<min>24</min>", "<min>20</min>"));
    assert_clears(MIN_ABOVE, "types.xml", "MF-X110", &lowered);

    let raised = apply(MIN_ABOVE, "types.xml", &fixes[1]);
    assert_eq!(
        raised,
        MIN_ABOVE.replace("<nominal>20</nominal>", "<nominal>24</nominal>")
    );
    assert_clears(MIN_ABOVE, "types.xml", "MF-X110", &raised);
}

#[test]
fn x111_swaps_quantmin_and_quantmax() {
    let text = "<types>\n    <type name=\"X\">\n        <quantmin>80</quantmin>\n        <quantmax>0</quantmax>\n    </type>\n</types>\n";
    let diagnostic = finding(text, "types.xml", "MF-X111");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Swap quantmin and quantmax");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <quantmin>0</quantmin>\n        <quantmax>80</quantmax>\n    </type>\n</types>\n"
    );
    assert_clears(text, "types.xml", "MF-X111", &fixed);
}

#[test]
fn x111_one_negative_sets_both() {
    let text = "<types>\n    <type name=\"X\">\n        <quantmin>0</quantmin>\n        <quantmax>-1</quantmax>\n    </type>\n</types>\n";
    let diagnostic = finding(text, "types.xml", "MF-X111");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Set both to -1");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <quantmin>-1</quantmin>\n        <quantmax>-1</quantmax>\n    </type>\n</types>\n"
    );
    assert_clears(text, "types.xml", "MF-X111", &fixed);
}

#[test]
fn x112_keeps_the_first_category_and_removes_the_rest() {
    let text = "<types>\n    <type name=\"X\">\n        <category name=\"tools\"/>\n        <category name=\"weapons\"/>\n        <category name=\"books\"/>\n    </type>\n</types>\n";
    let diagnostic = finding(text, "types.xml", "MF-X112");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Keep `tools`");
    assert_eq!(fixes[0].ops.len(), 2);
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <category name=\"tools\"/>\n    </type>\n</types>\n"
    );
    assert_clears(text, "types.xml", "MF-X112", &fixed);
}

const MISSING_CLASS: &str =
    "<types>\n    <type name=\"FielCanteen\">\n        <nominal>5</nominal>\n    </type>\n</types>\n";

fn known(name: &str) -> Facts {
    Facts::new()
        .with(fact(name, Some(2), ClassKind::Loot))
        .suggesting("FielCanteen", name)
}

#[test]
fn x121_renames_to_the_suggested_class() {
    let facts = known("FieldCanteen");
    let diagnostic = class_finding(MISSING_CLASS, &facts);
    assert_eq!(diagnostic.code, "MF-X121");
    let fixes = fixes_for(MISSING_CLASS, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Rename to `FieldCanteen`");
    let fixed = apply(MISSING_CLASS, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>5</nominal>\n    </type>\n</types>\n"
    );
    let left = class_findings(&read(&fixed, Some("types.xml")), &facts);
    assert!(
        !left.iter().any(|finding| finding.code == "MF-X121"),
        "{left:?}"
    );
}

#[test]
fn x121_to_an_existing_class_has_no_fix() {
    let text =
        "<types>\n    <type name=\"FieldCanteen\"/>\n    <type name=\"FielCanteen\"/>\n</types>\n";
    let facts = known("FieldCanteen");
    let diagnostic = class_findings(&read(text, Some("types.xml")), &facts)
        .into_iter()
        .find(|finding| finding.code == "MF-X121")
        .expect("a finding");
    assert!(fixes_for(text, Some("types.xml"), &diagnostic).is_empty());
}

#[test]
fn x123_uses_the_declared_casing() {
    let text = "<types>\n    <type name=\"Bandana_Blackpattern\"/>\n</types>\n";
    let facts = Facts::new()
        .with(fact("Bandana_BlackPattern", Some(2), ClassKind::Loot))
        .suggesting("Bandana_Blackpattern", "Bandana_BlackPattern");
    let diagnostic = class_finding(text, &facts);
    assert_eq!(diagnostic.code, "MF-X123");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `Bandana_BlackPattern`");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"Bandana_BlackPattern\"/>\n</types>\n"
    );
    let left = class_findings(&read(&fixed, Some("types.xml")), &facts);
    assert!(
        !left.iter().any(|finding| finding.code == "MF-X123"),
        "{left:?}"
    );
}

#[test]
fn x130_uses_the_suggested_category() {
    let text = "<types>\n    <type name=\"X\">\n        <category name=\"Toolz\"/>\n    </type>\n</types>\n";
    let diagnostic = economy_finding(text, "MF-X130");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `tools`");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <category name=\"tools\"/>\n    </type>\n</types>\n"
    );
    assert!(!economy_findings(&fixed)
        .iter()
        .any(|finding| finding.code == "MF-X130"));
}

#[test]
fn x130_uses_the_suggested_list_item_name() {
    let text = "<types>\n    <type name=\"X\">\n        <usage name=\"Miltary\"/>\n    </type>\n</types>\n";
    let diagnostic = economy_finding(text, "MF-X130");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `Military`");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <usage name=\"Military\"/>\n    </type>\n</types>\n"
    );
    assert!(!economy_findings(&fixed)
        .iter()
        .any(|finding| finding.code == "MF-X130"));
}

#[test]
fn x132_fixes_the_case_only_category() {
    let text = "<types>\n    <type name=\"X\">\n        <category name=\"Tools\"/>\n    </type>\n</types>\n";
    let diagnostic = economy_finding(text, "MF-X132");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `tools`");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <category name=\"tools\"/>\n    </type>\n</types>\n"
    );
    assert!(!economy_findings(&fixed)
        .iter()
        .any(|finding| finding.code == "MF-X132"));
}

#[test]
fn x132_fixes_the_case_only_list_item() {
    let text = "<types>\n    <type name=\"X\">\n        <usage name=\"military\"/>\n    </type>\n</types>\n";
    let diagnostic = economy_finding(text, "MF-X132");
    let fixes = fixes_for(text, Some("types.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `Military`");
    let fixed = apply(text, "types.xml", &fixes[0]);
    assert_eq!(
        fixed,
        "<types>\n    <type name=\"X\">\n        <usage name=\"Military\"/>\n    </type>\n</types>\n"
    );
    assert!(!economy_findings(&fixed)
        .iter()
        .any(|finding| finding.code == "MF-X132"));
}

fn x410_finding(text: &str) -> Diagnostic {
    economy_findings(text)
        .into_iter()
        .find(|diagnostic| diagnostic.code == "MF-X410")
        .expect("an MF-X410")
}

#[test]
fn x410_uses_the_suggested_usage() {
    let text = "<user_lists>\n    <usageflags>\n        <user name=\"TownVillage\">\n            <usage name=\"Militry\"/>\n        </user>\n    </usageflags>\n</user_lists>\n";
    let diagnostic = x410_finding(text);
    let fixes = fixes_for(text, Some("cfglimitsdefinitionuser.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Use `Military`");
    let fixed = apply(text, "cfglimitsdefinitionuser.xml", &fixes[0]);
    assert_eq!(fixed, text.replace("Militry", "Military"));
    assert!(!economy_findings(&fixed)
        .iter()
        .any(|finding| finding.code == "MF-X410"));
}

fn x510_finding(text: &str) -> Diagnostic {
    let facts = [IncludeFact {
        folder: "custom".to_string(),
        name: "my_types.xml".to_string(),
        folder_exists: true,
        exists: true,
        kind: Some(CeKind::SpawnableTypes),
    }];
    include_findings(&read(text, Some("cfgeconomycore.xml")), &facts)
        .into_iter()
        .find(|diagnostic| diagnostic.code == "MF-X510")
        .expect("an MF-X510")
}

#[test]
fn x510_writes_the_detected_include_type() {
    let text = "<economycore>\n    <ce folder=\"custom\">\n        <file name=\"my_types.xml\" type=\"types\"/>\n    </ce>\n</economycore>\n";
    let diagnostic = x510_finding(text);
    let fixes = fixes_for(text, Some("cfgeconomycore.xml"), &diagnostic);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].title, "Write `spawnabletypes`");
    let fixed = apply(text, "cfgeconomycore.xml", &fixes[0]);
    assert_eq!(
        fixed,
        text.replace("type=\"types\"", "type=\"spawnabletypes\"")
    );
    let facts = [IncludeFact {
        folder: "custom".to_string(),
        name: "my_types.xml".to_string(),
        folder_exists: true,
        exists: true,
        kind: Some(CeKind::SpawnableTypes),
    }];
    assert!(include_findings(&read(&fixed, Some("cfgeconomycore.xml")), &facts).is_empty());
}

#[test]
fn an_unrelated_code_has_no_fix() {
    let text =
        "<types>\n    <type name=\"X\">\n        <nominal>-5</nominal>\n    </type>\n</types>\n";
    let diagnostic = finding(text, "types.xml", "MF-X031");
    assert!(fixes_for(text, Some("types.xml"), &diagnostic).is_empty());
}

#[test]
fn a_stale_or_missing_span_has_no_fix() {
    {
        let diagnostic = finding(MIN_ABOVE, "types.xml", "MF-X110");
        let shifted = format!("<!--{}-->\n{MIN_ABOVE}", "x".repeat(200));
        assert!(fixes_for(&shifted, Some("types.xml"), &diagnostic).is_empty());
    }
    {
        let diagnostic = Diagnostic::new("MF-X110", Severity::Warning, "min is above nominal");
        assert!(fixes_for(MIN_ABOVE, Some("types.xml"), &diagnostic).is_empty());
    }
}

#[test]
fn fixable_codes_is_exactly_the_supported_list() {
    assert_eq!(
        fixable_codes(),
        [
            "MF-X020", "MF-X021", "MF-X030", "MF-X040", "MF-X110", "MF-X111", "MF-X112", "MF-X121",
            "MF-X123", "MF-X130", "MF-X132", "MF-X410", "MF-X510",
        ]
    );
}
