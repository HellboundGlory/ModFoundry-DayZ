//! Quick fixes: one diagnostic in, a titled [`CeOp`] list out.

use modfoundry_core::{Diagnostic, Span};

use crate::edit::{self, XmlOp};
use crate::kind::CeKind;
use crate::model::{self, Document, Record};
use crate::ops::{self, CeOp, RecordRef};
use crate::schema::{self, FieldType, Key};
use crate::xml::{self, NodeId, XmlDoc};

/// One titled fix: the ops that clear a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeFix {
    pub title: String,
    pub ops: Vec<CeOp>,
}

const CODES: [&str; 13] = [
    "MF-X020", "MF-X021", "MF-X030", "MF-X040", "MF-X110", "MF-X111", "MF-X112", "MF-X121",
    "MF-X123", "MF-X130", "MF-X132", "MF-X410", "MF-X510",
];

/// The fixes for `diagnostic` in `text`; empty when none applies.
pub fn fixes_for(text: &str, file_name: Option<&str>, diagnostic: &Diagnostic) -> Vec<CeFix> {
    if diagnostic.span.is_none() || !fixable_codes().contains(&diagnostic.code.as_str()) {
        return Vec::new();
    }
    let doc = xml::parse(text);
    if doc.blocking() {
        return Vec::new();
    }
    let document = model::read_doc(text, &doc, file_name);
    fixes_in(&doc, &document, diagnostic)
}

/// The fixes for `diagnostic` in an already parsed document; empty when none applies.
pub fn fixes_in(doc: &XmlDoc, document: &Document, diagnostic: &Diagnostic) -> Vec<CeFix> {
    let Some(span) = diagnostic.span else {
        return Vec::new();
    };
    if !fixable_codes().contains(&diagnostic.code.as_str()) || doc.blocking() {
        return Vec::new();
    }
    let fixes = match diagnostic.code.as_str() {
        "MF-X020" => root_fix(doc, document, span),
        "MF-X021" => rename_element(doc, span, &diagnostic.message),
        "MF-X030" => write_flag(document, span),
        "MF-X040" => remove_duplicate(document, span),
        "MF-X110" => min_above_nominal(document, span),
        "MF-X111" => quant_span(document, span),
        "MF-X112" => keep_first_category(doc, document, span),
        "MF-X121" | "MF-X123" => {
            rename_class(document, span, &diagnostic.message, &diagnostic.code)
        }
        "MF-X130" | "MF-X132" | "MF-X410" => use_name(document, span, &diagnostic.message),
        "MF-X510" => write_include_type(document, span, &diagnostic.message),
        _ => None,
    };
    fixes.unwrap_or_default()
}

/// The codes [`fixes_for`] can ever answer.
pub fn fixable_codes() -> &'static [&'static str] {
    &CODES
}

fn root_fix(doc: &XmlDoc, document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let kind = document.kind?;
    if document.fragment {
        let &first = doc.top_elements().first()?;
        if doc.element(first).is_none_or(|el| el.name_span != span) {
            return None;
        }
        return Some(vec![CeFix {
            title: format!("Wrap in `<{}>`", kind.root()),
            ops: vec![CeOp::WrapRoot],
        }]);
    }
    document.root_mismatch.as_ref()?;
    let root = doc.root()?;
    if doc.element(root).is_none_or(|el| el.name_span != span) {
        return None;
    }
    Some(vec![CeFix {
        title: format!("Rename the root to `<{}>`", kind.root()),
        ops: vec![CeOp::Xml(XmlOp::RenameElement {
            element: edit::path_to(doc, root),
            name: kind.root().to_string(),
        })],
    }])
}

fn rename_element(doc: &XmlDoc, span: Span, message: &str) -> Option<Vec<CeFix>> {
    let node = doc.node_at(span.start)?;
    let element = doc.element(node)?;
    if element.name_span != span {
        return None;
    }
    let name = suggestion(message)?;
    Some(vec![CeFix {
        title: format!("Rename to `<{name}>`"),
        ops: vec![CeOp::Xml(XmlOp::RenameElement {
            element: edit::path_to(doc, node),
            name: name.to_string(),
        })],
    }])
}

fn write_flag(document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    let field = record
        .fields
        .iter()
        .find(|field| field.span == Some(span))?;
    let is_flag = schema::schema(kind)
        .and_then(|schema| schema.section(&document.sections[section].id))
        .and_then(|schema| schema.record.field(&field.id))
        .is_some_and(|field| field.ty == FieldType::Flag);
    if !is_flag {
        return None;
    }
    let value = field.raw.as_deref().and_then(schema::whole)?;
    let replacement = if value == 0 { "0" } else { "1" };
    Some(vec![CeFix {
        title: format!("Write {replacement}"),
        ops: vec![set_field(
            document,
            kind,
            section,
            index,
            &field.id,
            replacement,
        )],
    }])
}

fn remove_duplicate(document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let records = &document.sections[section].records;
    let this = &records[index];
    if this.key_span.unwrap_or(this.span) != span {
        return None;
    }
    let key = key_of(kind, &document.sections[section].id);
    let section_id = document.sections[section].id.clone();
    let shared_elements = schema::schema(kind)?
        .section(&section_id)?
        .record
        .elements
        .len()
        > 1;
    let group: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| ops::key_matches(key, &record.key, &this.key))
        .filter(|(_, record)| !shared_elements || record.element == this.element)
        .map(|(index, _)| index)
        .collect();
    let other_index = *group.iter().find(|other| **other != index)?;
    let other = &records[other_index];
    Some(vec![
        CeFix {
            title: format!("Remove this entry (line {})", this.line),
            ops: vec![CeOp::RemoveRecord {
                section: Some(section_id.clone()),
                record: record_ref(document, kind, section, index),
            }],
        },
        CeFix {
            title: format!("Remove the other entry (line {})", other.line),
            ops: vec![CeOp::RemoveRecord {
                section: Some(section_id),
                record: record_ref(document, kind, section, other_index),
            }],
        },
    ])
}

fn min_above_nominal(document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    let (nominal, min) = (record.field("nominal")?, record.field("min")?);
    if min.span != Some(span) {
        return None;
    }
    let (nominal_raw, min_raw) = (nominal.raw.as_deref()?, min.raw.as_deref()?);
    Some(vec![
        CeFix {
            title: format!("Set min to {nominal_raw}"),
            ops: vec![set_field(
                document,
                kind,
                section,
                index,
                "min",
                nominal_raw,
            )],
        },
        CeFix {
            title: format!("Set nominal to {min_raw}"),
            ops: vec![set_field(
                document, kind, section, index, "nominal", min_raw,
            )],
        },
    ])
}

fn quant_span(document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    let (min, max) = (record.field("quantmin")?, record.field("quantmax")?);
    let (min_raw, min_span) = (min.raw.as_deref()?, min.span?);
    let (max_raw, max_span) = (max.raw.as_deref()?, max.span?);
    let (min_value, max_value) = (schema::whole(min_raw)?, schema::whole(max_raw)?);
    if min_value == -1 || max_value == -1 {
        let negative = if min_value == -1 { min_span } else { max_span };
        if span != negative {
            return None;
        }
        return Some(vec![CeFix {
            title: "Set both to -1".to_string(),
            ops: vec![
                set_field(document, kind, section, index, "quantmin", "-1"),
                set_field(document, kind, section, index, "quantmax", "-1"),
            ],
        }]);
    }
    if min_value > max_value {
        if span != min_span {
            return None;
        }
        return Some(vec![CeFix {
            title: "Swap quantmin and quantmax".to_string(),
            ops: vec![
                set_field(document, kind, section, index, "quantmin", max_raw),
                set_field(document, kind, section, index, "quantmax", min_raw),
            ],
        }]);
    }
    None
}

fn keep_first_category(doc: &XmlDoc, document: &Document, span: Span) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let record = &document.sections[section].records[index];
    let categories = doc.children_named(record.node, "category");
    let second = categories.get(1)?;
    if doc.element(*second).is_none_or(|el| el.name_span != span) {
        return None;
    }
    let name = doc
        .attr(categories[0], "name")
        .map(|attr| attr.value.clone())
        .unwrap_or_default();
    let ops = categories[1..]
        .iter()
        .rev()
        .map(|&category| {
            CeOp::Xml(XmlOp::RemoveElement {
                element: edit::path_to(doc, category),
            })
        })
        .collect();
    Some(vec![CeFix {
        title: format!("Keep `{name}`"),
        ops,
    }])
}

fn rename_class(document: &Document, span: Span, message: &str, code: &str) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    if record.key_span != Some(span) {
        return None;
    }
    let target = suggestion(message)?;
    if target_taken(document, kind, target, record.node) {
        return None;
    }
    let title = if code == "MF-X121" {
        format!("Rename to `{target}`")
    } else {
        format!("Use `{target}`")
    };
    Some(vec![CeFix {
        title,
        ops: vec![CeOp::RenameRecord {
            section: Some(document.sections[section].id.clone()),
            record: record_ref(document, kind, section, index),
            new_key: target.to_string(),
        }],
    }])
}

fn use_name(document: &Document, span: Span, message: &str) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    let target = suggestion(message)?;
    let section_id = document.sections[section].id.clone();
    let reference = record_ref(document, kind, section, index);
    if let Some(field) = record.field("category") {
        if field.span == Some(span) {
            return Some(vec![CeFix {
                title: format!("Use `{target}`"),
                ops: vec![CeOp::SetField {
                    section: Some(section_id),
                    record: reference,
                    field: "category".to_string(),
                    value: Some(target.to_string()),
                }],
            }]);
        }
    }
    list_item_fix(
        record,
        span,
        section_id,
        reference,
        format!("Use `{target}`"),
        target,
    )
}

fn write_include_type(document: &Document, span: Span, message: &str) -> Option<Vec<CeFix>> {
    let (section, index) = model::record_at(document, span.start)?;
    let kind = document.kind?;
    let record = &document.sections[section].records[index];
    let target = write_target(message)?;
    let section_id = document.sections[section].id.clone();
    let reference = record_ref(document, kind, section, index);
    list_item_fix(
        record,
        span,
        section_id,
        reference,
        format!("Write `{target}`"),
        target,
    )
}

fn list_item_fix(
    record: &Record,
    span: Span,
    section_id: String,
    reference: RecordRef,
    title: String,
    target: &str,
) -> Option<Vec<CeFix>> {
    for list in &record.lists {
        for (index, item) in list.items.iter().enumerate() {
            if let Some(attr) = item.attrs.iter().find(|attr| attr.span == span) {
                return Some(vec![CeFix {
                    title,
                    ops: vec![CeOp::SetListItem {
                        section: Some(section_id),
                        record: reference,
                        list: list.id.clone(),
                        index,
                        attr: attr.name.clone(),
                        value: Some(target.to_string()),
                    }],
                }]);
            }
        }
    }
    None
}

fn write_target(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once("Write `")?;
    let (target, _) = rest.split_once('`')?;
    Some(target)
}

pub(crate) const DID_YOU_MEAN: &str = "Did you mean";

pub(crate) fn did_you_mean(nearest: Option<&str>) -> String {
    match nearest {
        Some(name) => format!(" {DID_YOU_MEAN} `{name}`?"),
        None => String::new(),
    }
}

pub(crate) fn suggestion(message: &str) -> Option<&str> {
    const TRIGGERS: [&str; 3] = [DID_YOU_MEAN, " is defined as ", " is declared as "];
    if !TRIGGERS.iter().any(|trigger| message.contains(trigger)) {
        return None;
    }
    let token = message.split('`').skip(1).step_by(2).last()?;
    Some(token.trim_matches(|c| c == '<' || c == '>'))
}

fn set_field(
    document: &Document,
    kind: CeKind,
    section: usize,
    index: usize,
    field: &str,
    value: &str,
) -> CeOp {
    CeOp::SetField {
        section: Some(document.sections[section].id.clone()),
        record: record_ref(document, kind, section, index),
        field: field.to_string(),
        value: Some(value.to_string()),
    }
}

fn record_ref(document: &Document, kind: CeKind, section: usize, index: usize) -> RecordRef {
    let records = &document.sections[section].records;
    let key = key_of(kind, &document.sections[section].id);
    let occurrence = records[..index]
        .iter()
        .filter(|record| ops::key_matches(key, &record.key, &records[index].key))
        .count();
    RecordRef {
        key: records[index].key.clone(),
        occurrence,
    }
}

fn target_taken(document: &Document, kind: CeKind, target: &str, ignore: NodeId) -> bool {
    let Some(schema) = schema::schema(kind) else {
        return false;
    };
    document.sections.iter().any(|section| {
        schema.section(&section.id).is_some_and(|schema| {
            section.records.iter().any(|record| {
                record.node != ignore && ops::key_matches(schema.record.key, &record.key, target)
            })
        })
    })
}

fn key_of(kind: CeKind, section: &str) -> Key {
    schema::schema(kind)
        .and_then(|schema| schema.section(section))
        .map_or(Key::Attr("name"), |schema| schema.record.key)
}

#[cfg(test)]
mod tests {
    use super::suggestion;

    #[test]
    fn reads_the_suggested_name() {
        let rows: [(&str, Option<&str>); 9] = [
            (
                "`<categorie>` isn't part of a `<type>` here. Did you mean `<category>`?",
                Some("category"),
            ),
            (
                "`Toolz` isn't a category this mission defines. Did you mean `tools`?",
                Some("tools"),
            ),
            ("`Tools` is defined as `tools`.", Some("tools")),
            ("`military` is defined as `Military`.", Some("Military")),
            (
                "`Bandana_Blackpattern` is declared as `Bandana_BlackPattern`.",
                Some("Bandana_BlackPattern"),
            ),
            (
                "`FilterCartridge`: min (24) is above nominal (20). The economy never reaches min.",
                None,
            ),
            ("`a` and `b`.", None),
            ("`count_in_cargo` must be 0 or 1, not `2`.", None),
            ("Did you mean something else?", None),
        ];
        for (message, expected) in rows {
            assert_eq!(suggestion(message), expected, "{message}");
        }
    }
}
