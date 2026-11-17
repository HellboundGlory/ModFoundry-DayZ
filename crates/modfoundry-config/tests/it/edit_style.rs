//! `infer_style` and `render_value` against each style fixture.

use crate::edit_fixtures as fixtures;

use fixtures::*;
use modfoundry_config::edit::*;

fn style(
    indent: &str,
    unit: &str,
    eol: &'static str,
    allman: bool,
    spaced_eq: bool,
    padded_braces: bool,
    multiline_arrays: bool,
) -> Style {
    Style {
        indent: indent.into(),
        unit: unit.into(),
        eol,
        allman,
        spaced_eq,
        padded_braces,
        multiline_arrays,
    }
}

#[test]
fn canonical_allman() {
    assert_eq!(
        infer_style(CANON, Some(&path(&ITEM))),
        style("        ", "    ", "\n", true, true, true, false)
    );
}

#[test]
fn indent_unit_and_eol_follow_the_file() {
    let tabs = restyle(CANON, "\t", true);
    assert_eq!(
        infer_style(&tabs, Some(&path(&ITEM))),
        style("\t\t", "\t", "\r\n", true, true, true, false)
    );
    let two = restyle(CANON, "  ", false);
    assert_eq!(
        infer_style(&two, Some(&path(&ITEM))),
        style("    ", "  ", "\n", true, true, true, false)
    );
}

#[test]
fn knr_braces() {
    assert_eq!(
        infer_style(KNR, Some(&path(&ITEM))),
        style("        ", "    ", "\n", false, true, true, false)
    );
}

#[test]
fn one_line_classes() {
    assert_eq!(
        infer_style(ONE_LINE, None),
        style("", "    ", "\n", false, true, true, false)
    );
    assert_eq!(
        infer_style(ONE_LINE, Some(&path(&["A"]))).indent,
        "    ",
        "no member sits on its own line, so the class indent plus the unit"
    );
}

#[test]
fn derapified_vanilla() {
    let text = crlf(VANILLA_LF);
    assert_eq!(
        infer_style(&text, Some(&path(&MYMOD))),
        style("\t\t", "\t", "\r\n", true, false, false, true)
    );
    assert_eq!(
        infer_style(&text, Some(&path(&ITEM))),
        style("\t\t", "\t", "\r\n", true, false, false, true)
    );
}

#[test]
fn scaffold() {
    assert_eq!(
        infer_style(
            &scaffold_config(),
            Some(&path(&["CfgPatches", "MyMod_Scripts"]))
        ),
        style("        ", "    ", "\n", true, true, true, false)
    );
}

#[test]
fn class_scope_wins_over_the_file() {
    let text = "class A\n{\n    x=1;\n    class B\n    {\n\ty = 2;\n    };\n};\nclass C\n{\n    z = 3;\n};\n";
    assert!(!infer_style(text, Some(&path(&["A"]))).spaced_eq);
    assert!(infer_style(text, Some(&path(&["C"]))).spaced_eq);
    assert_eq!(infer_style(text, Some(&path(&["A", "B"]))).indent, "\t");
}

#[test]
fn mixed_eols_take_the_majority() {
    let text = "class A\r\n{\r\n    x = 1;\r\n    y = 2;\n};\r\n";
    assert_eq!(infer_style(text, None).eol, "\r\n");
}

#[test]
fn an_empty_class_body_indents_one_unit_in() {
    let text = "class A\n{\n};\n";
    assert_eq!(infer_style(text, Some(&path(&["A"]))).indent, "    ");
}

#[test]
fn fallbacks_are_what_mf_new_writes() {
    let expected = style("", "    ", "\n", true, true, true, false);
    assert_eq!(infer_style("", None), expected);
    assert_eq!(infer_style("", Some(&path(&["Missing"]))), expected);
    assert_eq!(
        infer_style("class A\n{\n};\n", Some(&path(&["Nope"]))).indent,
        "",
        "an unknown class is treated as the file root"
    );
}

fn plain() -> Style {
    style("    ", "    ", "\n", true, true, true, false)
}

#[test]
fn renders_scalars_and_inline_arrays() {
    let padded = plain();
    assert_eq!(render_value(&s("a b"), &padded).unwrap(), "\"a b\"");
    assert_eq!(render_value(&n("-1.5e3"), &padded).unwrap(), "-1.5e3");
    assert_eq!(render_value(&id("mod"), &padded).unwrap(), "mod");
    assert_eq!(render_value(&arr(vec![]), &padded).unwrap(), "{}");
    assert_eq!(
        render_value(&arr(vec![s("a"), n("2")]), &padded).unwrap(),
        "{ \"a\", 2 }"
    );
    let tight = Style {
        padded_braces: false,
        ..plain()
    };
    assert_eq!(
        render_value(&arr(vec![s("a"), n("2")]), &tight).unwrap(),
        "{\"a\", 2}"
    );
    assert_eq!(
        render_value(&arr(vec![arr(vec![n("1")]), arr(vec![])]), &padded).unwrap(),
        "{ { 1 }, {} }"
    );
}

#[test]
fn renders_multi_line_arrays_at_the_style_indent() {
    let multi = Style {
        multiline_arrays: true,
        indent: "\t\t".into(),
        unit: "\t".into(),
        eol: "\r\n",
        ..plain()
    };
    assert_eq!(
        render_value(&arr(vec![s("a"), s("b")]), &multi).unwrap(),
        "{\r\n\t\t\t\"a\",\r\n\t\t\t\"b\"\r\n\t\t}"
    );
    assert_eq!(render_value(&arr(vec![]), &multi).unwrap(), "{}");
}

#[test]
fn rejects_unwritable_values() {
    let st = plain();
    let bad_number = |v: &str| render_value(&n(v), &st).is_err();
    for ok in ["0", "-1", "1.5", ".5", "5.", "1e5", "1E-5", "-.5e+3", "10"] {
        assert!(!bad_number(ok), "{ok} should be a number");
    }
    for bad in ["", "-", "1..2", "e5", "0x10", "1e", "+1", "1 2", "abc"] {
        assert!(bad_number(bad), "{bad:?} should not be a number");
    }
    assert!(render_value(&id("a b"), &st).is_err());
    assert!(render_value(&id("1a"), &st).is_err());
    assert!(render_value(&id(""), &st).is_err());
    assert!(render_value(&s("a\"b"), &st).is_err());
    assert!(render_value(&arr(vec![s("a\nb")]), &st).is_err());
}
