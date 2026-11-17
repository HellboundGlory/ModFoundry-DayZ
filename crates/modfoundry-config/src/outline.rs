//! A serializable, resolution-free view of one config.cpp: the class tree as
//! written, for editors and agents that must render a file without resolving
//! inheritance or includes.

use crate::ast::{ConfigClass, ConfigProperty, Member, Value};
use crate::parser::parse;
use modfoundry_core::text::LineIndex;
use modfoundry_core::{Diagnostic, Span};
use serde::Serialize;

/// One class node: its place in the file and its nested classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutlineNode {
    pub name: String,
    /// Class names from the root down to and including this one.
    pub path: Vec<String>,
    /// The first declared parent.
    pub base: Option<String>,
    pub forward: bool,
    pub span: Span,
    pub name_span: Span,
    pub line: usize,
    pub children: Vec<OutlineNode>,
}

/// One property statement of a class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PropertyRow {
    pub name: String,
    pub array: bool,
    pub append: bool,
    pub kind: ValueKind,
    pub display: String,
    /// Top-level array items exactly as written.
    pub items: Vec<String>,
    pub span: Span,
    pub value_span: Option<Span>,
    pub line: usize,
    pub editable: bool,
}

/// The value shape a [`PropertyRow`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Str,
    Number,
    Ident,
    Array,
    Other,
    None,
}

/// One class's own contents, addressed by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassView {
    pub path: Vec<String>,
    pub base: Option<String>,
    pub forward: bool,
    pub properties: Vec<PropertyRow>,
    pub deletes: Vec<String>,
    pub child_classes: Vec<String>,
}

/// The whole file: root classes, whether an `#include` appears, and the parse
/// issue count that gates editability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outline {
    pub roots: Vec<OutlineNode>,
    pub has_include: bool,
    pub issues: usize,
}

/// The outline plus the parse diagnostics it was recovered from.
pub fn parse_outline(text: &str) -> (Outline, Vec<Diagnostic>) {
    let parsed = parse(text);
    let issues = parsed.issues.len();
    let lines = LineIndex::new(text);
    let roots = nodes(&parsed.file.members, &[], &lines);
    let outline = Outline {
        roots,
        has_include: parsed.file.has_include,
        issues,
    };
    (outline, parsed.diagnostics())
}

/// The contents of the class at `path` (case-insensitive), or `None`.
pub fn class_view(text: &str, path: &[String]) -> Option<ClassView> {
    let parsed = parse(text);
    let class = find_class(&parsed.file.members, path)?;
    let lines = LineIndex::new(text);
    Some(view(class, path, text, &lines, parsed.issues.len()))
}

/// The path of the innermost class whose statement span holds `offset`.
pub fn class_at(text: &str, offset: usize) -> Option<Vec<String>> {
    let parsed = parse(text);
    innermost(&parsed.file.members, offset, &[])
}

fn nodes(members: &[Member], parent: &[String], lines: &LineIndex) -> Vec<OutlineNode> {
    members
        .iter()
        .filter_map(|m| match m {
            Member::Class(c) => Some(node(c, parent, lines)),
            _ => None,
        })
        .collect()
}

fn node(class: &ConfigClass, parent: &[String], lines: &LineIndex) -> OutlineNode {
    let mut path = parent.to_vec();
    path.push(class.name.clone());
    OutlineNode {
        name: class.name.clone(),
        base: class.parents.first().cloned(),
        forward: class.forward,
        span: class.spans.statement,
        name_span: class.spans.name,
        line: lines.line_of(class.spans.statement.start),
        children: nodes(&class.members, &path, lines),
        path,
    }
}

fn view(
    class: &ConfigClass,
    path: &[String],
    text: &str,
    lines: &LineIndex,
    issues: usize,
) -> ClassView {
    ClassView {
        path: path.to_vec(),
        base: class.parents.first().cloned(),
        forward: class.forward,
        properties: class
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Property(p) => Some(row(p, text, lines, issues)),
                _ => None,
            })
            .collect(),
        deletes: class
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Delete { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect(),
        child_classes: class
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Class(c) => Some(c.name.clone()),
                _ => None,
            })
            .collect(),
    }
}

fn row(property: &ConfigProperty, text: &str, lines: &LineIndex, issues: usize) -> PropertyRow {
    let kind = kind_of(property.value.as_ref());
    PropertyRow {
        name: property.name.clone(),
        array: property.declared_array,
        append: property.array_append,
        kind,
        display: display(property, text),
        items: property
            .spans
            .value_items
            .iter()
            .map(|s| slice(text, *s).to_string())
            .collect(),
        span: property.spans.statement,
        value_span: property.spans.value,
        line: lines.line_of(property.spans.statement.start),
        editable: issues == 0 && kind != ValueKind::Other && !nested_array(property),
    }
}

fn kind_of(value: Option<&Value>) -> ValueKind {
    match value {
        None => ValueKind::None,
        Some(Value::Str(_)) => ValueKind::Str,
        Some(Value::Number(_) | Value::Float(_)) => ValueKind::Number,
        Some(Value::Ident(_)) => ValueKind::Ident,
        Some(Value::Array(_)) => ValueKind::Array,
        Some(Value::Other(_)) => ValueKind::Other,
    }
}

fn display(property: &ConfigProperty, text: &str) -> String {
    let Some(span) = property.spans.value else {
        return String::new();
    };
    match &property.value {
        Some(Value::Str(s)) => s.clone(),
        Some(Value::Array(_)) => "{…}".to_string(),
        _ => slice(text, span).to_string(),
    }
}

fn nested_array(property: &ConfigProperty) -> bool {
    matches!(&property.value, Some(Value::Array(items)) if items.iter().any(|v| matches!(v, Value::Array(_))))
}

fn slice(text: &str, span: Span) -> &str {
    text.get(span.start..span.end).unwrap_or("").trim()
}

fn find_class<'a>(members: &'a [Member], path: &[String]) -> Option<&'a ConfigClass> {
    let (head, rest) = path.split_first()?;
    let class = members.iter().find_map(|m| match m {
        Member::Class(c) if c.name.eq_ignore_ascii_case(head) => Some(c),
        _ => None,
    })?;
    if rest.is_empty() {
        Some(class)
    } else {
        find_class(&class.members, rest)
    }
}

fn innermost(members: &[Member], offset: usize, parent: &[String]) -> Option<Vec<String>> {
    members.iter().find_map(|m| match m {
        Member::Class(c) if holds(c.spans.statement, offset) => {
            let mut path = parent.to_vec();
            path.push(c.name.clone());
            Some(innermost(&c.members, offset, &path).unwrap_or(path))
        }
        _ => None,
    })
}

fn holds(span: Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED: &str = r#"#include "extra.hpp"
class Root : Base
{
    name = "hi";
    count = 3;
    flag = true;
    items[] = { "a", "b" };
    nested[] = { 1, { 2 } };
    more[] += { "c" };
    macro = SOME_MACRO;
    class Child
    {
        x = 1;
    };
    class Fwd;
    delete Old;
};
"#;

    fn row<'a>(view: &'a ClassView, name: &str) -> &'a PropertyRow {
        view.properties
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no row {name} in {:?}", view.properties))
    }

    #[test]
    fn outline_keeps_order_paths_bases_forwards_and_lines() {
        let out = parse_outline(NESTED).0;
        assert_eq!(out.issues, 0);
        assert!(out.has_include);

        assert_eq!(out.roots.len(), 1);
        let root = &out.roots[0];
        assert_eq!(root.name, "Root");
        assert_eq!(root.path, ["Root"]);
        assert_eq!(root.base.as_deref(), Some("Base"));
        assert!(!root.forward);
        assert_eq!(root.line, 2);
        assert_eq!(
            &NESTED[root.span.start..root.span.end],
            NESTED[NESTED.find("class Root").unwrap()..].trim_end()
        );
        assert_eq!(&NESTED[root.name_span.start..root.name_span.end], "Root");

        assert_eq!(root.children.len(), 2);
        let child = &root.children[0];
        assert_eq!(child.name, "Child");
        assert_eq!(child.path, ["Root", "Child"]);
        assert_eq!(child.base, None);
        assert_eq!(child.line, 11);

        let fwd = &root.children[1];
        assert_eq!(fwd.name, "Fwd");
        assert!(fwd.forward);
        assert_eq!(fwd.path, ["Root", "Fwd"]);
        assert_eq!(fwd.line, 15);
    }

    #[test]
    fn a_row_carries_its_kind_display_items_and_flags() {
        let view = class_view(NESTED, &["Root".into()]).unwrap();
        assert_eq!(view.base.as_deref(), Some("Base"));
        assert_eq!(view.child_classes, ["Child", "Fwd"]);
        assert_eq!(view.deletes, ["Old"]);

        let name = row(&view, "name");
        assert_eq!(name.kind, ValueKind::Str);
        assert_eq!(name.display, "hi");
        assert_eq!(name.line, 4);
        assert!(name.editable);

        assert_eq!(row(&view, "count").kind, ValueKind::Number);
        assert_eq!(row(&view, "count").display, "3");
        assert_eq!(row(&view, "flag").kind, ValueKind::Ident);
        assert_eq!(row(&view, "flag").display, "true");
        assert_eq!(row(&view, "macro").kind, ValueKind::Ident);
        assert_eq!(row(&view, "macro").display, "SOME_MACRO");

        let items = row(&view, "items");
        assert_eq!(items.kind, ValueKind::Array);
        assert_eq!(items.display, "{…}");
        assert_eq!(items.items, ["\"a\"", "\"b\""]);
        assert!(items.array);
        assert!(items.editable);

        let more = row(&view, "more");
        assert!(more.append);
        assert_eq!(more.items, ["\"c\""]);

        let nested = row(&view, "nested");
        assert_eq!(nested.items, ["1", "{ 2 }"]);
        assert!(!nested.editable, "a nested array is not a flat edit");
    }

    #[test]
    fn class_view_lookup_is_case_insensitive() {
        let view = class_view(NESTED, &["root".into(), "CHILD".into()]).unwrap();
        assert_eq!(row(&view, "x").display, "1");
        assert!(class_view(NESTED, &["root".into(), "Gone".into()]).is_none());
        assert!(class_view(NESTED, &[]).is_none());
    }

    #[test]
    fn a_recovered_tree_keeps_its_issues_and_disables_editing() {
        let text = "class Broken\n{\n    a = 1 + 2;\n    b = 3;\n};\n";
        let out = parse_outline(text).0;
        assert_eq!(out.issues, 1, "the expression tail is one parse issue");
        assert!(!out.has_include);
        assert_eq!(out.roots.len(), 1);
        assert_eq!(out.roots[0].name, "Broken");

        let view = class_view(text, &["Broken".into()]).unwrap();
        assert_eq!(row(&view, "a").kind, ValueKind::Other);
        assert_eq!(row(&view, "a").display, "1 + 2");
        assert!(!row(&view, "a").editable);
        assert!(!row(&view, "b").editable, "issues disable the whole file");
    }

    #[test]
    fn class_at_finds_the_innermost_class_at_each_boundary() {
        let inside_root = NESTED.find("name = ").unwrap();
        assert_eq!(class_at(NESTED, inside_root).unwrap(), ["Root"]);

        let keyword = NESTED.find("class Root").unwrap();
        assert_eq!(class_at(NESTED, keyword).unwrap(), ["Root"]);

        let child = NESTED.find("x = 1").unwrap();
        assert_eq!(class_at(NESTED, child).unwrap(), ["Root", "Child"]);

        let fwd_kw = NESTED.find("class Fwd").unwrap();
        assert_eq!(class_at(NESTED, fwd_kw).unwrap(), ["Root", "Fwd"]);

        let trailing = NESTED.rfind("};").unwrap();
        assert_eq!(class_at(NESTED, trailing).unwrap(), ["Root"]);

        assert_eq!(class_at(NESTED, 0), None, "the #include is no class");
        assert_eq!(class_at(NESTED, NESTED.len()), None);
    }
}
