//! The types rules — MF-X110, MF-X111, MF-X112 — and `validate_text`.

use crate::common::fixtures::{slice_opt, SAMPLE_TYPES};
use modfoundry_ce::lints::{types_findings, validate_text};
use modfoundry_ce::model::read_doc;
use modfoundry_ce::xml::parse;
use modfoundry_ce::{Diagnostic, Severity};

fn types(text: &str) -> Vec<Diagnostic> {
    let doc = parse(text);
    let document = read_doc(text, &doc, None);
    types_findings(&doc, &document)
}

fn with_code<'a>(findings: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    findings.iter().filter(|d| d.code == code).collect()
}

fn record(nominal: &str, min: &str) -> String {
    format!("<types><type name=\"X\"><nominal>{nominal}</nominal><min>{min}</min></type></types>")
}

#[test]
fn min_above_nominal_fires_at_the_min_value() {
    let text = record("20", "24");
    let found = types(&text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X110");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(slice_opt(&text, found[0].span), "24");
    assert_eq!(
        found[0].message,
        "`X`: min (24) is above nominal (20). The economy never reaches min."
    );
}

#[test]
fn min_at_or_below_nominal_does_not_fire() {
    assert!(types(&record("20", "5")).is_empty());
    assert!(types(&record("20", "20")).is_empty());
    assert!(types(&record("20", "0")).is_empty());
}

#[test]
fn nominal_zero_is_never_flagged() {
    assert!(types(&record("0", "5")).is_empty());
}

#[test]
fn an_invalid_nominal_is_only_x030() {
    let text = record("abc", "24");
    let found = types(&text);
    assert_eq!(with_code(&found, "MF-X110").len(), 0, "{found:?}");
}

#[test]
fn an_invalid_nominal_beside_a_comment_is_reported() {
    let text = record("abc<!-- c -->", "24");
    let found = validate_text(&text, None);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X030");
    assert_eq!(slice_opt(&text, found[0].span), "abc");
}

#[test]
fn quantmin_above_quantmax_fires_at_quantmin() {
    let text =
        "<types><type name=\"X\"><quantmin>80</quantmin><quantmax>0</quantmax></type></types>";
    let found = types(text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X111");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(slice_opt(text, found[0].span), "80");
    assert_eq!(
        found[0].message,
        "`X`: quantmin (80) is above quantmax (0)."
    );
}

#[test]
fn only_one_negative_quant_fires_at_that_one() {
    let text =
        "<types><type name=\"X\"><quantmin>-1</quantmin><quantmax>0</quantmax></type></types>";
    let found = types(text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X111");
    assert_eq!(slice_opt(text, found[0].span), "-1");
    assert_eq!(
        found[0].message,
        "`X`: set both quantmin and quantmax to -1, or neither."
    );

    let other =
        "<types><type name=\"X\"><quantmin>0</quantmin><quantmax>-1</quantmax></type></types>";
    let found = types(other);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(slice_opt(other, found[0].span), "-1");
}

#[test]
fn matching_quant_values_are_clean() {
    assert!(types(
        "<types><type name=\"X\"><quantmin>-1</quantmin><quantmax>-1</quantmax></type></types>"
    )
    .is_empty());
    assert!(types(
        "<types><type name=\"X\"><quantmin>0</quantmin><quantmax>0</quantmax></type></types>"
    )
    .is_empty());
    assert!(types(
        "<types><type name=\"X\"><quantmin>40</quantmin><quantmax>80</quantmax></type></types>"
    )
    .is_empty());
}

#[test]
fn a_second_category_is_a_hint_at_its_name() {
    let text = "<types><type name=\"X\"><category name=\"tools\"/><category name=\"weapons\"/></type></types>";
    let found = types(text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X112");
    assert_eq!(found[0].severity, Severity::Hint);
    assert_eq!(slice_opt(text, found[0].span), "category");
    assert_eq!(found[0].message, "`X` has 2 categories. A type has one.");
}

#[test]
fn one_category_is_clean() {
    assert!(types("<types><type name=\"X\"><category name=\"tools\"/></type></types>").is_empty());
}

#[test]
fn validate_text_on_the_sample_types_is_one_finding() {
    let found = validate_text(SAMPLE_TYPES, None);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X110");
    assert_eq!(
        found[0].message,
        "`FilterCartridge`: min (24) is above nominal (20). The economy never reaches min."
    );
}

#[test]
fn validate_text_on_a_broken_tree_is_only_x001() {
    let text = "<types><type name=\"X\"><nominal>20</nominal><min>24</min></type>";
    let found = validate_text(text, None);
    let codes: Vec<&str> = found.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, ["MF-X001"], "{found:?}");
}

#[test]
fn validate_text_on_a_fragment_adds_the_types_findings() {
    let text = "<type name=\"X\"><nominal>20</nominal><min>24</min></type>";
    let found = validate_text(text, None);
    let codes: Vec<&str> = found.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, ["MF-X020", "MF-X110"], "{found:?}");
}

#[test]
fn validate_text_on_a_non_ce_file_is_only_its_xml_issues() {
    let text = "<Trader><!-- a -- b --></Trader>";
    let found = validate_text(text, None);
    let codes: Vec<&str> = found.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(codes, ["MF-X010"], "{found:?}");
}
