//! Schema-driven file lints shared by every structured Central Economy kind:
//! MF-X020, X021, X022, X030, X031, X040, X110, X111
//! and X112, plus [`validate_text`], the file-level entry point.

use crate::fixes::{did_you_mean, DID_YOU_MEAN};
use crate::kind::CeKind;
use crate::model::{self, Document, Record};
use crate::schema::{self, FieldType, Key, ListSchema, Place, RecordSchema, ValueProblem};
use crate::xml::{self, NodeId, XmlDoc};
use modfoundry_core::text::edit_distance_ignore_ascii_case;
use modfoundry_core::{Diagnostic, Severity, Span};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

/// Every finding for a CE file's text: the XML issues, plus —
/// when the parse is not blocking — the schema findings and the types findings.
/// Sorted by span start, then code.
pub fn validate_text(text: &str, file_name: Option<&str>) -> Vec<Diagnostic> {
    let doc = xml::parse(text);
    let mut findings = doc.issues.clone();
    if !doc.blocking() {
        let document = model::read_doc(text, &doc, file_name);
        findings.extend(schema_findings(&doc, &document));
        findings.extend(globals_findings(&document));
        findings.extend(types_findings(&doc, &document));
    }
    findings.sort_by(|a, b| {
        span_start(a)
            .cmp(&span_start(b))
            .then_with(|| a.code.cmp(&b.code))
    });
    findings
}

/// The types rules — MF-X110, MF-X111, MF-X112 — for a read document; empty for
/// any other kind. Sorted by span start.
pub fn types_findings(doc: &XmlDoc, document: &Document) -> Vec<Diagnostic> {
    if document.kind != Some(CeKind::Types) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    if let Some(section) = document.section("types") {
        for record in &section.records {
            check_types_record(doc, record, &mut findings);
        }
    }
    findings.sort_by_key(span_start);
    findings
}

pub(crate) fn span_start(diagnostic: &Diagnostic) -> usize {
    diagnostic.span.map_or(0, |span| span.start)
}

fn check_types_record(doc: &XmlDoc, record: &Record, findings: &mut Vec<Diagnostic>) {
    if let (Some((nominal, _)), Some((min, span))) =
        (int_value(record, "nominal"), int_value(record, "min"))
    {
        if nominal > 0 && min > nominal {
            findings.push(Diagnostic::at(
                "MF-X110",
                Severity::Warning,
                span,
                format!(
                    "`{}`: min ({min}) is above nominal ({nominal}). The economy never reaches min.",
                    record.key
                ),
            ));
        }
    }

    if let (Some((quantmin, quantmin_span)), Some((quantmax, quantmax_span))) =
        (int_value(record, "quantmin"), int_value(record, "quantmax"))
    {
        if (quantmin == -1) != (quantmax == -1) {
            let span = if quantmin == -1 {
                quantmin_span
            } else {
                quantmax_span
            };
            findings.push(Diagnostic::at(
                "MF-X111",
                Severity::Warning,
                span,
                format!(
                    "`{}`: set both quantmin and quantmax to -1, or neither.",
                    record.key
                ),
            ));
        } else if quantmin >= 0 && quantmax >= 0 && quantmin > quantmax {
            findings.push(Diagnostic::at(
                "MF-X111",
                Severity::Warning,
                quantmin_span,
                format!(
                    "`{}`: quantmin ({quantmin}) is above quantmax ({quantmax}).",
                    record.key
                ),
            ));
        }
    }

    let categories = doc.children_named(record.node, "category");
    if categories.len() > 1 {
        let span = doc
            .element(categories[1])
            .map_or(record.span, |element| element.name_span);
        findings.push(Diagnostic::at(
            "MF-X112",
            Severity::Hint,
            span,
            format!(
                "`{}` has {} categories. A type has one.",
                record.key,
                categories.len()
            ),
        ));
    }
}

fn int_value(record: &Record, id: &str) -> Option<(i64, Span)> {
    let field = record.field(id)?;
    if !field.valid {
        return None;
    }
    let value = field.raw.as_deref()?.parse::<i64>().ok()?;
    Some((value, field.span?))
}

/// Every schema lint finding for a read document, sorted by span start.
pub fn schema_findings(doc: &XmlDoc, document: &Document) -> Vec<Diagnostic> {
    let Some(kind) = document.kind else {
        return Vec::new();
    };
    let mut findings = Vec::new();

    if document.fragment {
        if let Some(element) = doc.top_elements().first().and_then(|&id| doc.element(id)) {
            findings.push(Diagnostic::at(
                "MF-X020",
                Severity::Warning,
                element.name_span,
                format!(
                    "This file has no `<{}>` root, so a server can't load it on its own. Wrap the entries in `<{}>`.",
                    kind.root(),
                    kind.root()
                ),
            ));
        }
    }

    if let Some(root_name) = &document.root_mismatch {
        if let Some(element) = doc.root().and_then(|id| doc.element(id)) {
            findings.push(Diagnostic::at(
                "MF-X020",
                Severity::Warning,
                element.name_span,
                format!(
                    "These are {}, but the root is `<{}>`. Rename it to `<{}>`.",
                    kind.label().to_lowercase(),
                    root_name,
                    kind.root()
                ),
            ));
        }
    }

    if let Some(kind_schema) = schema::schema(kind) {
        for section in &document.sections {
            let Some(section_schema) = kind_schema.section(&section.id) else {
                continue;
            };
            let record_schema = &section_schema.record;
            let known_children = record_schema.known_children();
            let record_attrs = record_attrs(record_schema);
            let mut seen: HashMap<String, usize> = HashMap::new();
            for record in &section.records {
                check_record(
                    doc,
                    record,
                    record_schema,
                    &known_children,
                    &record_attrs,
                    &mut findings,
                );
                check_duplicate_key(record, record_schema, &mut seen, &mut findings);
            }
        }
    }

    findings.sort_by_key(|finding| finding.span.map_or(0, |span| span.start));
    findings
}

/// MF-X030 for a globals `<var>` whose `value` doesn't fit its `type`. Sorted
/// by span start.
fn globals_findings(document: &Document) -> Vec<Diagnostic> {
    let Some(section) = document.section("variables") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        let Some(type_value) = record.field("type").and_then(|field| field.raw.clone()) else {
            continue;
        };
        let Some(value) = record.field("value") else {
            continue;
        };
        let (Some(raw), Some(span)) = (value.raw.as_deref(), value.span) else {
            continue;
        };
        let ty = match type_value.as_str() {
            "0" => FieldType::Int {
                min: None,
                max: None,
            },
            "1" => FieldType::Float {
                min: None,
                max: None,
            },
            _ => continue,
        };
        if ty.check(raw).is_ok() {
            continue;
        }
        let description = if type_value == "0" {
            "an integer variable, so its value must be a whole number"
        } else {
            "a float variable, so its value must be a number"
        };
        findings.push(Diagnostic::at(
            "MF-X030",
            Severity::Warning,
            span,
            format!("`{}` is {description}, not `{raw}`.", record.key),
        ));
    }
    findings
}

/// The known name within edit distance 2 of `name`, case-insensitively.
pub fn nearest<'a>(name: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(usize, &'a str)> = None;
    for &candidate in known {
        let distance = edit_distance_ignore_ascii_case(name, candidate);
        if distance <= 2 && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn check_record(
    doc: &XmlDoc,
    record: &Record,
    schema: &RecordSchema,
    known_children: &[&str],
    record_attrs: &[&str],
    findings: &mut Vec<Diagnostic>,
) {
    if let Key::Attr(key) = schema.key {
        if doc.attr(record.node, key).is_none() {
            if let Some(element) = doc.element(record.node) {
                findings.push(Diagnostic::at(
                    "MF-X030",
                    Severity::Warning,
                    element.name_span,
                    format!("`<{}>` needs a `{key}`.", element.name),
                ));
            }
        }
    }

    if let Some(element) = doc.element(record.node) {
        for attr in &element.attrs {
            if !record_attrs.contains(&attr.name.as_str()) {
                findings.push(attribute_finding(
                    &element.name,
                    &attr.name,
                    attr.name_span,
                    record_attrs,
                ));
            }
        }
    }

    for child in doc.child_elements_iter(record.node) {
        let Some(element) = doc.element(child) else {
            continue;
        };
        let name = element.name.as_str();
        if !known_children.contains(&name) {
            let suggestion = nearest(name, known_children);
            let hint = match suggestion {
                Some(suggestion) => format!(" {DID_YOU_MEAN} `<{suggestion}>`?"),
                None => String::new(),
            };
            findings.push(Diagnostic::at(
                "MF-X021",
                Severity::Warning,
                element.name_span,
                format!(
                    "`<{name}>` isn't part of a `<{}>` here.{hint}",
                    record.element
                ),
            ));
            continue;
        }
        let child_attrs = child_attrs(schema, name);
        if child_attrs.is_empty() {
            continue;
        }
        for attr in &element.attrs {
            if !child_attrs.contains(&attr.name.as_str()) {
                findings.push(attribute_finding(
                    name,
                    &attr.name,
                    attr.name_span,
                    &child_attrs,
                ));
            }
        }
    }

    for field in &record.fields {
        let (Some(raw), Some(span)) = (field.raw.as_deref(), field.span) else {
            continue;
        };
        let Some(field_schema) = schema.field(&field.id) else {
            continue;
        };
        if let Err(problem) = field_schema.ty.check(raw) {
            findings.push(value_finding(&field.id, problem, raw, span));
        }
    }

    for list in schema.lists {
        check_list(doc, record.node, list, findings);
    }
}

fn check_list(doc: &XmlDoc, parent: NodeId, list: &ListSchema, findings: &mut Vec<Diagnostic>) {
    let holder = match list.container {
        Some(container) => match doc.first_child_named(parent, container) {
            Some(id) => id,
            None => return,
        },
        None => parent,
    };
    let allowed: Vec<&str> = list.attrs.iter().map(|attr| attr.id).collect();
    for item in doc.children_named(holder, list.element) {
        let Some(element) = doc.element(item) else {
            continue;
        };
        for attr in &element.attrs {
            if !allowed.contains(&attr.name.as_str()) {
                findings.push(attribute_finding(
                    &element.name,
                    &attr.name,
                    attr.name_span,
                    &allowed,
                ));
                continue;
            }
            let Some(attr_schema) = list.attrs.iter().find(|schema| schema.id == attr.name) else {
                continue;
            };
            if let Err(problem) = attr_schema.ty.check(&attr.value) {
                findings.push(value_finding(
                    attr_schema.id,
                    problem,
                    &attr.value,
                    attr.value_span,
                ));
            }
        }
        if let Some(nested) = list.nested {
            check_list(doc, item, nested, findings);
        }
        for inner in list.inner {
            check_list(doc, item, inner, findings);
        }
    }
}

fn check_duplicate_key(
    record: &Record,
    schema: &RecordSchema,
    seen: &mut HashMap<String, usize>,
    findings: &mut Vec<Diagnostic>,
) {
    let mut identity = if schema.elements.len() > 1 {
        format!("{}\u{0}{}", record.element, record.key)
    } else {
        record.key.clone()
    };
    if matches!(schema.key, Key::Attr(_)) {
        identity = identity.to_ascii_lowercase();
    }
    match seen.entry(identity) {
        Entry::Occupied(first) => findings.push(Diagnostic::at(
            "MF-X040",
            Severity::Warning,
            record.key_span.unwrap_or(record.span),
            format!(
                "`{}` is defined twice in this file (also on line {}). Keep one.",
                record.key,
                first.get()
            ),
        )),
        Entry::Vacant(slot) => {
            slot.insert(record.line);
        }
    }
}

fn record_attrs(schema: &RecordSchema) -> Vec<&'static str> {
    let mut allowed = Vec::new();
    if let Key::Attr(key) = schema.key {
        allowed.push(key);
    }
    for field in schema.fields {
        if let Place::Attr(name) = field.place {
            allowed.push(name);
        }
    }
    allowed
}

fn child_attrs(schema: &RecordSchema, child: &str) -> Vec<&'static str> {
    schema
        .fields
        .iter()
        .filter_map(|field| match field.place {
            Place::ChildAttr { child: c, attr } if c == child => Some(attr),
            _ => None,
        })
        .collect()
}

fn attribute_finding(element: &str, name: &str, span: Span, allowed: &[&str]) -> Diagnostic {
    let hint = did_you_mean(nearest(name, allowed));
    Diagnostic::at(
        "MF-X022",
        Severity::Hint,
        span,
        format!("`<{element}>` has no `{name}` attribute.{hint}"),
    )
}

fn value_finding(id: &str, problem: ValueProblem, raw: &str, span: Span) -> Diagnostic {
    let (code, text) = match problem {
        ValueProblem::Invalid(text) => ("MF-X030", text),
        ValueProblem::OutOfRange(text) => ("MF-X031", text),
    };
    Diagnostic::at(
        code,
        Severity::Warning,
        span,
        format!("`{id}` must be {text}, not `{raw}`."),
    )
}
