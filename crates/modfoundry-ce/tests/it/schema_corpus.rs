//! Corpus gate: the schema tables against every vanilla file of the six
//! structured kinds. Walks the raw XML, not
//! the record model, so it checks the tables themselves.
//!
//! Opt-in: skipped unless `MF_DAYZ_INSTALL` is set.
//!
//! ```text
//! MF_DAYZ_INSTALL=... cargo test -p modfoundry-ce --release --test it -- schema_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, CorpusFile};
use modfoundry_ce::kind::{detect, CeKind};
use modfoundry_ce::schema::{schema, Key, ListSchema, Place, RecordSchema, SectionSchema};
use modfoundry_ce::xml::{parse, NodeId, XmlDoc};

#[derive(Default)]
struct Gate {
    files: Vec<(CeKind, usize)>,
    records: Vec<(CeKind, usize)>,
    elements: Vec<String>,
    attrs: Vec<String>,
    values: Vec<String>,
    order: Vec<String>,
}

#[test]
fn schema_corpus_gate() {
    let files = vanilla();
    if files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL not set");
        return;
    }

    let mut gate = Gate::default();
    for file in &files {
        let doc = parse(&file.text);
        let detected = detect(Some(&file.rel), &doc);
        let Some(kind) = detected.kind else {
            continue;
        };
        let Some(kind_schema) = schema(kind) else {
            continue;
        };
        count(&mut gate.files, kind);
        for section in kind_schema.sections {
            for parent in record_parents(&doc, detected.fragment, section) {
                for record in doc.children_named(parent, section.record.elements[0]) {
                    walk_record(file, &doc, record, &section.record, &mut gate);
                    count(&mut gate.records, kind);
                }
            }
        }
    }

    println!("schema corpus gate over {} files", files.len());
    for kind in [
        CeKind::Types,
        CeKind::SpawnableTypes,
        CeKind::RandomPresets,
        CeKind::Limits,
        CeKind::LimitsUser,
        CeKind::EconomyCore,
    ] {
        let files = count_of(&gate.files, kind);
        let records = count_of(&gate.records, kind);
        println!("  {files:>3} files {records:>6} records  {}", kind.label());
    }
    println!("unknown elements: {}", gate.elements.len());
    println!("unknown attributes: {}", gate.attrs.len());
    println!("failed values: {}", gate.values.len());
    println!(
        "records whose child order is not a subsequence: {}",
        gate.order.len()
    );
    for line in gate
        .elements
        .iter()
        .chain(&gate.attrs)
        .chain(&gate.values)
        .chain(&gate.order)
        .take(40)
    {
        println!("  {line}");
    }

    assert!(
        gate.elements.is_empty(),
        "{} unknown element(s)",
        gate.elements.len()
    );
    assert!(
        gate.attrs.is_empty(),
        "{} unknown attribute(s)",
        gate.attrs.len()
    );
    assert!(
        gate.values.is_empty(),
        "{} failed value(s)",
        gate.values.len()
    );
    assert!(
        gate.order.is_empty(),
        "{} record(s) with a child order outside `order`",
        gate.order.len()
    );
}

fn count(counts: &mut Vec<(CeKind, usize)>, kind: CeKind) {
    match counts.iter_mut().find(|(k, _)| *k == kind) {
        Some((_, n)) => *n += 1,
        None => counts.push((kind, 1)),
    }
}

fn count_of(counts: &[(CeKind, usize)], kind: CeKind) -> usize {
    counts
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(0, |(_, n)| *n)
}

fn record_parents(doc: &XmlDoc, fragment: bool, section: &SectionSchema) -> Vec<NodeId> {
    if fragment {
        return doc.top_elements();
    }
    let Some(root) = doc.root() else {
        return Vec::new();
    };
    match section.container {
        Some(container) => doc.children_named(root, container),
        None => vec![root],
    }
}

fn walk_record(
    file: &CorpusFile,
    doc: &XmlDoc,
    id: NodeId,
    record: &RecordSchema,
    gate: &mut Gate,
) {
    let label = file.label.as_str();
    let element = record.elements[0];
    let allowed = allowed_record_attrs(record);

    for attr in &doc.element(id).unwrap().attrs {
        if !allowed.contains(&attr.name.as_str()) {
            gate.attrs
                .push(format!("{label} <{element}> attribute {:?}", attr.name));
        }
    }

    let mut children = Vec::new();
    for child in doc.child_elements(id) {
        let name = doc.element(child).unwrap().name.clone();
        children.push(name.clone());
        if !record.known_children().contains(&name.as_str()) {
            gate.elements
                .push(format!("{label} <{element}> child <{name}>"));
            continue;
        }
        let allowed_child: Vec<&str> = record
            .fields
            .iter()
            .filter_map(|field| match field.place {
                Place::ChildAttr { child, attr } if child == name.as_str() => Some(attr),
                _ => None,
            })
            .collect();
        if !allowed_child.is_empty() {
            for attr in &doc.element(child).unwrap().attrs {
                if !allowed_child.contains(&attr.name.as_str()) {
                    gate.attrs
                        .push(format!("{label} <{name}> attribute {:?}", attr.name));
                }
            }
        }
    }

    if !in_order(&children, record.order) {
        gate.order.push(format!(
            "{label} <{element}> children {children:?} are not a subsequence of {:?}",
            record.order
        ));
    }

    for field in record.fields {
        let raw = match field.place {
            Place::ChildText(child) => doc
                .children_named(id, child)
                .first()
                .map(|&node| doc.text_of(node)),
            Place::ChildAttr { child, attr } => doc
                .children_named(id, child)
                .first()
                .and_then(|&node| doc.attr(node, attr).map(|a| a.value.clone())),
            Place::Attr(attr) => doc.attr(id, attr).map(|a| a.value.clone()),
            Place::ChildPresent(child) => doc
                .children_named(id, child)
                .first()
                .map(|_| "1".to_string()),
        };
        if let Some(raw) = raw {
            if let Err(problem) = field.ty.check(&raw) {
                gate.values.push(format!(
                    "{label} <{element}> `{}` = {raw:?}: {problem:?}",
                    field.id
                ));
            }
        }
    }

    for list in record.lists {
        walk_list(doc, id, list, label, gate);
    }
}

fn walk_list(doc: &XmlDoc, parent: NodeId, list: &ListSchema, label: &str, gate: &mut Gate) {
    let parents = match list.container {
        Some(container) => doc.children_named(parent, container),
        None => vec![parent],
    };
    for item_parent in parents {
        for item in doc.children_named(item_parent, list.element) {
            for attr in &doc.element(item).unwrap().attrs {
                if !list.attrs.iter().any(|known| known.id == attr.name) {
                    gate.attrs.push(format!(
                        "{label} <{}> attribute {:?}",
                        list.element, attr.name
                    ));
                }
            }
            if let Some(nested) = list.nested {
                walk_list(doc, item, nested, label, gate);
            }
            for inner in list.inner {
                walk_list(doc, item, inner, label, gate);
            }
        }
    }
}

fn allowed_record_attrs(record: &RecordSchema) -> Vec<&'static str> {
    let mut allowed = Vec::new();
    if let Key::Attr(attr) = record.key {
        allowed.push(attr);
    }
    for field in record.fields {
        if let Place::Attr(attr) = field.place {
            allowed.push(attr);
        }
    }
    allowed
}

fn in_order(sequence: &[String], order: &[&str]) -> bool {
    // `tag`, `usage` and `value` are independent lists, so they rank together:
    // vanilla enoch's SCARH writes `value` before `usage` and must still pass.
    // `cargo` and `attachments` are independent repeated lists too: vanilla
    // writes them grouped but in either order.
    let list_rank = order.iter().position(|candidate| *candidate == "tag");
    let cargo_rank = order.iter().position(|candidate| *candidate == "cargo");
    let rank = |name: &str| -> Option<usize> {
        let at = order.iter().position(|candidate| *candidate == name)?;
        if matches!(name, "tag" | "usage" | "value") {
            list_rank
        } else if matches!(name, "cargo" | "attachments") {
            cargo_rank
        } else {
            Some(at)
        }
    };
    let mut last = None;
    for name in sequence {
        let Some(position) = rank(name) else {
            continue;
        };
        if last.is_some_and(|previous| position < previous) {
            return false;
        }
        last = Some(position);
    }
    true
}
