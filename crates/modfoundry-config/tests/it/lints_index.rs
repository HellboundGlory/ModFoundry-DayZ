//! The index lint MF-C410 and the MF-C400 dependency check.

use modfoundry_config::fixes::{fixes_for, FixFacts};
use modfoundry_config::lints::{dependency_findings, index_findings};
use modfoundry_config::parser::parse;
use modfoundry_config::resolve::{ConfigOrigin, Resolver, SourceConfig};
use modfoundry_config::{Diagnostic, Severity};

fn resolver(configs: &[(&str, ConfigOrigin)]) -> Resolver {
    Resolver::new(
        configs
            .iter()
            .enumerate()
            .map(|(i, (text, origin))| SourceConfig {
                id: format!("source:{i}"),
                display: format!("{i}.pbo"),
                vpath: format!("p{i}/config.bin"),
                origin: *origin,
                file: parse(text).file,
            })
            .collect(),
    )
}

fn game() -> Resolver {
    resolver(&[(
        "class CfgPatches { class DZ_A {}; class DZ_B {}; };\n",
        ConfigOrigin::Vanilla,
    )])
}

fn patches(required: &str) -> String {
    format!("class CfgPatches {{ class Mine {{ requiredAddons[] = {{ {required} }}; }}; }};\n")
}

fn snippets<'a>(text: &'a str, found: &[Diagnostic]) -> Vec<&'a str> {
    found
        .iter()
        .map(|d| {
            let span = d.span.expect("finding has a span");
            &text[span.start..span.end]
        })
        .collect()
}

#[test]
fn a_declared_addon_raises_nothing() {
    let text = patches("\"DZ_A\", \"DZ_B\"");
    assert!(index_findings(&text, &game(), None).is_empty());
}

#[test]
fn an_unknown_addon_is_a_c410_warning_on_its_item() {
    let text = patches("\"DZ_A\", \"Nope\"");
    let found = index_findings(&text, &game(), None);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-C410");
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(found[0].message.contains("`Nope`"), "{}", found[0].message);
    assert_eq!(snippets(&text, &found), vec!["\"Nope\""]);
}

#[test]
fn every_unknown_item_gets_its_own_finding() {
    let text = patches("\"Nope\", \"DZ_A\", \"Gone\"");
    let found = index_findings(&text, &game(), None);

    assert_eq!(snippets(&text, &found), vec!["\"Nope\"", "\"Gone\""]);
}

#[test]
fn a_name_the_file_declares_itself_is_known() {
    let text = "\
class CfgPatches { class First {}; class Second { requiredAddons[] = { \"First\", \"DZ_A\" }; }; };
";
    assert!(index_findings(text, &game(), None).is_empty());
}

#[test]
fn a_name_declared_in_a_later_cfgpatches_block_is_known() {
    let text = "class CfgPatches { class A { requiredAddons[] = {\"B\"}; }; };\nclass CfgPatches { class B {}; };\n";
    assert!(index_findings(text, &game(), None).is_empty());
}

#[test]
fn the_files_own_stored_patch_names_do_not_count() {
    let stored = resolver(&[(
        "class CfgPatches { class Old {}; };\n",
        ConfigOrigin::Workspace,
    )]);
    let text = "class CfgPatches { class New { requiredAddons[] = {\"Old\"}; }; };\n";
    assert_eq!(index_findings(text, &stored, Some("source:0")).len(), 1);
}

#[test]
fn matching_ignores_case() {
    let text = patches("\"dz_a\", \"DZ_b\"");
    assert!(index_findings(&text, &game(), None).is_empty());
}

#[test]
fn an_item_in_an_append_array_is_checked() {
    let text = "\
class CfgPatches { class Mine { requiredAddons[] += { \"Nope\" }; }; };
";
    let found = index_findings(text, &game(), None);

    assert_eq!(snippets(text, &found), vec!["\"Nope\""]);
}

#[test]
fn a_vanilla_source_makes_the_addons_it_requires_known() {
    let text = patches("\"DZ_Hidden\"");
    let source =
        "class CfgPatches { class DZ_Other { requiredAddons[] = { \"dz_hidden\" }; }; };\n";
    for (origin, expected) in [(ConfigOrigin::Vanilla, 0), (ConfigOrigin::Workspace, 1)] {
        let found = index_findings(&text, &resolver(&[(source, origin)]), None);
        assert_eq!(found.len(), expected, "{origin:?}: {found:?}");
    }
}

#[test]
fn a_file_without_cfgpatches_raises_nothing() {
    assert!(index_findings("class CfgVehicles { class A {}; };\n", &game(), None).is_empty());
}

#[test]
fn an_unknown_addon_in_a_later_cfgpatches_block_is_checked() {
    let text = "class CfgPatches { class A { requiredAddons[] = {\"DZ_A\"}; }; };\nclass CfgPatches { class B { requiredAddons[] = {\"Nope\"}; }; };\n";
    let found = index_findings(text, &game(), None);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-C410");
    assert_eq!(snippets(text, &found), vec!["\"Nope\""]);
}

#[test]
fn an_addon_listed_in_a_later_cfgpatches_block_satisfies_the_dependency() {
    let text = "class CfgPatches { class A { requiredAddons[] = {\"DZ_A\"}; }; };\nclass CfgPatches { class B { requiredAddons[] = {\"JM_CF\"}; }; };\n";
    assert!(dependency_findings(text, &[dep("CF", &["JM_CF"])]).is_empty());
}

#[test]
fn the_c400_span_is_the_first_required_addons_of_any_block() {
    let text = "class CfgPatches { class A {}; };\nclass CfgPatches { class B { requiredAddons[] = {\"X\"}; }; };\n";
    let found = dependency_findings(text, &[dep("CF", &["JM_CF"])]);

    assert_eq!(snippets(text, &found), vec!["requiredAddons"]);
}

#[test]
fn an_item_in_an_append_array_satisfies_the_dependency() {
    let text = "\
class CfgPatches { class Mine { requiredAddons[] = { \"DZ_A\" }; requiredAddons[] += { \"JM_CF\" }; }; };
";
    assert!(dependency_findings(text, &[dep("CF", &["JM_CF"])]).is_empty());
}

fn dep(name: &str, patches: &[&str]) -> (String, Vec<String>) {
    (
        name.to_string(),
        patches.iter().map(|p| (*p).to_string()).collect(),
    )
}

#[test]
fn a_dependency_with_no_addon_listed_is_a_c400_warning() {
    let text = patches("\"DZ_A\"");
    let found = dependency_findings(&text, &[dep("Community Framework", &["JM_CF", "RPC"])]);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-C400");
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(
        found[0]
            .message
            .starts_with("`Community Framework` is marked as a dependency"),
        "{}",
        found[0].message
    );
    assert!(found[0].message.contains("`JM_CF`"), "{}", found[0].message);
    assert!(found[0].message.contains("`RPC`"), "{}", found[0].message);
    assert_eq!(snippets(&text, &found), vec!["requiredAddons"]);
}

#[test]
fn one_listed_addon_satisfies_the_dependency() {
    let text = patches("\"DZ_A\", \"rpc\"");
    assert!(dependency_findings(&text, &[dep("CF", &["JM_CF", "RPC"])]).is_empty());
}

#[test]
fn a_dependency_with_no_patches_is_not_reported() {
    let text = patches("\"DZ_A\"");
    assert!(dependency_findings(&text, &[dep("Empty", &[])]).is_empty());
}

#[test]
fn each_unsatisfied_dependency_is_reported_once() {
    let text = patches("\"DZ_A\"");
    let found = dependency_findings(&text, &[dep("One", &["A1"]), dep("Two", &["B1"])]);

    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].message.starts_with("`One`"));
    assert!(found[1].message.starts_with("`Two`"));
}

#[test]
fn the_span_falls_back_to_the_first_patch_class_then_to_none() {
    let no_required = "class CfgPatches { class Mine {}; };\n";
    let found = dependency_findings(no_required, &[dep("CF", &["JM_CF"])]);
    assert_eq!(snippets(no_required, &found), vec!["Mine"]);

    let found = dependency_findings("class CfgVehicles {};\n", &[dep("CF", &["JM_CF"])]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].span, None);
}

#[test]
fn a_c400_finding_feeds_the_fix_for_the_same_mod() {
    let text = patches("\"DZ_A\"");
    let facts = FixFacts {
        dependency_patches: vec![
            dep("Other", &["Other_P"]),
            dep("Community Framework", &["JM_CF"]),
        ],
        ..FixFacts::default()
    };
    let found = dependency_findings(&text, &[dep("Community Framework", &["JM_CF"])]);

    let fixes = fixes_for(&text, &found[0], &facts);

    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert!(fixes[0].title.contains("JM_CF"), "{}", fixes[0].title);
}
