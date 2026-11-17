//! The schema-driven record model: one pass over Central Economy XML that
//! yields the sections, records, fields and lists the table, the Details form,
//! the lints and the agents all read.

use crate::kind::{self, CeKind};
use crate::schema::{self, FieldSchema, Key, ListSchema, Place, RecordSchema, SectionSchema};
use crate::xml::{NodeId, XmlDoc};
use modfoundry_core::text::LineIndex;
use modfoundry_core::Span;
use serde::{Deserialize, Serialize};

/// A parsed file: its kind, its sections, and its parse health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub kind: Option<CeKind>,
    pub fragment: bool,
    pub root_mismatch: Option<String>,
    pub sections: Vec<Section>,
    pub issues: usize,
    pub blocking: bool,
}

/// One schema section and the records found for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub label: String,
    pub records: Vec<Record>,
}

/// One record: its key, element, span and the values of its schema fields and lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub key: String,
    pub element: String,
    #[serde(skip)]
    pub node: NodeId,
    pub span: Span,
    pub key_span: Option<Span>,
    /// 1-based source line of the record's start.
    pub line: usize,
    pub fields: Vec<FieldValue>,
    pub lists: Vec<ListValue>,
    pub unknown: Vec<String>,
}

/// One schema field's value at a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldValue {
    pub id: String,
    pub raw: Option<String>,
    pub span: Option<Span>,
    pub valid: bool,
}

/// One schema list's items at a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListValue {
    pub id: String,
    pub items: Vec<ListItem>,
}

/// One list item: its span, attributes and any nested items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    #[serde(skip)]
    pub node: NodeId,
    pub span: Span,
    pub attrs: Vec<ItemAttr>,
    pub nested: Vec<ListItem>,
}

/// One attribute of a list item; `span` covers the decoded-from value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemAttr {
    pub name: String,
    pub value: String,
    pub span: Span,
}

/// Parses `text` and reads the document model from it.
pub fn read(text: &str, file_name: Option<&str>) -> Document {
    let doc = crate::xml::parse(text);
    read_doc(text, &doc, file_name)
}

/// Reads the document model from an already parsed [`XmlDoc`].
pub fn read_doc(text: &str, doc: &XmlDoc, file_name: Option<&str>) -> Document {
    let detected = kind::detect(file_name, doc);
    let lines = LineIndex::new(text);
    let sections = match detected.kind.and_then(schema::schema) {
        Some(kind_schema) => kind_schema
            .sections
            .iter()
            .map(|section| read_section(doc, &lines, section, detected.fragment))
            .collect(),
        None => Vec::new(),
    };
    Document {
        kind: detected.kind,
        fragment: detected.fragment,
        root_mismatch: detected.root_mismatch,
        sections,
        issues: doc.issues.len(),
        blocking: doc.blocking(),
    }
}

/// The `(section, record)` whose span holds `offset`, if any.
pub fn record_at(document: &Document, offset: usize) -> Option<(usize, usize)> {
    document
        .sections
        .iter()
        .enumerate()
        .find_map(|(section, value)| {
            value
                .records
                .iter()
                .position(|record| offset >= record.span.start && offset < record.span.end)
                .map(|record| (section, record))
        })
}

impl Document {
    /// The section with this id, or `None`.
    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.id == id)
    }

    /// The `occurrence`-th record in `section` whose key equals `key`, case-insensitively.
    pub fn find(&self, section: &str, key: &str, occurrence: usize) -> Option<&Record> {
        self.section(section)?
            .records
            .iter()
            .filter(|record| record.key.eq_ignore_ascii_case(key))
            .nth(occurrence)
    }
}

impl Record {
    /// The field with this id, or `None`.
    pub fn field(&self, id: &str) -> Option<&FieldValue> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// The list with this id, or `None`.
    pub fn list(&self, id: &str) -> Option<&ListValue> {
        self.lists.iter().find(|list| list.id == id)
    }

    /// Each item's `name` attribute, else its `user` attribute.
    pub fn names(&self, list: &str) -> Vec<String> {
        let Some(list) = self.list(list) else {
            return Vec::new();
        };
        list.items
            .iter()
            .filter_map(|item| {
                item.attr("name")
                    .or_else(|| item.attr("user"))
                    .map(|attr| attr.value.clone())
            })
            .collect()
    }
}

impl ListItem {
    /// The first attribute named `name`, or `None`.
    pub fn attr(&self, name: &str) -> Option<&ItemAttr> {
        self.attrs.iter().find(|attr| attr.name == name)
    }
}

fn read_section(
    doc: &XmlDoc,
    lines: &LineIndex,
    section: &SectionSchema,
    fragment: bool,
) -> Section {
    let record_schema = &section.record;
    let candidates = if fragment {
        doc.top_elements()
    } else {
        match doc.root() {
            Some(root) => match section.container {
                Some(container) => doc
                    .first_child_named(root, container)
                    .map(|id| doc.child_elements(id))
                    .unwrap_or_default(),
                None => doc.child_elements(root),
            },
            None => Vec::new(),
        }
    };
    let known = record_schema.known_children();
    let mut records = Vec::new();
    for id in candidates {
        let name = doc.element(id).map_or("", |element| element.name.as_str());
        if !record_schema.elements.contains(&name) {
            continue;
        }
        let index = records.len() + 1;
        records.push(read_record(doc, lines, id, record_schema, &known, index));
    }
    Section {
        id: section.id.to_string(),
        label: section.label.to_string(),
        records,
    }
}

fn read_record(
    doc: &XmlDoc,
    lines: &LineIndex,
    id: NodeId,
    schema: &RecordSchema,
    known: &[&str],
    index: usize,
) -> Record {
    let element = doc.element(id).map(|element| element.name.clone());
    let span = doc.nodes[id].span;
    let (key, key_span) = match schema.key {
        Key::Attr(name) => match doc.attr(id, name) {
            Some(attr) => (attr.value.clone(), Some(attr.value_span)),
            None => (String::new(), None),
        },
        Key::ElementName => (element.clone().unwrap_or_default(), None),
        Key::Index => (format!("#{index}"), None),
    };
    let fields = schema
        .fields
        .iter()
        .map(|field| read_field(doc, id, field))
        .collect();
    let lists = schema
        .lists
        .iter()
        .map(|list| ListValue {
            id: list.id.to_string(),
            items: read_items(doc, id, list),
        })
        .collect();
    let mut unknown: Vec<String> = Vec::new();
    for child in doc.child_elements_iter(id) {
        let name = doc
            .element(child)
            .map_or("", |element| element.name.as_str());
        if !known.contains(&name) && !unknown.iter().any(|seen| seen == name) {
            unknown.push(name.to_string());
        }
    }
    Record {
        key,
        element: element.unwrap_or_default(),
        node: id,
        span,
        key_span,
        line: lines.line_of(span.start),
        fields,
        lists,
        unknown,
    }
}

fn read_field(doc: &XmlDoc, id: NodeId, field: &FieldSchema) -> FieldValue {
    let (raw, span) = match field.place {
        Place::ChildText(child) => match doc.first_child_named(id, child) {
            Some(child) => (Some(doc.text_of(child)), doc.text_span(child)),
            None => (None, None),
        },
        Place::ChildAttr { child, attr } => {
            match doc
                .first_child_named(id, child)
                .and_then(|c| doc.attr(c, attr))
            {
                Some(attr) => (Some(attr.value.clone()), Some(attr.value_span)),
                None => (None, None),
            }
        }
        Place::Attr(name) => match doc.attr(id, name) {
            Some(attr) => (Some(attr.value.clone()), Some(attr.value_span)),
            None => (None, None),
        },
        Place::ChildPresent(child) => match doc.first_child_named(id, child) {
            Some(child) => (
                Some("1".to_string()),
                doc.element(child).map(|element| element.name_span),
            ),
            None => (None, None),
        },
    };
    let valid = raw.as_deref().is_none_or(|raw| field.ty.check(raw).is_ok());
    FieldValue {
        id: field.id.to_string(),
        raw,
        span,
        valid,
    }
}

fn read_items(doc: &XmlDoc, parent: NodeId, list: &ListSchema) -> Vec<ListItem> {
    let holder = match list.container {
        Some(container) => match doc.first_child_named(parent, container) {
            Some(id) => id,
            None => return Vec::new(),
        },
        None => parent,
    };
    doc.children_named(holder, list.element)
        .into_iter()
        .map(|id| read_item(doc, id, list))
        .collect()
}

fn read_item(doc: &XmlDoc, id: NodeId, list: &ListSchema) -> ListItem {
    let attrs = doc
        .element(id)
        .map(|element| {
            element
                .attrs
                .iter()
                .map(|attr| ItemAttr {
                    name: attr.name.clone(),
                    value: attr.value.clone(),
                    span: attr.value_span,
                })
                .collect()
        })
        .unwrap_or_default();
    let nested = list
        .nested
        .map(|nested| read_items(doc, id, nested))
        .unwrap_or_default();
    ListItem {
        node: id,
        span: doc.nodes[id].span,
        attrs,
        nested,
    }
}
