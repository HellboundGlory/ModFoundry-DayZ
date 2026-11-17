//! Record-level operations: compile a [`CeOp`] into the [`XmlOp`]s that carry
//! it out, addressing records through the schema.
//!
//! The model reads records; this module writes them. Every path uses
//! `Nth` picks, so two records with the same key never make a path ambiguous.

use serde::{Deserialize, Serialize};

use crate::edit::{self, EditError, NewElement, Position, XmlOp};
use crate::kind::CeKind;
use crate::model::{self, Document};
use crate::schema::{self, FieldSchema, Key, Place, RecordSchema, ValueProblem};
use crate::xml::{NodeId, XmlDoc};

/// Names the `occurrence`-th record whose key matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordRef {
    pub key: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub occurrence: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Where a new record lands among its section's records.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordPosition {
    First,
    #[default]
    Last,
    Sorted,
    After(RecordRef),
}

/// One item of a new record's list, with attributes in written order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewItem {
    pub attrs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nested: Vec<NewItem>,
}

/// A list a new record carries, as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewList {
    pub list: String,
    pub items: Vec<NewItem>,
}

/// One record-level change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CeOp {
    /// Set, or with `None` remove, one field of one record.
    SetField {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        field: String,
        value: Option<String>,
    },
    /// Add a record built from the schema's fields and templates.
    AddRecord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        element: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lists: Vec<NewList>,
        #[serde(default)]
        position: RecordPosition,
    },
    /// Remove one record.
    RemoveRecord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
    },
    /// Rename one record's key.
    RenameRecord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        new_key: String,
    },
    /// Copy one record under a new key.
    DuplicateRecord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        new_key: String,
    },
    /// Append, or insert at `index`, one item of a record's list.
    AddListItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        attrs: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// Remove the `index`-th item of a record's list.
    RemoveListItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        index: usize,
    },
    /// Set, or with `None` remove, one attribute of the `index`-th list item.
    SetListItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        index: usize,
        attr: String,
        value: Option<String>,
    },
    /// Append, or insert at `nested_index`, one nested item of the `index`-th list item.
    AddNestedItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        index: usize,
        attrs: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nested_index: Option<usize>,
    },
    /// Remove the `nested_index`-th nested item of the `index`-th list item.
    RemoveNestedItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        index: usize,
        nested_index: usize,
    },
    /// Set, or with `None` remove, one attribute of a nested item.
    SetNestedItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
        record: RecordRef,
        list: String,
        index: usize,
        nested_index: usize,
        attr: String,
        value: Option<String>,
    },
    /// Wrap a rootless fragment in its kind's root element.
    WrapRoot,
    /// Pass an XML-level op through unchanged.
    Xml(XmlOp),
}

/// Compiles `op` against an already parsed document.
pub fn compile_in(doc: &XmlDoc, document: &Document, op: &CeOp) -> Result<Vec<XmlOp>, EditError> {
    if doc.blocking() {
        return Err(EditError::Unparseable);
    }
    if let CeOp::Xml(op) = op {
        return Ok(vec![op.clone()]);
    }
    let kind = document.kind.ok_or_else(no_table)?;
    if matches!(op, CeOp::WrapRoot) {
        return wrap_root(document, kind);
    }
    let kind_schema = schema::schema(kind).ok_or_else(no_table)?;
    match op {
        CeOp::SetField {
            section,
            record,
            field,
            value,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            if matches!(section_schema.record.key, Key::Attr(key) if field.as_str() == key) {
                return Err(EditError::Unsupported(
                    "Rename the entry instead.".to_string(),
                ));
            }
            let field_schema = section_schema
                .record
                .field(field)
                .ok_or_else(|| EditError::NotFound(format!("No field named `{field}` here.")))?;
            let current = found.field(field).and_then(|field| field.raw.as_deref());
            if current.is_none() && value.is_none() {
                return Ok(Vec::new());
            }
            let check_value = current != value.as_deref();
            set_field(
                doc,
                found.node,
                &section_schema.record,
                field_schema,
                value.as_deref(),
                check_value,
            )
        }
        CeOp::AddRecord {
            section,
            key,
            element,
            fields,
            lists,
            position,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            add_record(
                doc,
                document,
                kind,
                kind_schema,
                section_schema,
                key,
                element.as_deref(),
                fields,
                lists,
                position,
            )
        }
        CeOp::RemoveRecord { section, record } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            remove_record(doc, document, section_schema, record)
        }
        CeOp::RenameRecord {
            section,
            record,
            new_key,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            rename_record(doc, document, kind, section_schema, record, new_key)
        }
        CeOp::DuplicateRecord {
            section,
            record,
            new_key,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            duplicate_record(
                doc,
                document,
                kind,
                kind_schema,
                section_schema,
                record,
                new_key,
            )
        }
        CeOp::AddListItem {
            section,
            record,
            list,
            attrs,
            index,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            add_list_item(
                doc,
                found,
                &section_schema.record,
                list_schema,
                attrs,
                *index,
            )
        }
        CeOp::RemoveListItem {
            section,
            record,
            list,
            index,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            remove_list_item(doc, found, list_schema, *index)
        }
        CeOp::SetListItem {
            section,
            record,
            list,
            index,
            attr,
            value,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            set_list_item(doc, found, list_schema, *index, attr, value.as_deref())
        }
        CeOp::AddNestedItem {
            section,
            record,
            list,
            index,
            attrs,
            nested_index,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            add_nested_item(doc, found, list_schema, *index, attrs, *nested_index)
        }
        CeOp::RemoveNestedItem {
            section,
            record,
            list,
            index,
            nested_index,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            remove_nested_item(doc, found, list_schema, *index, *nested_index)
        }
        CeOp::SetNestedItem {
            section,
            record,
            list,
            index,
            nested_index,
            attr,
            value,
        } => {
            let section_schema = section_for(kind_schema, section.as_deref())?;
            let found = find_record(document, section_schema, record)?;
            let list_schema = list_for(&section_schema.record, list)?;
            set_nested_item(
                doc,
                found,
                list_schema,
                *index,
                *nested_index,
                attr,
                value.as_deref(),
            )
        }
        CeOp::WrapRoot | CeOp::Xml(_) => unreachable!("returned above"),
    }
}

/// Applies `ops` in order; each is compiled against the text the previous produced.
pub fn plan_all(text: &str, file_name: Option<&str>, ops: &[CeOp]) -> Result<String, EditError> {
    let mut current = text.to_string();
    for op in ops {
        let doc = crate::xml::parse(&current);
        if doc.blocking() {
            return Err(EditError::Unparseable);
        }
        let document = model::read_doc(&current, &doc, file_name);
        let xml_ops = compile_in(&doc, &document, op)?;
        let Some((first, rest)) = xml_ops.split_first() else {
            continue;
        };
        let edits = edit::plan_in(&current, &doc, first)?;
        current = edit::apply(&current, &edits);
        if !rest.is_empty() {
            current = edit::plan_all(&current, rest)?;
        }
    }
    debug_assert!(
        check_result(text, &current).is_ok(),
        "plan_all produced an invalid document: {}",
        check_result(text, &current).unwrap_err()
    );
    Ok(current)
}

/// Checks a planned result stayed lossless: it parses with no blocking issue,
/// rebuilds to itself, and carries no more `MF-X` issues than the input.
pub fn check_result(before: &str, after: &str) -> Result<(), String> {
    let doc = crate::xml::parse(after);
    if doc.blocking() {
        return Err("the result has a syntax error".to_string());
    }
    if doc.rebuild(after) != after {
        return Err("the result does not rebuild to itself".to_string());
    }
    let before_issues = crate::xml::parse(before).issues.len();
    if doc.issues.len() > before_issues {
        return Err(format!(
            "the result has {} parse issues, more than the {before_issues} before",
            doc.issues.len()
        ));
    }
    Ok(())
}

/// Where a new child named `child` goes: before the first existing child whose
/// name comes later in the schema's `order`, else last.
pub fn canonical_index(
    doc: &XmlDoc,
    record: NodeId,
    schema: &RecordSchema,
    child: &str,
) -> Position {
    let Some(target) = schema.order.iter().position(|name| *name == child) else {
        return Position::Last;
    };
    for (index, id) in doc.child_elements(record).iter().enumerate() {
        let Some(name) = doc.element(*id).map(|element| element.name.as_str()) else {
            continue;
        };
        if schema
            .order
            .iter()
            .position(|order| *order == name)
            .is_some_and(|position| position > target)
        {
            return Position::Index(index);
        }
    }
    Position::Last
}

fn no_table() -> EditError {
    EditError::Unsupported("This file has no table view.".to_string())
}

fn section_for<'a>(
    kind_schema: &'a schema::KindSchema,
    section: Option<&str>,
) -> Result<&'a schema::SectionSchema, EditError> {
    match section {
        Some(id) => kind_schema
            .section(id)
            .ok_or_else(|| EditError::NotFound(format!("No section named `{id}` here."))),
        None => kind_schema.sections.first().ok_or_else(no_table),
    }
}

fn find_record<'a>(
    document: &'a Document,
    section: &schema::SectionSchema,
    record: &RecordRef,
) -> Result<&'a model::Record, EditError> {
    document
        .section(section.id)
        .ok_or_else(no_table)?
        .records
        .iter()
        .filter(|candidate| key_matches(section.record.key, &candidate.key, &record.key))
        .nth(record.occurrence)
        .ok_or_else(|| {
            EditError::NotFound(format!(
                "No {} named `{}` here.",
                section.record.noun, record.key
            ))
        })
}

fn taken(
    document: &Document,
    section: &schema::SectionSchema,
    key: &str,
    element: Option<&str>,
    ignore: Option<NodeId>,
) -> bool {
    if matches!(section.record.key, Key::Index) {
        return false;
    }
    let shared_elements = section.record.elements.len() > 1;
    match document.section(section.id) {
        Some(data) => data.records.iter().any(|record| {
            Some(record.node) != ignore
                && (!shared_elements || element.is_none_or(|element| record.element == element))
                && key_matches(section.record.key, &record.key, key)
        }),
        None => false,
    }
}

fn validate_key(kind: CeKind, section: &schema::SectionSchema, key: &str) -> Result<(), EditError> {
    validate_key_for(kind, section, key, true)
}

/// A rename only needs a name that is safe to write; it must be able to restore a
/// name the file already carried, and some shipped types carry odd ones.
fn validate_rename_key(
    kind: CeKind,
    section: &schema::SectionSchema,
    key: &str,
) -> Result<(), EditError> {
    validate_key_for(kind, section, key, false)
}

fn validate_key_for(
    kind: CeKind,
    section: &schema::SectionSchema,
    key: &str,
    class_names: bool,
) -> Result<(), EditError> {
    match section.record.key {
        Key::Index => Ok(()),
        Key::ElementName => {
            if section.record.elements.contains(&key) {
                Ok(())
            } else {
                Err(EditError::InvalidName(format!(
                    "`{key}` is not one of this section's elements."
                )))
            }
        }
        Key::Attr(_) if class_names && kind == CeKind::Types => {
            if schema::is_class_name(key) {
                Ok(())
            } else {
                Err(EditError::InvalidName(schema::CLASS_NAME_RULE.to_string()))
            }
        }
        Key::Attr(_) => {
            if key.is_empty() {
                Err(EditError::InvalidName(
                    "An entry name can't be empty.".to_string(),
                ))
            } else if key.contains(['"', '<', '>', '&']) {
                Err(EditError::InvalidName(
                    "An entry name can't contain `\"`, `<`, `>` or `&`.".to_string(),
                ))
            } else {
                Ok(())
            }
        }
    }
}

fn record_element(
    section: &schema::SectionSchema,
    element: Option<&str>,
    key: &str,
) -> Result<String, EditError> {
    if matches!(section.record.key, Key::ElementName) {
        return Ok(key.to_string());
    }
    if section.record.elements.len() == 1 {
        return Ok(section.record.elements[0].to_string());
    }
    match element {
        Some(name) if section.record.elements.contains(&name) => Ok(name.to_string()),
        _ => Err(EditError::InvalidName(
            "This section needs an element name.".to_string(),
        )),
    }
}

fn record_attrs(
    section: &schema::SectionSchema,
    key: &str,
    fields: &[(String, String)],
    templates: bool,
    check_values: bool,
) -> Result<Vec<(String, String)>, EditError> {
    let record = &section.record;
    let mut attrs = match record.key {
        Key::Attr(name) => vec![(name.to_string(), key.to_string())],
        Key::ElementName | Key::Index => Vec::new(),
    };
    for field in record.fields {
        let Place::Attr(name) = field.place else {
            continue;
        };
        if let Some(value) = checked_value(fields, field, templates, check_values)? {
            attrs.push((name.to_string(), value));
        }
    }
    Ok(attrs)
}

#[allow(clippy::too_many_arguments)]
fn add_record(
    doc: &XmlDoc,
    document: &Document,
    kind: CeKind,
    kind_schema: &schema::KindSchema,
    section: &schema::SectionSchema,
    key: &str,
    element: Option<&str>,
    fields: &[(String, String)],
    lists: &[NewList],
    position: &RecordPosition,
) -> Result<Vec<XmlOp>, EditError> {
    validate_key(kind, section, key)?;
    let element = record_element(section, element, key)?;
    if taken(document, section, key, Some(&element), None) {
        return Err(EditError::Exists(format!("`{key}` already has an entry.")));
    }
    let record = NewElement {
        name: element,
        attrs: record_attrs(section, key, fields, true, true)?,
        children: build_children(section, fields, lists, true, true)?,
        text: None,
    };
    place_record(
        doc,
        document,
        kind,
        kind_schema,
        section,
        record,
        position,
        key,
    )
}

fn remove_record(
    doc: &XmlDoc,
    document: &Document,
    section: &schema::SectionSchema,
    record: &RecordRef,
) -> Result<Vec<XmlOp>, EditError> {
    let found = find_record(document, section, record)?;
    Ok(vec![XmlOp::RemoveElement {
        element: edit::path_to(doc, found.node),
    }])
}

fn rename_record(
    doc: &XmlDoc,
    document: &Document,
    kind: CeKind,
    section: &schema::SectionSchema,
    record: &RecordRef,
    new_key: &str,
) -> Result<Vec<XmlOp>, EditError> {
    let found = find_record(document, section, record)?;
    let Key::Attr(attr) = section.record.key else {
        return Err(EditError::Unsupported(
            "This kind names entries by their element, so rename isn't supported.".to_string(),
        ));
    };
    validate_rename_key(kind, section, new_key)?;
    if taken(
        document,
        section,
        new_key,
        Some(&found.element),
        Some(found.node),
    ) {
        return Err(EditError::Exists(format!(
            "`{new_key}` already has an entry."
        )));
    }
    Ok(vec![XmlOp::SetAttr {
        element: edit::path_to(doc, found.node),
        name: attr.to_string(),
        value: new_key.to_string(),
    }])
}

/// The values a copy of `record` carries: its raw fields and its lists.
pub fn copy_values(record: &model::Record) -> (Vec<(String, String)>, Vec<NewList>) {
    let fields = record
        .fields
        .iter()
        .filter_map(|field| field.raw.clone().map(|raw| (field.id.clone(), raw)))
        .collect();
    let lists = record
        .lists
        .iter()
        .map(|list| NewList {
            list: list.id.clone(),
            items: list.items.iter().map(new_item).collect(),
        })
        .collect();
    (fields, lists)
}

fn duplicate_record(
    doc: &XmlDoc,
    document: &Document,
    kind: CeKind,
    kind_schema: &schema::KindSchema,
    section: &schema::SectionSchema,
    record: &RecordRef,
    new_key: &str,
) -> Result<Vec<XmlOp>, EditError> {
    let source = find_record(document, section, record)?;
    validate_key(kind, section, new_key)?;
    let element = match section.record.key {
        Key::ElementName => new_key.to_string(),
        _ => source.element.clone(),
    };
    if taken(document, section, new_key, Some(&element), None) {
        return Err(EditError::Exists(format!(
            "`{new_key}` already has an entry."
        )));
    }
    let (fields, lists) = copy_values(source);
    let copy = NewElement {
        name: element,
        attrs: record_attrs(section, new_key, &fields, false, false)?,
        children: build_children(section, &fields, &lists, false, false)?,
        text: None,
    };
    let position = RecordPosition::After(RecordRef {
        key: record.key.clone(),
        occurrence: record.occurrence,
    });
    place_record(
        doc,
        document,
        kind,
        kind_schema,
        section,
        copy,
        &position,
        new_key,
    )
}

fn wrap_root(document: &Document, kind: CeKind) -> Result<Vec<XmlOp>, EditError> {
    if document.fragment {
        Ok(vec![XmlOp::WrapTop {
            name: kind.root().to_string(),
        }])
    } else {
        Err(EditError::Exists(format!(
            "This file already has its <{}> root.",
            kind.root()
        )))
    }
}

fn list_for<'a>(record: &'a RecordSchema, id: &str) -> Result<&'a schema::ListSchema, EditError> {
    record
        .list(id)
        .ok_or_else(|| EditError::NotFound(format!("No list named `{id}` here.")))
}

fn nested_for(list: &schema::ListSchema) -> Result<&schema::ListSchema, EditError> {
    list.nested
        .ok_or_else(|| EditError::NotFound(format!("List `{}` has no nested items.", list.id)))
}

fn list_attr(list: &schema::ListSchema, id: &str) -> Result<&'static FieldSchema, EditError> {
    list.attrs
        .iter()
        .find(|field| field.id == id)
        .ok_or_else(|| EditError::NotFound(format!("No attribute named `{id}` here.")))
}

fn item_identities(item: &model::ListItem) -> Vec<&str> {
    item.attrs
        .iter()
        .filter(|attr| attr.name == "name" || attr.name == "user")
        .map(|attr| attr.value.as_str())
        .collect()
}

fn ensure_unique(
    record: &model::Record,
    list: &schema::ListSchema,
    attrs: &[(String, String)],
    skip: Option<usize>,
) -> Result<(), EditError> {
    if !list.chips {
        return Ok(());
    }
    let Some(value) = record.list(list.id) else {
        return Ok(());
    };
    for (index, item) in value.items.iter().enumerate() {
        if Some(index) == skip {
            continue;
        }
        for existing in item_identities(item) {
            if attrs.iter().any(|(name, value)| {
                (name == "name" || name == "user") && value.eq_ignore_ascii_case(existing)
            }) {
                return Err(EditError::Exists(format!(
                    "`{existing}` is already listed."
                )));
            }
        }
    }
    Ok(())
}

fn check_attrs(list: &schema::ListSchema, attrs: &[(String, String)]) -> Result<(), EditError> {
    for (name, value) in attrs {
        check(list_attr(list, name)?, value)?;
    }
    Ok(())
}

fn child_offset(doc: &XmlDoc, parent: NodeId, node: NodeId) -> usize {
    doc.child_elements(parent)
        .iter()
        .position(|&id| id == node)
        .expect("a list item is a child of its record")
}

fn add_list_item(
    doc: &XmlDoc,
    record: &model::Record,
    schema: &RecordSchema,
    list: &schema::ListSchema,
    attrs: &[(String, String)],
    index: Option<usize>,
) -> Result<Vec<XmlOp>, EditError> {
    check_attrs(list, attrs)?;
    ensure_unique(record, list, attrs, None)?;
    let element = build_item(
        list,
        &NewItem {
            attrs: attrs.to_vec(),
            nested: Vec::new(),
        },
    );
    let holder = match list.container {
        Some(container) => doc.first_child_named(record.node, container),
        None => Some(record.node),
    };
    let Some(holder) = holder else {
        let container = list
            .container
            .expect("a missing holder is a missing container");
        if index.unwrap_or(0) > 0 {
            return Err(EditError::IndexOutOfRange {
                len: 0,
                index: index.unwrap_or(0),
            });
        }
        return Ok(vec![XmlOp::InsertElement {
            parent: edit::path_to(doc, record.node),
            at: canonical_index(doc, record.node, schema, container),
            element: NewElement {
                name: container.to_string(),
                children: vec![element],
                ..NewElement::default()
            },
        }]);
    };
    let items = doc.children_named(holder, list.element);
    let at = list_item_position(doc, holder, schema, list, &items, index)?;
    Ok(vec![XmlOp::InsertElement {
        parent: edit::path_to(doc, holder),
        at,
        element,
    }])
}

fn list_item_position(
    doc: &XmlDoc,
    holder: NodeId,
    schema: &RecordSchema,
    list: &schema::ListSchema,
    items: &[NodeId],
    index: Option<usize>,
) -> Result<Position, EditError> {
    let child = list.container.unwrap_or(list.element);
    let append = |doc: &XmlDoc, items: &[NodeId]| match items.last() {
        Some(&last) => Position::Index(child_offset(doc, holder, last) + 1),
        None => canonical_index(doc, holder, schema, child),
    };
    match index {
        Some(index) => {
            if index > items.len() {
                return Err(EditError::IndexOutOfRange {
                    len: items.len(),
                    index,
                });
            }
            if index == items.len() {
                Ok(append(doc, items))
            } else {
                Ok(Position::Index(child_offset(doc, holder, items[index])))
            }
        }
        None => Ok(append(doc, items)),
    }
}

fn list_items<'a>(
    record: &'a model::Record,
    list: &schema::ListSchema,
) -> Result<&'a [model::ListItem], EditError> {
    record
        .list(list.id)
        .map(|value| value.items.as_slice())
        .ok_or_else(|| EditError::NotFound(format!("No list named `{}` here.", list.id)))
}

fn remove_list_item(
    doc: &XmlDoc,
    record: &model::Record,
    list: &schema::ListSchema,
    index: usize,
) -> Result<Vec<XmlOp>, EditError> {
    let items = list_items(record, list)?;
    let item = items.get(index).ok_or(EditError::IndexOutOfRange {
        len: items.len(),
        index,
    })?;
    Ok(vec![XmlOp::RemoveElement {
        element: edit::path_to(doc, item.node),
    }])
}

fn set_list_item(
    doc: &XmlDoc,
    record: &model::Record,
    list: &schema::ListSchema,
    index: usize,
    attr: &str,
    value: Option<&str>,
) -> Result<Vec<XmlOp>, EditError> {
    let field = list_attr(list, attr)?;
    let item = outer_item(record, list, index)?;
    let changed = item.attr(attr).map(|existing| existing.value.as_str()) != value;
    let ops = set_item_attr(doc, field, item, attr, value)?;
    if !changed {
        return Ok(ops);
    }
    if let Some(value) = value {
        if attr == "name" || attr == "user" {
            ensure_unique(
                record,
                list,
                &[(attr.to_string(), value.to_string())],
                Some(index),
            )?;
        }
    }
    Ok(ops)
}

fn set_item_attr(
    doc: &XmlDoc,
    field: &FieldSchema,
    item: &model::ListItem,
    attr: &str,
    value: Option<&str>,
) -> Result<Vec<XmlOp>, EditError> {
    match value {
        Some(value) => {
            if item.attr(attr).map(|existing| existing.value.as_str()) != Some(value) {
                check(field, value)?;
            }
            Ok(vec![XmlOp::SetAttr {
                element: edit::path_to(doc, item.node),
                name: attr.to_string(),
                value: value.to_string(),
            }])
        }
        None if item.attr(attr).is_some() => Ok(vec![XmlOp::RemoveAttr {
            element: edit::path_to(doc, item.node),
            name: attr.to_string(),
        }]),
        None => Ok(Vec::new()),
    }
}

fn outer_item<'a>(
    record: &'a model::Record,
    list: &schema::ListSchema,
    index: usize,
) -> Result<&'a model::ListItem, EditError> {
    let items = list_items(record, list)?;
    items.get(index).ok_or(EditError::IndexOutOfRange {
        len: items.len(),
        index,
    })
}

fn nested_at(
    doc: &XmlDoc,
    outer: &model::ListItem,
    nested_index: usize,
) -> Result<Position, EditError> {
    if nested_index > outer.nested.len() {
        return Err(EditError::IndexOutOfRange {
            len: outer.nested.len(),
            index: nested_index,
        });
    }
    match outer.nested.get(nested_index) {
        Some(item) => Ok(Position::Index(child_offset(doc, outer.node, item.node))),
        None => Ok(after_last(doc, outer)),
    }
}

fn after_last(doc: &XmlDoc, outer: &model::ListItem) -> Position {
    match outer.nested.last() {
        Some(last) => Position::Index(child_offset(doc, outer.node, last.node) + 1),
        None => Position::First,
    }
}

fn add_nested_item(
    doc: &XmlDoc,
    record: &model::Record,
    list: &schema::ListSchema,
    index: usize,
    attrs: &[(String, String)],
    nested_index: Option<usize>,
) -> Result<Vec<XmlOp>, EditError> {
    let nested = nested_for(list)?;
    check_attrs(nested, attrs)?;
    let outer = outer_item(record, list, index)?;
    let at = match nested_index {
        Some(nested_index) => nested_at(doc, outer, nested_index)?,
        None => after_last(doc, outer),
    };
    Ok(vec![XmlOp::InsertElement {
        parent: edit::path_to(doc, outer.node),
        at,
        element: build_item(
            nested,
            &NewItem {
                attrs: attrs.to_vec(),
                nested: Vec::new(),
            },
        ),
    }])
}

fn remove_nested_item(
    doc: &XmlDoc,
    record: &model::Record,
    list: &schema::ListSchema,
    index: usize,
    nested_index: usize,
) -> Result<Vec<XmlOp>, EditError> {
    let outer = outer_item(record, list, index)?;
    let item = outer
        .nested
        .get(nested_index)
        .ok_or(EditError::IndexOutOfRange {
            len: outer.nested.len(),
            index: nested_index,
        })?;
    Ok(vec![XmlOp::RemoveElement {
        element: edit::path_to(doc, item.node),
    }])
}

fn set_nested_item(
    doc: &XmlDoc,
    record: &model::Record,
    list: &schema::ListSchema,
    index: usize,
    nested_index: usize,
    attr: &str,
    value: Option<&str>,
) -> Result<Vec<XmlOp>, EditError> {
    let nested = nested_for(list)?;
    let field = list_attr(nested, attr)?;
    let outer = outer_item(record, list, index)?;
    let item = outer
        .nested
        .get(nested_index)
        .ok_or(EditError::IndexOutOfRange {
            len: outer.nested.len(),
            index: nested_index,
        })?;
    set_item_attr(doc, field, item, attr, value)
}

fn new_item(item: &model::ListItem) -> NewItem {
    NewItem {
        attrs: item
            .attrs
            .iter()
            .map(|attr| (attr.name.clone(), attr.value.clone()))
            .collect(),
        nested: item.nested.iter().map(new_item).collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn place_record(
    doc: &XmlDoc,
    document: &Document,
    kind: CeKind,
    kind_schema: &schema::KindSchema,
    section: &schema::SectionSchema,
    element: NewElement,
    position: &RecordPosition,
    key: &str,
) -> Result<Vec<XmlOp>, EditError> {
    let records = &document.section(section.id).ok_or_else(no_table)?.records;
    if document.fragment {
        let at = position_for(doc, None, section, records, position, key)?;
        return Ok(vec![XmlOp::InsertElement {
            parent: edit::XmlPath::default(),
            at,
            element,
        }]);
    }
    let root = doc
        .root()
        .ok_or_else(|| EditError::Unsupported(format!("Add the <{}> root first.", kind.root())))?;
    match section.container {
        None => {
            let at = position_for(doc, Some(root), section, records, position, key)?;
            Ok(vec![XmlOp::InsertElement {
                parent: edit::path_to(doc, root),
                at,
                element,
            }])
        }
        Some(container) => match doc.first_child_named(root, container) {
            Some(id) => {
                let at = position_for(doc, Some(id), section, records, position, key)?;
                Ok(vec![XmlOp::InsertElement {
                    parent: edit::path_to(doc, id),
                    at,
                    element,
                }])
            }
            None => {
                let at = container_position(doc, root, kind_schema, section);
                let mut parent = edit::path_to(doc, root);
                parent.0.push(edit::Step {
                    name: container.to_string(),
                    pick: edit::Pick::Nth(0),
                });
                Ok(vec![
                    XmlOp::InsertElement {
                        parent: edit::path_to(doc, root),
                        at,
                        element: NewElement {
                            name: container.to_string(),
                            ..NewElement::default()
                        },
                    },
                    XmlOp::InsertElement {
                        parent,
                        at: Position::Last,
                        element,
                    },
                ])
            }
        },
    }
}

fn container_position(
    doc: &XmlDoc,
    root: NodeId,
    kind_schema: &schema::KindSchema,
    section: &schema::SectionSchema,
) -> Position {
    let index = kind_schema
        .sections
        .iter()
        .position(|candidate| candidate.id == section.id)
        .unwrap_or(usize::MAX);
    for (offset, child) in doc.child_elements(root).iter().enumerate() {
        let Some(name) = doc.element(*child).map(|element| element.name.as_str()) else {
            continue;
        };
        if kind_schema
            .sections
            .iter()
            .position(|candidate| candidate.container == Some(name))
            .is_some_and(|position| position > index)
        {
            return Position::Index(offset);
        }
    }
    Position::Last
}

fn position_for(
    doc: &XmlDoc,
    parent: Option<NodeId>,
    section: &schema::SectionSchema,
    records: &[model::Record],
    position: &RecordPosition,
    key: &str,
) -> Result<Position, EditError> {
    match position {
        RecordPosition::First => Ok(Position::First),
        RecordPosition::Last => Ok(Position::Last),
        RecordPosition::After(reference) => {
            let record = records
                .iter()
                .filter(|candidate| key_matches(section.record.key, &candidate.key, &reference.key))
                .nth(reference.occurrence)
                .ok_or_else(|| {
                    EditError::NotFound(format!(
                        "No {} named `{}` here.",
                        section.record.noun, reference.key
                    ))
                })?;
            match siblings_of(doc, parent)
                .iter()
                .position(|node| *node == record.node)
            {
                Some(offset) => Ok(Position::Index(offset + 1)),
                None => Ok(Position::Last),
            }
        }
        RecordPosition::Sorted => {
            if !sorted(records) {
                return Ok(Position::Last);
            }
            match records
                .iter()
                .find(|record| greater(&record.key, key))
                .and_then(|record| {
                    siblings_of(doc, parent)
                        .iter()
                        .position(|node| *node == record.node)
                }) {
                Some(offset) => Ok(Position::Index(offset)),
                None => Ok(Position::Last),
            }
        }
    }
}

fn siblings_of(doc: &XmlDoc, parent: Option<NodeId>) -> Vec<NodeId> {
    match parent {
        Some(id) => doc.child_elements(id),
        None => doc.top_elements(),
    }
}

fn sorted(records: &[model::Record]) -> bool {
    records
        .windows(2)
        .all(|pair| !greater(&pair[0].key, &pair[1].key))
}

fn greater(a: &str, b: &str) -> bool {
    a.bytes()
        .map(|b| b.to_ascii_lowercase())
        .gt(b.bytes().map(|b| b.to_ascii_lowercase()))
}

fn build_children(
    section: &schema::SectionSchema,
    fields: &[(String, String)],
    lists: &[NewList],
    templates: bool,
    check_values: bool,
) -> Result<Vec<NewElement>, EditError> {
    let record = &section.record;
    for (id, _) in fields {
        if record.field(id).is_none() {
            return Err(EditError::NotFound(format!("No field named `{id}` here.")));
        }
    }
    for list in lists {
        if record.list(&list.list).is_none() {
            return Err(EditError::NotFound(format!(
                "No list named `{}` here.",
                list.list
            )));
        }
    }
    let mut children = Vec::new();
    for name in record.order {
        let mut attrs = Vec::new();
        for field in record.fields {
            if field_child(field) != Some(*name) {
                continue;
            }
            let Some(value) = checked_value(fields, field, templates, check_values)? else {
                continue;
            };
            match field.place {
                Place::ChildText(child) => children.push(NewElement {
                    name: child.to_string(),
                    text: Some(value),
                    ..NewElement::default()
                }),
                Place::ChildPresent(child) => {
                    if value == "1" {
                        children.push(NewElement {
                            name: child.to_string(),
                            ..NewElement::default()
                        });
                    }
                }
                Place::ChildAttr { attr, .. } => attrs.push((attr.to_string(), value)),
                Place::Attr(_) => {}
            }
        }
        if !attrs.is_empty() {
            children.push(NewElement {
                name: name.to_string(),
                attrs,
                ..NewElement::default()
            });
        }
        if let Some(list) = record
            .lists
            .iter()
            .find(|candidate| candidate.container.unwrap_or(candidate.element) == *name)
        {
            if let Some(new_list) = lists.iter().find(|candidate| candidate.list == list.id) {
                let items: Vec<NewElement> = new_list
                    .items
                    .iter()
                    .map(|item| build_item(list, item))
                    .collect();
                match list.container {
                    Some(container) => children.push(NewElement {
                        name: container.to_string(),
                        children: items,
                        ..NewElement::default()
                    }),
                    None => children.extend(items),
                }
            }
        }
    }
    Ok(children)
}

fn field_child(field: &FieldSchema) -> Option<&str> {
    match field.place {
        Place::ChildText(child) | Place::ChildPresent(child) | Place::ChildAttr { child, .. } => {
            Some(child)
        }
        Place::Attr(_) => None,
    }
}

fn field_value(
    fields: &[(String, String)],
    field: &FieldSchema,
    templates: bool,
) -> Option<String> {
    provided(fields, field.id).map(str::to_string).or_else(|| {
        if templates {
            field.template.map(str::to_string)
        } else {
            None
        }
    })
}

fn checked_value(
    fields: &[(String, String)],
    field: &FieldSchema,
    templates: bool,
    check_values: bool,
) -> Result<Option<String>, EditError> {
    let value = field_value(fields, field, templates);
    if let Some(value) = &value {
        if check_values && provided(fields, field.id).is_some() {
            check(field, value)?;
        }
    }
    Ok(value)
}

fn provided<'a>(fields: &'a [(String, String)], id: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(name, _)| name == id)
        .map(|(_, value)| value.as_str())
}

fn build_item(list: &schema::ListSchema, item: &NewItem) -> NewElement {
    let children = list
        .nested
        .map(|nested| {
            item.nested
                .iter()
                .map(|child| build_item(nested, child))
                .collect()
        })
        .unwrap_or_default();
    NewElement {
        name: list.element.to_string(),
        attrs: item
            .attrs
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        children,
        text: None,
    }
}

pub(crate) fn key_matches(key: Key, found: &str, wanted: &str) -> bool {
    match key {
        Key::Attr(_) => found.eq_ignore_ascii_case(wanted),
        Key::ElementName | Key::Index => found == wanted,
    }
}

fn check(field: &FieldSchema, value: &str) -> Result<(), EditError> {
    field.ty.check(value).map_err(|problem| {
        let text = match problem {
            ValueProblem::Invalid(text) | ValueProblem::OutOfRange(text) => text,
        };
        EditError::InvalidValue(format!("{} must be {}.", field.label, text))
    })
}

fn child_attrs(
    schema: &RecordSchema,
    child: &str,
    target: &str,
    value: &str,
) -> Vec<(String, String)> {
    schema
        .fields
        .iter()
        .filter_map(|field| {
            let Place::ChildAttr { child: name, attr } = field.place else {
                return None;
            };
            if name != child {
                return None;
            }
            let raw = if field.id == target {
                value
            } else {
                field.template?
            };
            Some((attr.to_string(), raw.to_string()))
        })
        .collect()
}

fn set_field(
    doc: &XmlDoc,
    record: NodeId,
    schema: &RecordSchema,
    field: &FieldSchema,
    value: Option<&str>,
    check_value: bool,
) -> Result<Vec<XmlOp>, EditError> {
    match field.place {
        Place::ChildText(child) => match value {
            Some(value) => {
                if check_value {
                    check(field, value)?;
                }
                Ok(vec![match doc.first_child_named(record, child) {
                    Some(id) => XmlOp::SetText {
                        element: edit::path_to(doc, id),
                        text: value.to_string(),
                    },
                    None => XmlOp::InsertElement {
                        parent: edit::path_to(doc, record),
                        at: canonical_index(doc, record, schema, child),
                        element: NewElement {
                            name: child.to_string(),
                            text: Some(value.to_string()),
                            ..NewElement::default()
                        },
                    },
                }])
            }
            None => Ok(match doc.first_child_named(record, child) {
                Some(id) => vec![XmlOp::RemoveElement {
                    element: edit::path_to(doc, id),
                }],
                None => Vec::new(),
            }),
        },
        Place::ChildAttr { child, attr } => match value {
            Some(value) => {
                if check_value {
                    check(field, value)?;
                }
                Ok(vec![match doc.first_child_named(record, child) {
                    Some(id) => XmlOp::SetAttr {
                        element: edit::path_to(doc, id),
                        name: attr.to_string(),
                        value: value.to_string(),
                    },
                    None => XmlOp::InsertElement {
                        parent: edit::path_to(doc, record),
                        at: canonical_index(doc, record, schema, child),
                        element: NewElement {
                            name: child.to_string(),
                            attrs: child_attrs(schema, child, field.id, value),
                            ..NewElement::default()
                        },
                    },
                }])
            }
            None => {
                let Some(id) = doc.first_child_named(record, child) else {
                    return Ok(Vec::new());
                };
                let attrs = &doc.element(id).expect("a child node is an element").attrs;
                if !attrs.iter().any(|a| a.name == attr) {
                    return Ok(Vec::new());
                }
                if attrs.len() == 1 {
                    Ok(vec![XmlOp::RemoveElement {
                        element: edit::path_to(doc, id),
                    }])
                } else {
                    Ok(vec![XmlOp::RemoveAttr {
                        element: edit::path_to(doc, id),
                        name: attr.to_string(),
                    }])
                }
            }
        },
        Place::Attr(name) => {
            if matches!(schema.key, Key::Attr(key) if key == name) {
                return Err(EditError::Unsupported(
                    "Rename the entry instead.".to_string(),
                ));
            }
            match value {
                Some(value) => {
                    if check_value {
                        check(field, value)?;
                    }
                    Ok(vec![XmlOp::SetAttr {
                        element: edit::path_to(doc, record),
                        name: name.to_string(),
                        value: value.to_string(),
                    }])
                }
                None if doc.attr(record, name).is_some() => Ok(vec![XmlOp::RemoveAttr {
                    element: edit::path_to(doc, record),
                    name: name.to_string(),
                }]),
                None => Ok(Vec::new()),
            }
        }
        Place::ChildPresent(child) => match value {
            Some("1") => {
                if doc.first_child_named(record, child).is_some() {
                    return Ok(Vec::new());
                }
                Ok(vec![XmlOp::InsertElement {
                    parent: edit::path_to(doc, record),
                    at: canonical_index(doc, record, schema, child),
                    element: NewElement {
                        name: child.to_string(),
                        ..NewElement::default()
                    },
                }])
            }
            Some("0") | None => Ok(match doc.first_child_named(record, child) {
                Some(id) => vec![XmlOp::RemoveElement {
                    element: edit::path_to(doc, id),
                }],
                None => Vec::new(),
            }),
            Some(_) => Err(EditError::InvalidValue(format!(
                "{} must be 0 or 1.",
                field.label
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, Record, Section};
    use crate::schema::{ListSchema, SectionSchema};

    static ELEMENTS: [&str; 2] = ["dynamic", "animals"];
    static NO_FIELDS: [FieldSchema; 0] = [];
    static NO_LISTS: [ListSchema; 0] = [];
    static NO_ORDER: [&str; 0] = [];

    fn section(key: Key) -> SectionSchema {
        SectionSchema {
            id: "economy",
            label: "Economy",
            container: None,
            record: RecordSchema {
                elements: &ELEMENTS,
                key,
                noun: "entry",
                plural: "entries",
                fields: &NO_FIELDS,
                lists: &NO_LISTS,
                order: &NO_ORDER,
            },
        }
    }

    fn record(key: &str) -> Record {
        Record {
            key: key.to_string(),
            element: key.to_string(),
            node: 0,
            span: crate::Span::new(0, 0),
            key_span: None,
            line: 1,
            fields: Vec::new(),
            lists: Vec::new(),
            unknown: Vec::new(),
        }
    }

    fn document(records: Vec<Record>) -> Document {
        Document {
            kind: Some(CeKind::Economy),
            fragment: false,
            root_mismatch: None,
            sections: vec![Section {
                id: "economy".to_string(),
                label: "Economy".to_string(),
                records,
            }],
            issues: 0,
            blocking: false,
        }
    }

    #[test]
    fn element_name_keys_are_the_sections_elements_and_stay_unique() {
        let economy = section(Key::ElementName);
        assert_eq!(validate_key(CeKind::Economy, &economy, "dynamic"), Ok(()));
        assert_eq!(
            validate_key(CeKind::Economy, &economy, "nope"),
            Err(EditError::InvalidName(
                "`nope` is not one of this section's elements.".into()
            ))
        );
        let doc = document(vec![record("dynamic")]);
        assert!(taken(&doc, &economy, "dynamic", None, None));
        assert!(!taken(&doc, &economy, "animals", None, None));
        assert!(!taken(&doc, &economy, "Dynamic", None, None));
    }

    #[test]
    fn index_keys_ignore_the_key() {
        let index = section(Key::Index);
        assert_eq!(validate_key(CeKind::Economy, &index, "anything"), Ok(()));
        let doc = document(vec![record("anything")]);
        assert!(!taken(&doc, &index, "anything", None, None));
    }

    #[test]
    fn check_result_accepts_a_clean_edit() {
        let before =
            "<types>\n    <type name=\"A\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";
        let after =
            "<types>\n    <type name=\"A\">\n        <nominal>2</nominal>\n    </type>\n</types>\n";
        assert_eq!(check_result(before, after), Ok(()));
        assert_eq!(check_result(before, before), Ok(()));
    }

    #[test]
    fn check_result_rejects_an_unclosed_tag() {
        let before = "<types>\n</types>\n";
        assert!(check_result(before, "<types>\n    <type name=\"A\">\n").is_err());
    }

    fn read(text: &str) -> (XmlDoc, Document) {
        let doc = crate::xml::parse(text);
        let document = crate::model::read_doc(text, &doc, None);
        (doc, document)
    }

    #[test]
    fn setting_a_field_to_its_own_invalid_value_is_a_no_op() {
        let text = "<types>\n    <type name=\"A\">\n        <flags count_in_map=\"3\"/>\n    </type>\n</types>\n";
        let (doc, document) = read(text);
        let op = CeOp::SetField {
            section: None,
            record: RecordRef {
                key: "A".to_string(),
                occurrence: 0,
            },
            field: "count_in_map".to_string(),
            value: Some("3".to_string()),
        };
        let ops = compile_in(&doc, &document, &op).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(
            plan_all(text, None, std::slice::from_ref(&op)),
            Ok(text.to_string())
        );
    }

    #[test]
    fn setting_a_list_item_to_its_own_value_is_a_no_op() {
        let text = "<types>\n    <type name=\"A\">\n        <value name=\"Tier3\"/>\n        <value name=\"Tier3\"/>\n    </type>\n</types>\n";
        let (doc, document) = read(text);
        let op = CeOp::SetListItem {
            section: None,
            record: RecordRef {
                key: "A".to_string(),
                occurrence: 0,
            },
            list: "value".to_string(),
            index: 0,
            attr: "name".to_string(),
            value: Some("Tier3".to_string()),
        };
        let ops = compile_in(&doc, &document, &op).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(
            plan_all(text, None, std::slice::from_ref(&op)),
            Ok(text.to_string())
        );
    }

    #[test]
    fn rename_restores_a_name_that_is_not_a_class() {
        let text = "<types>\n    <type name=\"Odd,\">\n        <nominal>1</nominal>\n    </type>\n</types>\n";
        let forward = CeOp::RenameRecord {
            section: None,
            record: RecordRef {
                key: "Odd,".to_string(),
                occurrence: 0,
            },
            new_key: "__mf_probe".to_string(),
        };
        let renamed = plan_all(text, None, &[forward]).unwrap();
        let back = CeOp::RenameRecord {
            section: None,
            record: RecordRef {
                key: "__mf_probe".to_string(),
                occurrence: 0,
            },
            new_key: "Odd,".to_string(),
        };
        assert_eq!(plan_all(&renamed, None, &[back]), Ok(text.to_string()));
    }
}
