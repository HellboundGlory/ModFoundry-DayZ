//! Round-trip gate for the AST spans: every recorded span must slice the exact
//! source text it claims, on inline fixtures and on the real config corpus.

use crate::common;

use common::corpus::{vanilla_configs, workshop_configs};
use modfoundry_config::ast::{ConfigClass, ConfigFile, ConfigProperty, Member, Value};
use modfoundry_config::parser::parse;
use modfoundry_config::rap::to_text;
use modfoundry_config::Span;
use std::collections::BTreeMap;

fn slice(text: &str, span: Span) -> &str {
    &text[span.start..span.end]
}

fn check_name(label: &str, name: &str, span: Span, text: &str) {
    let found = slice(text, span);
    assert!(
        found == name || found == format!("\"{name}\""),
        "{label}: name slice {found:?} != {name:?}"
    );
}

fn relex(label: &str, source: &str) -> Value {
    let wrapped = format!("class __P {{ __v = {source}; }};");
    let file = parse(&wrapped).file;
    file.find_class("__P")
        .and_then(|c| c.property("__v"))
        .and_then(|p| p.value.clone())
        .unwrap_or_else(|| panic!("{label}: value slice {source:?} did not re-parse"))
}

fn check_property(label: &str, p: &ConfigProperty, text: &str) {
    check_name(label, &p.name, p.spans.name, text);
    if let Some(brackets) = p.spans.brackets {
        assert_eq!(
            slice(text, brackets),
            "[]",
            "{label}: brackets for {}",
            p.name
        );
    }
    match &p.value {
        Some(value) => {
            let operator = slice(text, p.spans.operator);
            assert!(
                operator == "=" || operator == "+=",
                "{label}: operator {operator:?} for {}",
                p.name
            );
            let span = p.spans.value.expect("value span");
            let source = slice(text, span);
            assert_eq!(
                &relex(label, source),
                value,
                "{label}: {} value slice {source:?}",
                p.name
            );
            match p.spans.array_braces {
                Some((open, close)) => {
                    assert_eq!(slice(text, open), "{", "{label}: {} array open", p.name);
                    assert_eq!(slice(text, close), "}", "{label}: {} array close", p.name);
                    assert!(
                        open.end <= close.start,
                        "{label}: {} braces reversed",
                        p.name
                    );
                    let mut prev = open.end;
                    for item in &p.spans.value_items {
                        assert!(item.start >= prev, "{label}: {} items out of order", p.name);
                        assert!(
                            item.start >= open.end
                                && item.end <= close.start
                                && item.end > item.start,
                            "{label}: {} item {:?} outside braces",
                            p.name,
                            slice(text, *item)
                        );
                        prev = item.end;
                    }
                }
                None => assert!(
                    p.spans.value_items.is_empty(),
                    "{label}: {} stray items",
                    p.name
                ),
            }
        }
        None => assert!(
            p.spans.value.is_none(),
            "{label}: {} span without value",
            p.name
        ),
    }
    let statement = slice(text, p.spans.statement);
    assert!(
        statement.starts_with(slice(text, p.spans.name)),
        "{label}: statement {statement:?} does not start with the name"
    );
    assert!(statement.ends_with(';'), "{label}: statement {statement:?}");
}

fn check_class(label: &str, c: &ConfigClass, text: &str, members: &mut usize) {
    assert_eq!(
        slice(text, c.spans.keyword),
        "class",
        "{label}: {} keyword",
        c.name
    );
    check_name(label, &c.name, c.spans.name, text);
    if let Some(parents) = c.spans.parents {
        let source = slice(text, parents);
        assert!(
            source.starts_with(':'),
            "{label}: {} parents {source:?}",
            c.name
        );
        for parent in &c.parents {
            assert!(
                source.contains(parent.as_str()),
                "{label}: {} parents {source:?} missing {parent}",
                c.name
            );
        }
    }
    match (c.spans.open_brace, c.spans.close_brace) {
        (Some(open), Some(close)) => {
            assert_eq!(slice(text, open), "{", "{label}: {} open brace", c.name);
            assert_eq!(slice(text, close), "}", "{label}: {} close brace", c.name);
        }
        (None, None) => {}
        (open, close) => panic!("{label}: {} half-braced ({open:?}, {close:?})", c.name),
    }
    let statement = slice(text, c.spans.statement);
    assert!(
        statement.starts_with("class") || statement.starts_with("external"),
        "{label}: class statement {statement:?}"
    );
    let trimmed = statement.trim_end();
    assert!(
        trimmed.ends_with(';') || trimmed.ends_with('}'),
        "{label}: class statement {statement:?}"
    );
    check_members(label, &c.members, text, members);
}

fn check_members(label: &str, members: &[Member], text: &str, count: &mut usize) {
    let mut prev_end = 0;
    for member in members {
        *count += 1;
        let statement = match member {
            Member::Class(c) => {
                check_class(label, c, text, count);
                c.spans.statement
            }
            Member::Property(p) => {
                check_property(label, p, text);
                p.spans.statement
            }
            Member::Delete {
                name,
                span,
                statement,
            } => {
                let head = slice(text, *span);
                assert!(
                    head.starts_with("delete")
                        && (head.ends_with(name.as_str())
                            || head.ends_with(&format!("\"{name}\""))),
                    "{label}: delete slice {head:?}"
                );
                let source = slice(text, *statement);
                assert!(
                    source.starts_with("delete") && source.ends_with(';'),
                    "{label}: delete statement {source:?}"
                );
                *statement
            }
        };
        assert!(
            statement.start >= prev_end,
            "{label}: sibling statements overlap at {statement:?}"
        );
        prev_end = statement.end;
    }
}

fn check_file(label: &str, file: &ConfigFile, text: &str) -> usize {
    let mut members = 0;
    check_members(label, &file.members, text, &mut members);
    members
}

fn check_clean(label: &str, text: &str) -> usize {
    let parsed = parse(text);
    assert!(
        parsed.issues.is_empty(),
        "{label}: issues {:?}",
        parsed.issues
    );
    check_file(label, &parsed.file, text)
}

#[test]
fn inline_fixtures_slice_exactly() {
    let fixtures = [
        ("allman", "class A : B\n{\n\tx = 1;\n};"),
        ("kr", "class A: B {\n\tx = 1;\n}"),
        ("one-line", "class A { x = 1; };"),
        ("two-siblings", "class A { x = 1; }; class B: A { y = 2; };"),
        ("array-append", "class A { x[] += { 1, 2 }; z[] = {}; };"),
        ("forward", "class Base;\nexternal class Use: Base;"),
        ("delete", "class A : Base\n{\n\tdelete X;\n};"),
        ("external-multi", "external class Use: A, B;"),
        ("quoted-name", "class A { \"q\" = 1; };"),
        ("negative", "class A { a = -0.5; b[] = { -1, 2 }; };"),
        ("nested-array", "class A { a[] = { { 1, 2 }, 3 }; };"),
        ("crlf", "class A\r\n{\r\n\tx = 1;\r\n};\r\n"),
        ("tabs", "class A\n{\n\t\tx\t=\t1;\n};"),
        (
            "comments-between-tokens",
            "/*a*/ class /*b*/ A /*c*/ : /*d*/ B /*e*/ { /*f*/ x /*g*/ = /*h*/ 1 /*i*/; /*j*/ } /*k*/;",
        ),
        (
            "preproc-inside",
            "#define X\nclass A\n{\n#ifdef X\n\tx = 1;\n#endif\n#include \"y.hpp\"\n};",
        ),
    ];
    for (label, text) in fixtures {
        check_clean(label, text);
    }
}

#[test]
fn expression_tail_fixture() {
    let text = "class A { a = x + y; };";
    let parsed = parse(text);
    assert_eq!(parsed.issues.len(), 1, "the tail is still reported");
    check_file("tail", &parsed.file, text);
}

#[test]
fn workshop_config_cpp_spans_round_trip() {
    let configs = workshop_configs();
    let mut cpps = 0;
    let mut clean = 0;
    let mut members = 0;
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    for c in &configs {
        if c.is_bin {
            continue;
        }
        cpps += 1;
        if c.issues != 0 {
            let text = c.text.as_deref().expect("cpp text");
            for issue in &parse(text).issues {
                *failures.entry(issue.message.clone()).or_default() += 1;
            }
            continue;
        }
        let text = c.text.as_deref().expect("cpp text");
        let file = c.file.as_ref().expect("cpp tree");
        members += check_file(&c.label, file, text);
        clean += 1;
    }
    eprintln!("workshop: {clean}/{cpps} clean config.cpp, {members} members checked");
    let mut ranked: Vec<_> = failures.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (message, count) in ranked {
        eprintln!("  {count:>5}  {message}");
    }
}

#[test]
fn vanilla_config_bin_to_text_spans_round_trip() {
    let configs = vanilla_configs();
    let bins: Vec<_> = configs.iter().filter(|c| c.is_bin).collect();
    let mut files = 0;
    let mut members = 0;
    for c in &bins {
        if let Some(error) = &c.error {
            panic!("{}: unreadable config.bin: {error}", c.label);
        }
        let tree = c.file.as_ref().expect("bin tree");
        let text = to_text(tree);
        let reparsed = parse(&text);
        assert!(
            reparsed.issues.is_empty(),
            "{}: to_text output does not re-parse: {:?}",
            c.label,
            reparsed.issues
        );
        members += check_file(&c.label, &reparsed.file, &text);
        files += 1;
    }
    eprintln!(
        "vanilla: {files}/{} config.bin re-parsed with 0 issues, {members} members checked",
        bins.len()
    );
    assert_eq!(files, bins.len(), "every vanilla config.bin must re-parse");
}
