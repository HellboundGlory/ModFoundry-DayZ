//! Configs saved with a UTF-8 byte-order mark.

use modfoundry_config::edit::{plan_all, ConfigPath, EditOp, NewValue, Position, Target};
use modfoundry_config::highlight::{highlight, ConfigToken};
use modfoundry_config::parser::parse;
use modfoundry_config::rap::to_text;

const PLAIN: &str = "class CfgPatches { class A { units[] = {}; }; };";

#[test]
fn a_leading_byte_order_mark_parses_like_none() {
    let plain = parse(PLAIN);
    let marked = parse(&format!("\u{feff}{PLAIN}"));

    assert!(marked.issues.is_empty(), "{:?}", marked.issues);
    assert_eq!(to_text(&marked.file), to_text(&plain.file));
    assert_eq!(
        marked.file.members[0].name_span().start,
        plain.file.members[0].name_span().start + 3
    );
}

#[test]
fn a_byte_order_mark_inside_a_config_is_still_an_error() {
    let parsed = parse("class A { };\u{feff}class B { };");

    assert!(!parsed.issues.is_empty());
}

#[test]
fn an_edit_keeps_the_byte_order_mark() {
    let text = "\u{feff}class A { x = 1; };";
    let op = EditOp::SetValue {
        property: Target::Property {
            class: ConfigPath::of(&["A"]),
            name: "x".into(),
        },
        value: NewValue::Number("2".into()),
    };

    assert_eq!(
        plan_all(text, &[op]).unwrap(),
        "\u{feff}class A { x = 2; };"
    );
}

/// Each edit on a marked file gives the mark plus the plain file's result.
#[test]
fn edits_on_the_first_line_match_the_file_without_a_mark() {
    let plain = "class A {\n\tx = 1;\n};\nclass B { };\n";
    let marked = format!("\u{feff}{plain}");
    let ops = [
        EditOp::Remove {
            target: Target::Class(ConfigPath::of(&["A"])),
        },
        EditOp::AddClass {
            parent: ConfigPath::of(&[]),
            name: "N".into(),
            base: None,
            forward: false,
            position: Position::First,
        },
    ];

    for op in ops {
        let expected = plan_all(plain, std::slice::from_ref(&op)).unwrap();
        assert_eq!(
            plan_all(&marked, &[op]).unwrap(),
            format!("\u{feff}{expected}")
        );
    }
}

#[test]
fn a_comment_after_the_mark_is_highlighted() {
    let spans = highlight("\u{feff}// head\nclass A {};");

    assert!(
        spans
            .iter()
            .any(|span| span.start == 3 && span.end == 10 && span.kind == ConfigToken::Comment),
        "{spans:?}"
    );
}
