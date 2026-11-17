//! Bodies for the `ce.*` commands: walking, reading and editing Central Economy
//! XML sources through the pure crate.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use modfoundry_ce::classes::class_findings;
use modfoundry_ce::{lints, model, xml};
use modfoundry_core::Diagnostic;
use serde_json::{json, Value};

use crate::ce_classes::ConfigLookup;
use crate::economy_host::EconomyHost;
use crate::registry::FindingReport;
use crate::sources::has_ext;
use crate::walk::walk_files;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Every `.xml` file under `root`, deterministically ordered.
pub fn walk_ce_files(root: &Path) -> Vec<PathBuf> {
    walk_files(root, true, |entry, name| {
        has_ext(Path::new(name), "xml") && within_size_cap(entry)
    })
}

fn within_size_cap(entry: &std::fs::DirEntry) -> bool {
    entry
        .metadata()
        .is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES)
}

/// The `ce.get_tree` body: the record model and XML parse issues of one file.
pub fn ce_tree(path: &Path, text: Option<&str>) -> anyhow::Result<Value> {
    let text = read_text(path, text)?;
    let doc = xml::parse(&text);
    let document = model::read_doc(&text, &doc, file_name(path));
    let kind = document.kind;
    Ok(json!({
        "kind": kind,
        "label": kind.map(|kind| kind.label()),
        "structured": kind.is_some_and(|kind| kind.structured()),
        "fragment": document.fragment,
        "root_mismatch": document.root_mismatch,
        "sections": document.sections,
        "diagnostics": doc.issues,
    }))
}

/// The `ce.validate` body: the lints of one CE file, or of every CE file under
/// a directory; non-CE XML in a directory is skipped. When the host carries an
/// economy index, each file's cross-file findings follow its own. With `classes`,
/// the class findings of the types files come after all the others.
pub(crate) fn ce_validate(
    path: &Path,
    text: Option<&str>,
    host: &EconomyHost,
    classes: Option<&ConfigLookup>,
) -> anyhow::Result<Vec<FindingReport>> {
    if let Some(text) = text {
        return Ok(findings(path, text, host, classes));
    }
    if path.is_dir() {
        let mut out = Vec::new();
        let mut class_out = Vec::new();
        for file in walk_ce_files(path) {
            let Ok(text) = read_text(&file, None) else {
                continue;
            };
            let document = model::read(&text, file_name(&file));
            if document.kind.is_some() {
                out.extend(findings_of(&file, &text, host, &document));
                class_out.extend(class_findings_of(&file, &document, classes));
            }
        }
        out.extend(class_out);
        return Ok(out);
    }
    let text = read_text(path, None)?;
    Ok(findings(path, &text, host, classes))
}

fn findings(
    path: &Path,
    text: &str,
    host: &EconomyHost,
    classes: Option<&ConfigLookup>,
) -> Vec<FindingReport> {
    let document = model::read(text, file_name(path));
    let mut out = findings_of(path, text, host, &document);
    out.extend(class_findings_of(path, &document, classes));
    out
}

fn class_findings_of(
    path: &Path,
    document: &model::Document,
    classes: Option<&ConfigLookup>,
) -> Vec<FindingReport> {
    classes.map_or_else(Vec::new, |lookup| {
        reports(path, class_findings(document, lookup)).collect()
    })
}

fn findings_of(
    path: &Path,
    text: &str,
    host: &EconomyHost,
    document: &model::Document,
) -> Vec<FindingReport> {
    let mut out: Vec<FindingReport> =
        reports(path, lints::validate_text(text, file_name(path))).collect();
    if host.loaded().is_some() {
        out.extend(reports(path, host.cross(path, document)));
    }
    out
}

pub(crate) fn reports(
    path: &Path,
    diagnostics: Vec<Diagnostic>,
) -> impl Iterator<Item = FindingReport> + '_ {
    diagnostics.into_iter().map(|diagnostic| FindingReport {
        diagnostic,
        path: path.to_path_buf(),
    })
}

fn read_text<'a>(path: &Path, text: Option<&'a str>) -> anyhow::Result<Cow<'a, str>> {
    match text {
        Some(text) => Ok(Cow::Borrowed(text)),
        None => {
            let bytes =
                std::fs::read(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            Ok(Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()))
        }
    }
}

pub(crate) fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// The `ce.edit` ops schema: every `modfoundry_ce::ops::CeOp` variant with its
/// fields.
pub(crate) fn ce_edit_op_schema() -> Value {
    let record = record_ref_schema();
    let position = record_position_schema();
    let list = new_list_schema();
    let section = json!({ "type": "string", "description": "the section id; omit when the kind has a single section" });
    let value = |what: &str| json!({ "type": ["string", "null"], "description": what });
    let op = |tag: &str, body: Value| json!({ "type": "object", "properties": { tag: body }, "required": [tag] });
    json!({
        "description": "one record-level edit; the key names the op and holds its arguments",
        "oneOf": [
            op("set_field", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "field": { "type": "string", "description": "the field id" },
                    "value": value("the new text; null removes the field"),
                },
                "required": ["record", "field", "value"],
            })),
            op("add_record", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "key": { "type": "string", "description": "the new record's key" },
                    "element": { "type": "string", "description": "the element name when the section has more than one" },
                    "fields": pairs_schema("field id/value pairs, e.g. [[\"nominal\", \"12\"]]"),
                    "lists": { "type": "array", "items": list.clone() },
                    "position": position.clone(),
                },
                "required": ["key"],
            })),
            op("remove_record", json!({
                "type": "object",
                "properties": { "section": section.clone(), "record": record.clone() },
                "required": ["record"],
            })),
            op("rename_record", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "new_key": { "type": "string" },
                },
                "required": ["record", "new_key"],
            })),
            op("duplicate_record", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "new_key": { "type": "string" },
                },
                "required": ["record", "new_key"],
            })),
            op("add_list_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "attrs": pairs_schema("attribute name/value pairs in written order"),
                    "index": { "type": "integer", "minimum": 0, "description": "insert before this item; omit to append" },
                },
                "required": ["record", "list", "attrs"],
            })),
            op("remove_list_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "index": { "type": "integer", "minimum": 0 },
                },
                "required": ["record", "list", "index"],
            })),
            op("set_list_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "index": { "type": "integer", "minimum": 0 },
                    "attr": { "type": "string" },
                    "value": value("the new text; null removes the attribute"),
                },
                "required": ["record", "list", "index", "attr", "value"],
            })),
            op("add_nested_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "index": { "type": "integer", "minimum": 0 },
                    "attrs": pairs_schema("attribute name/value pairs in written order"),
                    "nested_index": { "type": "integer", "minimum": 0, "description": "insert before this nested item; omit to append" },
                },
                "required": ["record", "list", "index", "attrs"],
            })),
            op("remove_nested_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "index": { "type": "integer", "minimum": 0 },
                    "nested_index": { "type": "integer", "minimum": 0 },
                },
                "required": ["record", "list", "index", "nested_index"],
            })),
            op("set_nested_item", json!({
                "type": "object",
                "properties": {
                    "section": section.clone(),
                    "record": record.clone(),
                    "list": { "type": "string", "description": "the list id" },
                    "index": { "type": "integer", "minimum": 0 },
                    "nested_index": { "type": "integer", "minimum": 0 },
                    "attr": { "type": "string" },
                    "value": value("the new text; null removes the attribute"),
                },
                "required": ["record", "list", "index", "nested_index", "attr", "value"],
            })),
            op("wrap_root", json!({ "type": "null" })),
            op("xml", xml_op_schema()),
        ],
    })
}

/// A record pick: `{"key": "FieldCanteen", "occurrence": 0}`.
fn record_ref_schema() -> Value {
    json!({
        "type": "object",
        "description": "names one record by key and, for duplicates, occurrence",
        "properties": {
            "key": { "type": "string", "description": "the record's key" },
            "occurrence": { "type": "integer", "minimum": 0, "description": "0-based among records with the same key; default 0" },
        },
        "required": ["key"],
    })
}

fn record_position_schema() -> Value {
    json!({
        "description": "where among its section's records a new record lands",
        "oneOf": [
            { "type": "string", "enum": ["first", "last", "sorted"] },
            { "type": "object", "properties": { "after": record_ref_schema() }, "required": ["after"] },
        ],
    })
}

fn pairs_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 },
    })
}

fn new_item_schema(depth: u8) -> Value {
    let nested = if depth == 0 {
        json!({ "type": "array", "items": { "type": "object" } })
    } else {
        json!({ "type": "array", "items": new_item_schema(depth - 1) })
    };
    json!({
        "type": "object",
        "description": "a new list item, with attributes in written order",
        "properties": {
            "attrs": pairs_schema("attribute name/value pairs in written order"),
            "nested": nested,
        },
        "required": ["attrs"],
    })
}

fn new_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "list": { "type": "string", "description": "the list id" },
            "items": { "type": "array", "items": new_item_schema(3) },
        },
        "required": ["list", "items"],
    })
}

fn xml_op_schema() -> Value {
    let op = |tag: &str, body: Value| json!({ "type": "object", "properties": { tag: body }, "required": [tag] });
    let path = json!({
        "type": "array",
        "description": "steps from the document's top level; empty means the top level itself",
        "items": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "pick": {
                    "oneOf": [
                        { "type": "object", "properties": { "nth": { "type": "integer", "minimum": 0 } }, "required": ["nth"] },
                        {
                            "type": "object",
                            "properties": { "attr": { "type": "object", "properties": { "name": { "type": "string" }, "value": { "type": "string" } }, "required": ["name", "value"] } },
                            "required": ["attr"],
                        },
                    ]
                },
            },
            "required": ["name", "pick"],
        },
    });
    let element = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "attrs": pairs_schema("attribute name/value pairs in written order"),
            "text": { "type": ["string", "null"] },
            "children": { "type": "array", "items": { "type": "object" } },
        },
        "required": ["name", "attrs"],
    });
    json!({
        "description": "one XML-level edit, for structure the record ops don't cover",
        "oneOf": [
            op("set_attr", json!({ "type": "object", "properties": { "element": path.clone(), "name": { "type": "string" }, "value": { "type": "string" } }, "required": ["element", "name", "value"] })),
            op("remove_attr", json!({ "type": "object", "properties": { "element": path.clone(), "name": { "type": "string" } }, "required": ["element", "name"] })),
            op("set_text", json!({ "type": "object", "properties": { "element": path.clone(), "text": { "type": "string" } }, "required": ["element", "text"] })),
            op("rename_element", json!({ "type": "object", "properties": { "element": path.clone(), "name": { "type": "string" } }, "required": ["element", "name"] })),
            op("insert_element", json!({
                "type": "object",
                "properties": {
                    "parent": path.clone(),
                    "at": { "oneOf": [ { "type": "string", "enum": ["first", "last"] }, { "type": "object", "properties": { "index": { "type": "integer", "minimum": 0 } }, "required": ["index"] } ] },
                    "element": element,
                },
                "required": ["parent", "at", "element"],
            })),
            op("remove_element", json!({ "type": "object", "properties": { "element": path.clone() }, "required": ["element"] })),
            op("wrap_top", json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] })),
        ],
    })
}
