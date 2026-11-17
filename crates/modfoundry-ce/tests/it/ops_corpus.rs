//! Corpus gate: on every real types file, an edit that changes nothing
//! leaves every byte alone and an edit that is undone restores the text.
//!
//! Opt-in: skipped unless `MF_DAYZ_INSTALL` / `MF_WORKSHOP_PATH` are set.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- ops_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop};
use modfoundry_ce::edit::{self, TextEdit, XmlOp};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::model::{self, Section};
use modfoundry_ce::ops::{self, CeOp, RecordPosition, RecordRef};
use modfoundry_ce::schema;
use modfoundry_ce::xml::{self, XmlDoc};
use std::collections::HashMap;
use std::time::Instant;

/// The key every probe record is renamed to or added under.
const PROBE: &str = "__mf_probe";
/// Renames whose reverse is applied, per file (the reverse re-parses).
const REVERSE_LIMIT: usize = 20;

#[test]
fn ops_corpus_gate() {
    let started = Instant::now();
    let mut files = 0usize;
    let mut records = 0usize;
    let mut fields = 0usize;
    let mut items = 0usize;
    let mut skipped = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    let corpus = vanilla().into_iter().chain(workshop());
    for file in corpus {
        let doc = xml::parse(&file.text);
        if doc.blocking() {
            continue;
        }
        let document = model::read_doc(&file.text, &doc, Some(&file.rel));
        if document.kind != Some(CeKind::Types) {
            continue;
        }
        files += 1;

        let section = document.section("types").expect("types has a section");
        let kind_schema = schema::schema(CeKind::Types).expect("types has a schema");
        let record_schema = &kind_schema.sections[0].record;

        let mut counts: HashMap<String, usize> = HashMap::new();
        for record in &section.records {
            *counts.entry(record.key.to_ascii_lowercase()).or_default() += 1;
        }

        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut renames = 0usize;
        let mut nudged = false;
        for record in &section.records {
            records += 1;
            let lower = record.key.to_ascii_lowercase();
            let occurrence = seen.get(&lower).copied().unwrap_or(0);
            *seen.entry(lower.clone()).or_default() += 1;
            let reference = RecordRef {
                key: record.key.clone(),
                occurrence,
            };
            let unique = counts[&lower] == 1;

            for field in &record.fields {
                let Some(raw) = field.raw.clone() else {
                    continue;
                };
                fields += 1;
                let op = CeOp::SetField {
                    section: None,
                    record: reference.clone(),
                    field: field.id.clone(),
                    value: Some(raw),
                };
                match ops::compile_in(&doc, &document, &op) {
                    Ok(xml_ops) => {
                        if let Some(problem) = no_op_problem(&file.text, &doc, &xml_ops) {
                            mismatches.push(format!(
                                "{}: record `{}` field `{}`: {problem}",
                                file.label, record.key, field.id
                            ));
                        }
                    }
                    Err(problem) => mismatches.push(format!(
                        "{}: record `{}` field `{}`: compile_in: {problem}",
                        file.label, record.key, field.id
                    )),
                }
            }

            for list in &record.lists {
                let Some(list_schema) = record_schema.list(&list.id) else {
                    continue;
                };
                if !list_schema.chips {
                    continue;
                }
                for (index, item) in list.items.iter().enumerate() {
                    let Some(attr) = item
                        .attrs
                        .iter()
                        .find(|attr| attr.name == "name")
                        .or_else(|| item.attrs.iter().find(|attr| attr.name == "user"))
                    else {
                        continue;
                    };
                    items += 1;
                    let op = CeOp::SetListItem {
                        section: None,
                        record: reference.clone(),
                        list: list.id.clone(),
                        index,
                        attr: attr.name.clone(),
                        value: Some(attr.value.clone()),
                    };
                    match ops::compile_in(&doc, &document, &op) {
                        Ok(xml_ops) => {
                            if let Some(problem) = no_op_problem(&file.text, &doc, &xml_ops) {
                                mismatches.push(format!(
                                    "{}: record `{}` list `{}` item {index}: {problem}",
                                    file.label, record.key, list.id
                                ));
                            }
                        }
                        Err(problem) => mismatches.push(format!(
                            "{}: record `{}` list `{}` item {index}: compile_in: {problem}",
                            file.label, record.key, list.id
                        )),
                    }
                }
            }

            if unique && record.key != PROBE {
                let op = CeOp::RenameRecord {
                    section: None,
                    record: reference.clone(),
                    new_key: PROBE.to_string(),
                };
                match ops::compile_in(&doc, &document, &op) {
                    Ok(xml_ops) => {
                        let edits = edits_of(&file.text, &doc, &xml_ops);
                        match edits {
                            Err(problem) => mismatches.push(format!(
                                "{}: rename `{}`: {problem}",
                                file.label, record.key
                            )),
                            Ok(edits) => match (record.key_span, edits.as_slice()) {
                                (Some(span), [one]) => {
                                    if one.offset != span.start
                                        || one.offset + one.len != span.end
                                        || one.text != PROBE
                                    {
                                        mismatches.push(format!(
                                            "{}: rename `{}`: edit [{}, {}] {:?} != key [{}, {}]",
                                            file.label,
                                            record.key,
                                            one.offset,
                                            one.offset + one.len,
                                            one.text,
                                            span.start,
                                            span.end
                                        ));
                                    } else {
                                        renames += 1;
                                        if renames <= REVERSE_LIMIT {
                                            check_rename_round_trip(
                                                &file,
                                                &xml_ops,
                                                &record.key,
                                                &mut mismatches,
                                            );
                                        }
                                    }
                                }
                                (None, _) => {}
                                (_, edits) => mismatches.push(format!(
                                    "{}: rename `{}`: {} edits, expected 1",
                                    file.label,
                                    record.key,
                                    edits.len()
                                )),
                            },
                        }
                    }
                    Err(problem) => mismatches.push(format!(
                        "{}: rename `{}`: compile_in: {problem}",
                        file.label, record.key
                    )),
                }
            }

            if !nudged {
                if let Some(raw) = record.field("nominal").and_then(|field| field.raw.clone()) {
                    if let Ok(value) = raw.parse::<i64>() {
                        let bumped = value.wrapping_add(1).to_string();
                        let ops_seq = [
                            CeOp::SetField {
                                section: None,
                                record: reference.clone(),
                                field: "nominal".to_string(),
                                value: Some(bumped),
                            },
                            CeOp::SetField {
                                section: None,
                                record: reference.clone(),
                                field: "nominal".to_string(),
                                value: Some(raw),
                            },
                        ];
                        match ops::plan_all(&file.text, Some(&file.rel), &ops_seq) {
                            Ok(result) if result == file.text => {}
                            Ok(_) => mismatches.push(format!(
                                "{}: nominal nudge on `{}` did not restore the text",
                                file.label, record.key
                            )),
                            Err(problem) => mismatches.push(format!(
                                "{}: nominal nudge on `{}`: {problem}",
                                file.label, record.key
                            )),
                        }
                        nudged = true;
                    }
                }
            }
        }

        if records_start_lines(&file.text, section) {
            let ops_seq = [
                CeOp::AddRecord {
                    section: None,
                    key: PROBE.to_string(),
                    element: None,
                    fields: Vec::new(),
                    lists: Vec::new(),
                    position: RecordPosition::Last,
                },
                CeOp::RemoveRecord {
                    section: None,
                    record: RecordRef {
                        key: PROBE.to_string(),
                        occurrence: 0,
                    },
                },
            ];
            match ops::plan_all(&file.text, Some(&file.rel), &ops_seq) {
                Ok(result) if result == file.text => {}
                Ok(result) => mismatches.push(format!(
                    "{}: add/remove did not restore the text: {}",
                    file.label,
                    diff(&file.text, &result)
                )),
                Err(problem) => {
                    mismatches.push(format!("{}: add/remove: {problem}", file.label));
                }
            }
        } else {
            skipped += 1;
        }
    }

    let seconds = started.elapsed().as_secs_f64();
    println!(
        "ops corpus: {files} files, {records} records, {fields} fields, {items} list items, \
         {skipped} skipped, {seconds:.1}s"
    );
    println!("ops corpus: {} mismatch(es)", mismatches.len());
    for mismatch in &mismatches {
        println!("  {mismatch}");
    }
    assert!(
        mismatches.is_empty(),
        "{} no-op/round-trip mismatch(es) over the corpus (an engine bug — do not relax the gate)",
        mismatches.len()
    );
}

fn no_op_problem(text: &str, doc: &XmlDoc, xml_ops: &[XmlOp]) -> Option<String> {
    let edits = match edits_of(text, doc, xml_ops) {
        Ok(edits) => edits,
        Err(problem) => return Some(problem),
    };
    for edit in edits {
        if !no_op(text, &edit) {
            let got = text
                .get(edit.offset..edit.offset + edit.len)
                .unwrap_or("<out of range>");
            return Some(format!(
                "no-op edit [{}, {}] is {:?}, not {:?}",
                edit.offset,
                edit.offset + edit.len,
                got,
                edit.text
            ));
        }
    }
    None
}

fn edits_of(text: &str, doc: &XmlDoc, xml_ops: &[XmlOp]) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::new();
    for op in xml_ops {
        match edit::plan_in(text, doc, op) {
            Ok(mut planned) => edits.append(&mut planned),
            Err(problem) => return Err(format!("plan_in: {problem}")),
        }
    }
    Ok(edits)
}

fn no_op(text: &str, edit: &TextEdit) -> bool {
    text.get(edit.offset..edit.offset + edit.len)
        .is_some_and(|slice| slice == edit.text)
}

fn check_rename_round_trip(
    file: &common::corpus::CorpusFile,
    xml_ops: &[XmlOp],
    key: &str,
    mismatches: &mut Vec<String>,
) {
    let renamed = match edit::plan_all(&file.text, xml_ops) {
        Ok(renamed) => renamed,
        Err(problem) => {
            mismatches.push(format!("{}: rename `{key}` apply: {problem}", file.label));
            return;
        }
    };
    let op = CeOp::RenameRecord {
        section: None,
        record: RecordRef {
            key: PROBE.to_string(),
            occurrence: 0,
        },
        new_key: key.to_string(),
    };
    match ops::plan_all(&renamed, Some(&file.rel), &[op]) {
        Ok(restored) if restored == file.text => {}
        Ok(restored) => mismatches.push(format!(
            "{}: rename `{key}` round trip did not restore the text: {}",
            file.label,
            diff(&file.text, &restored)
        )),
        Err(problem) => {
            mismatches.push(format!("{}: rename `{key}` reverse: {problem}", file.label))
        }
    }
}

fn diff(before: &str, after: &str) -> String {
    let at = before.bytes().zip(after.bytes()).position(|(a, b)| a != b);
    match at {
        None => format!("length {} vs {}", before.len(), after.len()),
        Some(at) => {
            let lo = at.saturating_sub(24);
            let hi = (at + 24).min(before.len());
            let hi_after = (at + 24).min(after.len());
            format!(
                "first diff at byte {at}: before {:?} after {:?}",
                String::from_utf8_lossy(&before.as_bytes()[lo..hi]),
                String::from_utf8_lossy(&after.as_bytes()[lo..hi_after])
            )
        }
    }
}

fn records_start_lines(text: &str, section: &Section) -> bool {
    section.records.iter().all(|record| {
        let start = text[..record.span.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        text[start..record.span.start]
            .bytes()
            .all(|byte| byte == b' ' || byte == b'\t')
    })
}
