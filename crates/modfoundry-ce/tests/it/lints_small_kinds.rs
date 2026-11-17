//! The globals value rule — MF-X030.

use crate::common::fixtures::slice_opt;
use modfoundry_ce::lints::validate_text;
use modfoundry_ce::{Diagnostic, Severity};

fn findings(text: &str) -> Vec<Diagnostic> {
    validate_text(text, Some("globals.xml"))
}

fn var(name: &str, ty: &str, value: &str) -> String {
    format!(
        "<variables>\n    <var name=\"{name}\" type=\"{ty}\" value=\"{value}\"/>\n</variables>\n"
    )
}

#[test]
fn an_integer_variable_with_a_non_whole_value_is_flagged() {
    let text = var("ZombieMaxCount", "0", "1k");
    let found = findings(&text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X030");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(slice_opt(&text, found[0].span), "1k");
    assert_eq!(
        found[0].message,
        "`ZombieMaxCount` is an integer variable, so its value must be a whole number, not `1k`."
    );
}

#[test]
fn a_whole_number_integer_variable_is_clean() {
    assert!(findings(&var("ZombieMaxCount", "0", "1000")).is_empty());
}

#[test]
fn a_float_variable_takes_a_decimal() {
    assert!(findings(&var("CleanupLifetimeDefault", "1", "0.5")).is_empty());
}

#[test]
fn a_float_variable_with_a_non_number_is_flagged() {
    let text = var("CleanupLifetimeDefault", "1", "1k");
    let found = findings(&text);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "MF-X030");
    assert_eq!(slice_opt(&text, found[0].span), "1k");
    assert_eq!(
        found[0].message,
        "`CleanupLifetimeDefault` is a float variable, so its value must be a number, not `1k`."
    );
}
