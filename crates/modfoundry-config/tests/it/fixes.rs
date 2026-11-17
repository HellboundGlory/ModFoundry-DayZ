//! The pure quick fixes over a config.cpp text and one diagnostic.

use modfoundry_config::edit::{plan_all, EditError};
use modfoundry_config::fixes::*;
use modfoundry_config::{validate_text, Diagnostic, Severity, Span};

fn validate(text: &str) -> Vec<Diagnostic> {
    validate_text(text, None).diagnostics
}

fn codes(text: &str) -> Vec<String> {
    validate(text).into_iter().map(|d| d.code).collect()
}

fn find(text: &str, code: &str) -> Diagnostic {
    validate(text)
        .into_iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no {code} in:\n{text}"))
}

fn titles(fixes: &[Fix]) -> Vec<String> {
    fixes.iter().map(|f| f.title.clone()).collect()
}

fn applied(text: &str, fix: &Fix) -> String {
    let out = plan_all(text, &fix.ops).unwrap_or_else(|e| panic!("{e}: {:?}", fix.ops));
    assert_ne!(out, text, "the fix changed nothing");
    out
}

fn mod_config(path: &str) -> String {
    format!(
        "class CfgPatches\n{{\n    class M\n    {{\n        requiredAddons[] = {{ \"DZ_Data\" }};\n    }};\n}};\n\
         class CfgMods\n{{\n    class M\n    {{\n        class defs\n        {{\n            class worldScriptModule\n            {{\n                files[] = {{ \"{path}\" }};\n            }};\n        }};\n    }};\n}};\n"
    )
}

fn c400(mod_name: &str, patches: &[&str]) -> Diagnostic {
    let list = patches
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::at(
        "MF-C400",
        Severity::Warning,
        Span::new(0, 5),
        format!(
            "`{mod_name}` is marked as a dependency, but none of its addons ({list}) are in requiredAddons."
        ),
    )
}

const PATCHES: &str =
    "class CfgPatches\n{\n    class M\n    {\n        units[] = {};\n    };\n};\n";

#[test]
fn fixable_codes_are_exactly_the_answered_codes() {
    assert_eq!(
        fixable_codes(),
        &["MF-C300", "MF-C310", "MF-C330", "MF-C340", "MF-C341", "MF-C350", "MF-C400"]
    );
}

#[test]
fn unrelated_codes_and_missing_or_stale_spans_give_no_fix() {
    let text = mod_config("FieldKit/Scripts/5_Mission");
    let facts = FixFacts::default();
    let unrelated = Diagnostic::at("MF-C101", Severity::Error, Span::new(0, 5), "syntax");
    assert!(fixes_for(text.as_str(), &unrelated, &facts).is_empty());
    let stale = Diagnostic::at("MF-C310", Severity::Error, Span::new(0, 5), "stale");
    assert!(fixes_for(text.as_str(), &stale, &facts).is_empty());
    let mut pointless = find(&text, "MF-C310");
    pointless.span = None;
    assert!(fixes_for(text.as_str(), &pointless, &facts).is_empty());
}

#[test]
fn c310_points_files_at_the_slots_canonical_dir() {
    let text = mod_config("FieldKit/Scripts/5_Mission");
    let fixes = fixes_for(&text, &find(&text, "MF-C310"), &FixFacts::default());
    assert_eq!(
        titles(&fixes),
        ["Point files[] at `FieldKit/Scripts/4_World`"]
    );
    assert_eq!(
        fixes[0].checks,
        vec![
            FixCheck::DirExists("FieldKit/Scripts/4_World".into()),
            FixCheck::Clears("MF-C320".into()),
        ]
    );
    let out = applied(&text, &fixes[0]);
    assert_eq!(out, text.replace("5_Mission", "4_World"));
    assert!(!codes(&out).contains(&"MF-C310".into()));
    assert!(!codes(&out).contains(&"MF-C300".into()));
}

#[test]
fn c300_uses_the_one_near_miss_dir() {
    let text = mod_config("FieldKit/Scripts/4_worl");
    let fixes = fixes_for(&text, &find(&text, "MF-C300"), &FixFacts::default());
    assert_eq!(titles(&fixes), ["Use `4_World`"]);
    assert!(fixes[0].checks.is_empty());
    let out = applied(&text, &fixes[0]);
    assert_eq!(out, text.replace("4_worl", "4_World"));
    assert!(!codes(&out).contains(&"MF-C300".into()));

    let cased = mod_config("FieldKit/Scripts/4_world");
    let fixes = fixes_for(&cased, &find(&cased, "MF-C300"), &FixFacts::default());
    assert_eq!(titles(&fixes), ["Use `4_World`"]);
}

#[test]
fn c300_gives_no_fix_when_no_dir_matches_or_the_path_uses_backslashes() {
    // The closest canonical pair (`1_Core`/`3_Game`) is four apart, so two dirs never match one leaf.
    let nothing = mod_config("FieldKit/Scripts/Banana");
    assert!(fixes_for(&nothing, &find(&nothing, "MF-C300"), &FixFacts::default()).is_empty());
    let backslash = mod_config("FieldKit\\Scripts\\4_World");
    assert!(fixes_for(
        &backslash,
        &find(&backslash, "MF-C300"),
        &FixFacts::default()
    )
    .is_empty());
}

const DUP: &str = "class A\n{\n    x = 1;\n    X = 2;\n};\n";

#[test]
fn c330_offers_both_removals() {
    let fixes = fixes_for(DUP, &find(DUP, "MF-C330"), &FixFacts::default());
    assert_eq!(
        titles(&fixes),
        ["Remove this `X` (line 4)", "Remove the other `x` (line 3)"]
    );
    let kept_first = applied(DUP, &fixes[0]);
    assert_eq!(kept_first, "class A\n{\n    x = 1;\n};\n");
    assert!(!codes(&kept_first).contains(&"MF-C330".into()));
    let kept_second = applied(DUP, &fixes[1]);
    assert_eq!(kept_second, "class A\n{\n    X = 2;\n};\n");
    assert!(!codes(&kept_second).contains(&"MF-C330".into()));
}

#[test]
fn c330_is_a_no_op_for_a_stale_span() {
    let stale = Diagnostic::at("MF-C330", Severity::Warning, Span::new(0, 5), "stale");
    assert!(fixes_for(DUP, &stale, &FixFacts::default()).is_empty());
}

const UNRESOLVED: &str = "class A: Missing {};\n";

#[test]
fn c340_declares_the_base_above_the_class() {
    let fixes = fixes_for(
        UNRESOLVED,
        &find(UNRESOLVED, "MF-C340"),
        &FixFacts::default(),
    );
    assert_eq!(titles(&fixes), ["Declare `class Missing;` above `A`"]);
    let out = applied(UNRESOLVED, &fixes[0]);
    assert_eq!(out, "class Missing;\nclass A: Missing {};\n");
    assert!(!codes(&out).contains(&"MF-C340".into()));
}

#[test]
fn c340_offers_the_required_addons_variant_when_a_patch_owns_the_base() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        units[] = {};\n    };\n};\nclass A: Missing {};\n";
    let facts = FixFacts {
        owner_patch_of_base: Some(("Missing".into(), "DZ_Data".into())),
        ..Default::default()
    };
    let fixes = fixes_for(text, &find(text, "MF-C340"), &facts);
    assert_eq!(
        titles(&fixes),
        [
            "Declare `class Missing;` above `A`",
            "Declare `Missing` and add `DZ_Data` to requiredAddons",
        ]
    );
    let out = applied(text, &fixes[1]);
    assert!(out.contains("requiredAddons[] = { \"DZ_Data\" };"), "{out}");
    assert!(!codes(&out).contains(&"MF-C340".into()));
}

#[test]
fn c340_skips_the_addons_variant_when_it_is_already_listed() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"dz_data\" };\n    };\n};\nclass A: Missing {};\n";
    let facts = FixFacts {
        owner_patch_of_base: Some(("Missing".into(), "DZ_Data".into())),
        ..Default::default()
    };
    let fixes = fixes_for(text, &find(text, "MF-C340"), &facts);
    assert_eq!(titles(&fixes).len(), 1);
}

#[test]
fn c340_gives_no_fix_when_the_base_is_declared_later_in_the_body() {
    let text = "class A\n{\n    class B: C\n    {\n    };\n    class C\n    {\n    };\n};\n";
    assert!(codes(text).contains(&"MF-C340".into()));
    assert!(fixes_for(text, &find(text, "MF-C340"), &FixFacts::default()).is_empty());
}

#[test]
fn c341_uses_the_declared_casing() {
    let text = "class Base {};\nclass A: base {};\n";
    let fixes = fixes_for(text, &find(text, "MF-C341"), &FixFacts::default());
    assert_eq!(titles(&fixes), ["Use `Base`"]);
    let out = applied(text, &fixes[0]);
    assert_eq!(out, "class Base {};\nclass A: Base {};\n");
    assert!(!codes(&out).contains(&"MF-C341".into()));
}

#[test]
fn c341_gives_no_fix_when_set_base_would_drop_a_sibling_parent() {
    let text = "class Base {};\nclass Other {};\nclass A: Other, base {};\n";
    assert!(fixes_for(text, &find(text, "MF-C341"), &FixFacts::default()).is_empty());
}

#[test]
fn c350_writes_the_brackets_the_schema_wants() {
    let array = "class CfgPatches\n{\n    class M\n    {\n        units = { \"a\" };\n    };\n};\n";
    let fixes = fixes_for(array, &find(array, "MF-C350"), &FixFacts::default());
    assert_eq!(titles(&fixes), ["Write `units[]`"]);
    let out = applied(array, &fixes[0]);
    assert_eq!(out, array.replace("units = ", "units[] = "));
    assert!(!codes(&out).contains(&"MF-C350".into()));

    let scalar =
        "class CfgPatches\n{\n    class M\n    {\n        requiredVersion[] = 0.1;\n    };\n};\n";
    let fixes = fixes_for(scalar, &find(scalar, "MF-C350"), &FixFacts::default());
    assert_eq!(titles(&fixes), ["Write `requiredVersion` without []"]);
    let out = applied(scalar, &fixes[0]);
    assert_eq!(
        out,
        scalar.replace("requiredVersion[] =", "requiredVersion =")
    );
    assert!(!codes(&out).contains(&"MF-C350".into()));
}

#[test]
fn c350_gives_no_fix_when_the_value_shape_does_not_fit() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        units = \"a\";\n    };\n};\n";
    assert!(codes(text).contains(&"MF-C350".into()), "{:?}", codes(text));
    assert!(fixes_for(text, &find(text, "MF-C350"), &FixFacts::default()).is_empty());
}

#[test]
fn c400_offers_one_fix_per_missing_patch_plus_add_all() {
    let facts = FixFacts {
        dependency_patches: vec![("DepMod".into(), vec!["P1".into(), "P2".into()])],
        ..Default::default()
    };
    let fixes = fixes_for(PATCHES, &c400("DepMod", &["P1", "P2"]), &facts);
    assert_eq!(
        titles(&fixes),
        [
            "Add `P1` to requiredAddons",
            "Add `P2` to requiredAddons",
            "Add all 2"
        ]
    );
    let one = applied(PATCHES, &fixes[0]);
    assert!(one.contains("\"P1\""), "{one}");
    assert!(!one.contains("\"P2\""), "{one}");
    let all = applied(PATCHES, &fixes[2]);
    assert!(all.contains("\"P1\"") && all.contains("\"P2\""), "{all}");
}

#[test]
fn c400_only_offers_the_patches_that_are_missing() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"P1\" };\n    };\n};\n";
    let facts = FixFacts {
        dependency_patches: vec![("DepMod".into(), vec!["P1".into(), "P2".into()])],
        ..Default::default()
    };
    let fixes = fixes_for(text, &c400("DepMod", &["P1", "P2"]), &facts);
    assert_eq!(titles(&fixes), ["Add `P2` to requiredAddons"]);
}

#[test]
fn c400_only_answers_for_the_mod_the_diagnostic_names() {
    let facts = FixFacts {
        dependency_patches: vec![
            ("DepA".into(), vec!["A1".into()]),
            ("DepB".into(), vec!["B1".into()]),
        ],
        ..Default::default()
    };
    let fixes = fixes_for(PATCHES, &c400("DepB", &["B1"]), &facts);
    assert_eq!(titles(&fixes), ["Add `B1` to requiredAddons"]);
}

#[test]
fn c400_gives_no_fix_when_every_patch_is_already_listed() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"P1\", \"P2\" };\n    };\n};\n";
    let facts = FixFacts {
        dependency_patches: vec![("DepMod".into(), vec!["P1".into(), "P2".into()])],
        ..Default::default()
    };
    assert!(fixes_for(text, &c400("DepMod", &["P1", "P2"]), &facts).is_empty());
}

#[test]
fn c340_skips_the_addons_variant_when_a_later_block_lists_the_patch() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        units[] = {};\n    };\n};\nclass CfgPatches\n{\n    class N\n    {\n        requiredAddons[] = { \"dz_data\" };\n    };\n};\nclass A: Missing {};\n";
    let facts = FixFacts {
        owner_patch_of_base: Some(("Missing".into(), "DZ_Data".into())),
        ..Default::default()
    };
    let fixes = fixes_for(text, &find(text, "MF-C340"), &facts);
    assert_eq!(titles(&fixes), ["Declare `class Missing;` above `A`"]);
}

#[test]
fn c400_gives_no_fix_for_patches_a_later_block_lists() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        units[] = {};\n    };\n};\nclass CfgPatches\n{\n    class N\n    {\n        requiredAddons[] = { \"P1\", \"P2\" };\n    };\n};\n";
    let facts = FixFacts {
        dependency_patches: vec![("DepMod".into(), vec!["P1".into(), "P2".into()])],
        ..Default::default()
    };
    assert!(fixes_for(text, &c400("DepMod", &["P1", "P2"]), &facts).is_empty());
}

#[test]
fn c340_skips_the_addons_variant_when_an_append_array_lists_the_patch() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"A\" };\n        requiredAddons[] += { \"dz_data\" };\n    };\n};\nclass A: Missing {};\n";
    let facts = FixFacts {
        owner_patch_of_base: Some(("Missing".into(), "DZ_Data".into())),
        ..Default::default()
    };
    let fixes = fixes_for(text, &find(text, "MF-C340"), &facts);
    assert_eq!(titles(&fixes), ["Declare `class Missing;` above `A`"]);
}

#[test]
fn c400_gives_no_fix_for_patches_an_append_array_lists() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"P1\" };\n        requiredAddons[] += { \"P2\" };\n    };\n};\n";
    let facts = FixFacts {
        dependency_patches: vec![("DepMod".into(), vec!["P1".into(), "P2".into()])],
        ..Default::default()
    };
    assert!(fixes_for(text, &c400("DepMod", &["P1", "P2"]), &facts).is_empty());
}

fn addons(text: &str, names: &[&str]) -> Result<Vec<modfoundry_config::edit::EditOp>, EditError> {
    let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    add_required_addons(text, None, &names)
}

fn add_all(text: &str, names: &[&str]) -> String {
    let ops = addons(text, names).expect("ops");
    plan_all(text, &ops).expect("plan_all")
}

#[test]
fn add_required_addons_creates_a_missing_array() {
    let out = add_all(PATCHES, &["DZ_Data"]);
    assert!(out.contains("requiredAddons[] = { \"DZ_Data\" };"), "{out}");
    assert!(validate(&out).iter().all(|d| d.severity < Severity::Error));
}

#[test]
fn add_required_addons_fills_an_empty_array() {
    let text =
        "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = {};\n    };\n};\n";
    let out = add_all(text, &["DZ_Data"]);
    assert_eq!(out, text.replace("{}", "{ \"DZ_Data\" }"));
}

#[test]
fn add_required_addons_appends_to_a_one_line_array() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"DZ_Data\" };\n    };\n};\n";
    let out = add_all(text, &["DZ_Scripts"]);
    assert_eq!(
        out,
        text.replace("\"DZ_Data\" }", "\"DZ_Data\", \"DZ_Scripts\" }")
    );
}

#[test]
fn add_required_addons_appends_a_line_to_a_multi_line_array() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] =\n        {\n            \"DZ_Data\",\n        };\n    };\n};\n";
    let out = add_all(text, &["DZ_Scripts"]);
    assert_eq!(
        out,
        "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] =\n        {\n            \"DZ_Data\",\n            \"DZ_Scripts\",\n        };\n    };\n};\n"
    );
}

#[test]
fn add_required_addons_appends_to_a_plus_equals_array() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] += { \"DZ_Data\" };\n    };\n};\n";
    let out = add_all(text, &["DZ_Scripts"]);
    assert!(out.contains("+="), "{out}");
    assert!(out.contains("\"DZ_Scripts\""), "{out}");
}

#[test]
fn add_required_addons_never_duplicates_a_name() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"DZ_Data\" };\n    };\n};\n";
    assert_eq!(add_all(text, &["dz_data"]), text);
    assert_eq!(
        add_all(text, &["DZ_Data", "DZ_Scripts"]),
        text.replace("\"DZ_Data\" }", "\"DZ_Data\", \"DZ_Scripts\" }")
    );
}

#[test]
fn add_required_addons_skips_a_name_a_later_plus_equals_lists() {
    let text = "class CfgPatches\n{\n    class M\n    {\n        requiredAddons[] = { \"A\" };\n        requiredAddons[] += { \"X\" };\n    };\n};\n";
    assert!(addons(text, &["X"]).expect("ops").is_empty());
}

#[test]
fn add_required_addons_can_target_a_named_patch_class() {
    let text = "class CfgPatches\n{\n    class A\n    {\n    };\n    class B\n    {\n    };\n};\n";
    let names = vec!["DZ_Data".to_owned()];
    let ops = add_required_addons(text, Some("B"), &names).expect("ops");
    let out = plan_all(text, &ops).expect("plan_all");
    let b = out.find("class B").unwrap();
    assert!(out[b..].contains("requiredAddons"), "{out}");
    assert!(!out[..b].contains("requiredAddons"), "{out}");
}

#[test]
fn add_required_addons_needs_a_patch_class() {
    assert!(matches!(
        addons("class CfgMods {};\n", &["DZ_Data"]).unwrap_err(),
        EditError::NotFound(_)
    ));
    let names = vec!["DZ_Data".to_owned()];
    assert!(matches!(
        add_required_addons(PATCHES, Some("Nope"), &names).unwrap_err(),
        EditError::NotFound(_)
    ));
}
