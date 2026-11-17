//! The schema-driven file lints — MF-X020, X021, X022, X030, X031, X040.

use crate::common::fixtures::{slice_opt, SAMPLE_TYPES};
use modfoundry_ce::lints::{nearest, schema_findings};
use modfoundry_ce::model::read_doc;
use modfoundry_ce::xml::parse;
use modfoundry_ce::Diagnostic;

fn findings(text: &str) -> Vec<Diagnostic> {
    let doc = parse(text);
    let document = read_doc(text, &doc, None);
    schema_findings(&doc, &document)
}

fn with_code<'a>(findings: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    findings.iter().filter(|d| d.code == code).collect()
}

#[test]
fn the_sample_and_a_partial_record_are_clean() {
    assert_eq!(findings(SAMPLE_TYPES), Vec::new());
    let partial =
        "<types><type name=\"X\"><lifetime>60</lifetime><flags deloot=\"1\"/></type></types>";
    assert_eq!(findings(partial), Vec::new());
}

#[test]
fn nothing_for_no_kind_or_a_source_only_kind() {
    assert_eq!(findings("<Trader></Trader>"), Vec::new());
    assert_eq!(findings("<env></env>"), Vec::new());
}

#[test]
fn a_fragment_without_a_root_is_flagged() {
    let text = "<type name=\"A\"><nominal>1</nominal></type>\n\
                <type name=\"B\"><nominal>2</nominal></type>\n";
    let found = findings(text);
    let x020 = with_code(&found, "MF-X020");
    assert_eq!(x020.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x020[0].span), "type");
    assert_eq!(
        x020[0].message,
        "This file has no `<types>` root, so a server can't load it on its own. Wrap the entries in `<types>`."
    );

    let rooted = "<types><type name=\"A\"><nominal>1</nominal></type></types>";
    assert!(with_code(&findings(rooted), "MF-X020").is_empty());
}

#[test]
fn a_root_mismatch_is_flagged_at_the_root_name() {
    let text = "<types><type name=\"X\"><cargo/></type></types>";
    let found = findings(text);
    let x020 = with_code(&found, "MF-X020");
    assert_eq!(x020.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x020[0].span), "types");
    assert_eq!(
        x020[0].message,
        "These are spawnable types, but the root is `<types>`. Rename it to `<spawnabletypes>`."
    );
}

#[test]
fn an_unknown_child_is_flagged_with_the_nearest_name() {
    let text = "<types><type name=\"A\"><nominal>1</nominal><nominel>7</nominel></type></types>";
    let found = findings(text);
    let x021 = with_code(&found, "MF-X021");
    assert_eq!(x021.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x021[0].span), "nominel");
    assert_eq!(
        x021[0].message,
        "`<nominel>` isn't part of a `<type>` here. Did you mean `<nominal>`?"
    );
    assert_eq!(x021[0].severity, modfoundry_ce::Severity::Warning);
}

#[test]
fn a_misspelt_attachments_child_in_a_spawnable_type_is_flagged() {
    let text = "<spawnabletypes>\n    <type name=\"X\">\n        <attachement chance=\"0.5\"/>\n    </type>\n</spawnabletypes>\n";
    let found = findings(text);
    let x021 = with_code(&found, "MF-X021");
    assert_eq!(x021.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x021[0].span), "attachement");
    assert_eq!(
        x021[0].message,
        "`<attachement>` isn't part of a `<type>` here. Did you mean `<attachments>`?"
    );
}

#[test]
fn a_case_only_child_is_flagged_at_its_own_name() {
    let text = "<types><type name=\"A\"><nominal>1</nominal><Nominal>7</Nominal></type></types>";
    let found = findings(text);
    let x021 = with_code(&found, "MF-X021");
    assert_eq!(x021.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x021[0].span), "Nominal");
    assert_eq!(
        x021[0].message,
        "`<Nominal>` isn't part of a `<type>` here. Did you mean `<nominal>`?"
    );
}

#[test]
fn an_unknown_child_with_no_nearest_has_no_question() {
    let text = "<types><type name=\"A\"><nominal>1</nominal><zzzzz/></type></types>";
    let found = findings(text);
    let x021 = with_code(&found, "MF-X021");
    assert_eq!(x021.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x021[0].span), "zzzzz");
    assert_eq!(x021[0].message, "`<zzzzz>` isn't part of a `<type>` here.");
    assert!(!x021[0].message.contains("Did you mean"));
}

#[test]
fn an_unknown_child_attribute_on_a_list_child_is_flagged() {
    let text = "<types><type name=\"A\"><flags count_in_mapp=\"1\"/></type></types>";
    let found = findings(text);
    let x022 = with_code(&found, "MF-X022");
    assert_eq!(x022.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x022[0].span), "count_in_mapp");
    assert_eq!(
        x022[0].message,
        "`<flags>` has no `count_in_mapp` attribute. Did you mean `count_in_map`?"
    );
    assert_eq!(x022[0].severity, modfoundry_ce::Severity::Hint);

    let ok = "<types><type name=\"A\"><flags count_in_map=\"1\"/></type></types>";
    assert!(with_code(&findings(ok), "MF-X022").is_empty());
}

#[test]
fn an_unknown_attribute_on_a_record_is_flagged() {
    let text = "<types><type name=\"A\" extra=\"1\"/></types>";
    let found = findings(text);
    let x022 = with_code(&found, "MF-X022");
    assert_eq!(x022.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x022[0].span), "extra");
    assert_eq!(x022[0].message, "`<type>` has no `extra` attribute.");
    assert!(!x022[0].message.contains("Did you mean"));
}

#[test]
fn an_unknown_attribute_on_a_list_item_is_flagged() {
    let text = "<types><type name=\"A\"><usage name=\"Village\" bogus=\"1\"/></type></types>";
    let found = findings(text);
    let x022 = with_code(&found, "MF-X022");
    assert_eq!(x022.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x022[0].span), "bogus");
    assert_eq!(x022[0].message, "`<usage>` has no `bogus` attribute.");
}

#[test]
fn an_integer_flag_and_enum_that_do_not_fit_are_flagged() {
    let integer = "<types><type name=\"A\"><nominal>abc</nominal></type></types>";
    let found = findings(integer);
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(integer, x030[0].span), "abc");
    assert_eq!(
        x030[0].message,
        "`nominal` must be a whole number, not `abc`."
    );

    let flag = "<types><type name=\"A\"><flags crafted=\"3\"/></type></types>";
    let found = findings(flag);
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(flag, x030[0].span), "3");
    assert_eq!(x030[0].message, "`crafted` must be 0 or 1, not `3`.");

    let enumeration = "<economycore><classes><rootclass name=\"CfgVehicles\" act=\"plane\"/></classes></economycore>";
    let found = findings(enumeration);
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(enumeration, x030[0].span), "plane");
    assert_eq!(
        x030[0].message,
        "`act` must be one of `none`, `character`, `car`, not `plane`."
    );
}

#[test]
fn a_record_without_its_key_attribute_is_flagged() {
    let text = "<types><type><nominal>1</nominal></type></types>";
    let found = findings(text);
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x030[0].span), "type");
    assert_eq!(x030[0].message, "`<type>` needs a `name`.");
}

#[test]
fn values_out_of_range_are_flagged() {
    let percent = "<types><type name=\"A\"><quantmax>150</quantmax></type></types>";
    let found = findings(percent);
    let x031 = with_code(&found, "MF-X031");
    assert_eq!(x031.len(), 1, "{found:?}");
    assert_eq!(slice_opt(percent, x031[0].span), "150");
    assert_eq!(
        x031[0].message,
        "`quantmax` must be a percentage from -1 to 100, not `150`."
    );

    let negative = "<types><type name=\"A\"><nominal>-1</nominal></type></types>";
    let found = findings(negative);
    let x031 = with_code(&found, "MF-X031");
    assert_eq!(x031.len(), 1, "{found:?}");
    assert_eq!(slice_opt(negative, x031[0].span), "-1");
    assert_eq!(x031[0].message, "`nominal` must be 0 or more, not `-1`.");
}

#[test]
fn a_repeated_key_is_flagged_on_the_later_records() {
    let text = "<types>\n    <type name=\"A\"/>\n    <type name=\"a\"/>\n    <type name=\"A\"/>\n</types>\n";
    let found = findings(text);
    let x040 = with_code(&found, "MF-X040");
    assert_eq!(x040.len(), 2, "{found:?}");
    assert_eq!(slice_opt(text, x040[0].span), "a");
    assert_eq!(
        x040[0].message,
        "`a` is defined twice in this file (also on line 2). Keep one."
    );
    assert_eq!(slice_opt(text, x040[1].span), "A");
    assert_eq!(
        x040[1].message,
        "`A` is defined twice in this file (also on line 2). Keep one."
    );

    let ok = "<types><type name=\"A\"/><type name=\"B\"/></types>";
    assert!(with_code(&findings(ok), "MF-X040").is_empty());
}

#[test]
fn findings_are_sorted_by_span_start() {
    let text = "<types>\n    <type name=\"A\" extra=\"1\"><nominal>1</nominal><nominel>7</nominel></type>\n</types>\n";
    let found = findings(text);
    assert!(found.len() >= 2);
    let starts: Vec<usize> = found
        .iter()
        .filter_map(|d| d.span.map(|s| s.start))
        .collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted);
}

#[test]
fn nearest_matches_case_only_distance_one_two_and_none() {
    let known = ["nominal"];
    assert_eq!(nearest("Nominal", &known), Some("nominal"));
    assert_eq!(nearest("nominel", &known), Some("nominal"));
    assert_eq!(nearest("nomin", &known), Some("nominal"));
    assert_eq!(nearest("nomi", &known), None);
    assert_eq!(nearest("zzzzz", &known), None);
}

fn cargo_item(children: &str) -> String {
    format!(
        "<spawnabletypes>\n    <type name=\"Box\">\n        <cargo chance=\"1.0\">\n            <item name=\"Case\" chance=\"0.5\">{children}</item>\n        </cargo>\n    </type>\n</spawnabletypes>\n"
    )
}

#[test]
fn an_item_damage_out_of_range_is_flagged() {
    let text = cargo_item("<damage min=\"0.0\" max=\"2\"/>");
    let found = findings(&text);
    let x031 = with_code(&found, "MF-X031");
    assert_eq!(x031.len(), 1, "{found:?}");
    assert_eq!(slice_opt(&text, x031[0].span), "2");
    assert_eq!(x031[0].message, "`max` must be 1 or less, not `2`.");
}

#[test]
fn an_unknown_attribute_on_item_damage_is_flagged() {
    let text = cargo_item("<damage mn=\"0.1\" max=\"0.2\"/>");
    let found = findings(&text);
    let x022 = with_code(&found, "MF-X022");
    assert_eq!(x022.len(), 1, "{found:?}");
    assert_eq!(slice_opt(&text, x022[0].span), "mn");
    assert_eq!(
        x022[0].message,
        "`<damage>` has no `mn` attribute. Did you mean `min`?"
    );
}

#[test]
fn a_nested_cargo_item_is_checked_like_a_top_level_one() {
    let text = cargo_item(
        "<damage min=\"0.0\" max=\"0.0\"/><cargo chanse=\"1.0\"><item name=\"SCARH\" chance=\"x\"/></cargo>",
    );
    let found = findings(&text);
    let x022 = with_code(&found, "MF-X022");
    assert_eq!(x022.len(), 1, "{found:?}");
    assert_eq!(slice_opt(&text, x022[0].span), "chanse");
    assert_eq!(
        x022[0].message,
        "`<cargo>` has no `chanse` attribute. Did you mean `chance`?"
    );
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(&text, x030[0].span), "x");
    assert_eq!(x030[0].message, "`chance` must be a number, not `x`.");
}

#[test]
fn the_vanilla_storage_box_shape_is_clean() {
    let text = "<spawnabletypes>\n\
                \x20   <type name=\"StorageBox_Wooden\">\n\
                \x20       <cargo chance=\"1.0\">\n\
                \x20           <item name=\"GunCase_Black\" chance=\"0.5\">\n\
                \x20               <damage min=\"0.0\" max=\"0.0\" />\n\
                \x20               <cargo chance=\"1.0\">\n\
                \x20                   <item name=\"SCARH\" chance=\"1.0\" />\n\
                \x20               </cargo>\n\
                \x20               <cargo chance=\"1.0\">\n\
                \x20                   <item name=\"SCAR_PrecisionBttstck\" chance=\"0.50\" />\n\
                \x20                   <item name=\"SCAR_StockBttstck\" chance=\"0.50\" />\n\
                \x20               </cargo>\n\
                \x20           </item>\n\
                \x20           <item name=\"GunCase_White\" chance=\"0.5\">\n\
                \x20               <damage min=\"0.0\" max=\"0.0\" />\n\
                \x20               <cargo chance=\"1.0\"> <item name=\"SV98\" chance=\"1.0\" /> </cargo>\n\
                \x20               <cargo chance=\"1.0\"> <item name=\"SV98_Bttstck\" chance=\"1.0\" /> </cargo>\n\
                \x20           </item>\n\
                \x20       </cargo>\n\
                \x20   </type>\n\
                </spawnabletypes>\n";
    assert_eq!(findings(text), Vec::new());
}

#[test]
fn an_attachments_item_is_not_given_damage_or_cargo_rules() {
    let text = "<spawnabletypes>\n    <type name=\"Box\">\n        <attachments chance=\"1.0\">\n            <item name=\"Mag\"><damage min=\"9\" max=\"9\"/></item>\n        </attachments>\n    </type>\n</spawnabletypes>\n";
    assert_eq!(findings(text), Vec::new());
}

#[test]
fn an_unknown_event_position_is_flagged() {
    let text = "<events>\n    <event name=\"X\">\n        <position>sideways</position>\n    </event>\n</events>\n";
    let found = findings(text);
    let x030 = with_code(&found, "MF-X030");
    assert_eq!(x030.len(), 1, "{found:?}");
    assert_eq!(slice_opt(text, x030[0].span), "sideways");
    assert_eq!(
        x030[0].message,
        "`position` must be one of `fixed`, `player`, `uniform`, not `sideways`."
    );
}
