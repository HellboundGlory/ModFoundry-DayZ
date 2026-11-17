//! Hand-rolled round trips: an edit and its inverse give back the original bytes.

use crate::edit_fixtures as fixtures;

use fixtures::*;
use modfoundry_config::ast::{ConfigClass, ConfigProperty, Member, Value};
use modfoundry_config::edit::*;
use modfoundry_config::parser::parse;

fn texts() -> Vec<(&'static str, String)> {
    vec![
        ("canon", CANON.to_owned()),
        ("canon-tabs-crlf", restyle(CANON, "\t", true)),
        ("canon-two", restyle(CANON, "  ", false)),
        ("knr", KNR.to_owned()),
        ("one-line", ONE_LINE.to_owned()),
        ("vanilla", crlf(VANILLA_LF)),
        ("comments", COMMENTS.to_owned()),
        ("arrays", ARRAYS.to_owned()),
        ("scaffold", scaffold_config()),
    ]
}

struct Found<'a> {
    path: Vec<String>,
    class: &'a ConfigClass,
}

fn classes<'a>(members: &'a [Member], parent: &mut Vec<String>, out: &mut Vec<Found<'a>>) {
    for m in members {
        if let Member::Class(c) = m {
            parent.push(c.name.clone());
            out.push(Found {
                path: parent.clone(),
                class: c,
            });
            classes(&c.members, parent, out);
            parent.pop();
        }
    }
}

fn all_classes(text: &str) -> Vec<(Vec<String>, ConfigClass)> {
    let file = parse(text).file;
    let mut found = Vec::new();
    classes(&file.members, &mut Vec::new(), &mut found);
    found
        .into_iter()
        .map(|f| (f.path, f.class.clone()))
        .collect()
}

fn properties(class: &ConfigClass) -> Vec<&ConfigProperty> {
    class
        .members
        .iter()
        .filter_map(|m| match m {
            Member::Property(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn one_line_canonical(text: &str, p: &ConfigProperty, original: &NewValue) -> bool {
    let Some(span) = p.spans.value else {
        return false;
    };
    let slice = &text[span.start..span.end];
    let mut style = infer_style(text, None);
    style.multiline_arrays = false;
    [true, false].into_iter().any(|padded| {
        style.padded_braces = padded;
        render_value(original, &style).is_ok_and(|r| r == slice)
    })
}

#[test]
fn set_value_and_back_restores_every_property() {
    let mut checked = 0;
    for (label, text) in texts() {
        for (class_path, cls) in all_classes(&text) {
            let names: Vec<&str> = class_path.iter().map(String::as_str).collect();
            for p in properties(&cls) {
                let Some(original) = p.value.as_ref().and_then(to_new) else {
                    continue;
                };
                let probe = match &original {
                    NewValue::Array(_) => {
                        if !one_line_canonical(&text, p, &original) {
                            continue;
                        }
                        arr(vec![n("0")])
                    }
                    _ => s("__probe"),
                };
                let there = edit(&text, &set(&names, &p.name, probe));
                assert_ne!(there, text, "{label}/{}", p.name);
                let back = edit(&there, &set(&names, &p.name, original));
                assert_eq!(back, text, "{label}/{names:?}/{}", p.name);
                checked += 1;
            }
        }
    }
    assert!(checked > 30, "only {checked} properties round-tripped");
}

#[test]
fn rename_and_back_restores_every_member() {
    let mut checked = 0;
    for (label, text) in texts() {
        for (class_path, cls) in all_classes(&text) {
            let names: Vec<&str> = class_path.iter().map(String::as_str).collect();
            let there = edit(&text, &rename(class(&names), "__mf_probe"));
            let mut renamed = class_path.clone();
            *renamed.last_mut().unwrap() = "__mf_probe".into();
            let back_names: Vec<&str> = renamed.iter().map(String::as_str).collect();
            let back = edit(&there, &rename(class(&back_names), &cls.name));
            assert_eq!(back, text, "{label}/{names:?}");
            checked += 1;
            for p in properties(&cls) {
                let there = edit(&text, &rename(prop(&names, &p.name), "__mf_probe"));
                let back = edit(&there, &rename(prop(&names, "__mf_probe"), &p.name));
                assert_eq!(back, text, "{label}/{names:?}/{}", p.name);
                checked += 1;
            }
        }
    }
    assert!(checked > 80, "only {checked} members round-tripped");
}

#[test]
fn add_and_remove_restores_multi_line_bodies() {
    let mut checked = 0;
    for (label, text) in texts() {
        if label == "one-line" {
            continue;
        }
        for (class_path, cls) in all_classes(&text) {
            if cls.forward {
                continue;
            }
            let names: Vec<&str> = class_path.iter().map(String::as_str).collect();
            for position in [Position::First, Position::Last] {
                let with_prop = edit(&text, &add(&names, "__mf_probe", n("1"), position.clone()));
                let back = edit(&with_prop, &remove(prop(&names, "__mf_probe")));
                assert_eq!(back, text, "{label}/{names:?}/property/{position:?}");

                let with_class = edit(
                    &text,
                    &add_class(&names, "__mf_probe", Some("Base"), false, position.clone()),
                );
                let mut inner = class_path.clone();
                inner.push("__mf_probe".into());
                let inner_names: Vec<&str> = inner.iter().map(String::as_str).collect();
                let back = edit(&with_class, &remove(class(&inner_names)));
                assert_eq!(back, text, "{label}/{names:?}/class/{position:?}");
                checked += 1;
            }
        }
        let with_root = edit(
            &text,
            &add_class(&[], "__mf_probe", None, false, Position::Last),
        );
        let back = edit(&with_root, &remove(class(&["__mf_probe"])));
        assert_eq!(back, text, "{label}/root");
    }
    assert!(checked > 30, "only {checked} bodies round-tripped");
}

#[test]
fn array_insert_and_remove_restore_arrays() {
    let mut checked = 0;
    for (label, text) in texts() {
        for (class_path, cls) in all_classes(&text) {
            let names: Vec<&str> = class_path.iter().map(String::as_str).collect();
            for p in properties(&cls) {
                let Some(Value::Array(items)) = &p.value else {
                    continue;
                };
                if items.is_empty() {
                    continue;
                }
                for index in 0..=items.len() {
                    let grown = edit(&text, &insert(&names, &p.name, index, n("777")));
                    let back = edit(&grown, &remove_item(&names, &p.name, index));
                    assert_eq!(back, text, "{label}/{names:?}/{}/{index}", p.name);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 20, "only {checked} inserts round-tripped");
}

#[test]
fn moving_an_item_there_and_back_restores_the_array() {
    let mut checked = 0;
    for (label, text) in texts() {
        for (class_path, cls) in all_classes(&text) {
            let names: Vec<&str> = class_path.iter().map(String::as_str).collect();
            for p in properties(&cls) {
                let Some(Value::Array(items)) = &p.value else {
                    continue;
                };
                for from in 0..items.len() {
                    for to in 0..items.len() {
                        let moved = edit(&text, &move_item(&names, &p.name, from, to));
                        let back = edit(&moved, &move_item(&names, &p.name, to, from));
                        assert_eq!(back, text, "{label}/{}/{from}->{to}", p.name);
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 10, "only {checked} moves round-tripped");
}
