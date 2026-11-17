//! Corpus gate: the vanilla spawnable types and random presets, and the
//! workshop spawnable-types report.
//!
//! Opt-in: the gate needs `MF_DAYZ_INSTALL`, the report needs
//! `MF_WORKSHOP_PATH`; each prints `skipped:` and passes when its variable is
//! unset.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- record_files_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop, CorpusFile};
use modfoundry_ce::economy::{
    cross_findings, include_findings, Economy, EconomySource, IncludeFact, Origin,
};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::lints::{schema_findings, validate_text};
use modfoundry_ce::model::{self, Document};
use modfoundry_ce::schema;
use modfoundry_ce::xml::{self, NodeId, XmlDoc};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[test]
fn vanilla_gate() {
    let files = vanilla();
    if files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL not set");
        return;
    }

    let mut worlds: BTreeMap<String, Vec<Read>> = BTreeMap::new();
    for file in &files {
        let Some(world) = &file.world else {
            continue;
        };
        worlds.entry(world.clone()).or_default().push(read(file));
    }
    assert_eq!(
        worlds.keys().map(String::as_str).collect::<Vec<_>>(),
        VANILLA_WORLDS,
        "vanilla worlds"
    );

    for (world, reads) in &worlds {
        let sources: Vec<EconomySource<'_>> = reads
            .iter()
            .filter(|read| {
                matches!(
                    read.document.kind,
                    Some(
                        CeKind::Limits
                            | CeKind::LimitsUser
                            | CeKind::Types
                            | CeKind::RandomPresets
                            | CeKind::Events
                    )
                )
            })
            .map(|read| EconomySource {
                id: &read.rel,
                origin: Origin::Reference,
                document: &read.document,
            })
            .collect();
        let economy = Economy::build(&sources);
        assert!(economy.has_limits, "{world} has no limits document");
        assert!(economy.has_presets, "{world} has no presets document");
        assert!(economy.has_events, "{world} has no events document");

        let mut schema_codes: BTreeMap<String, usize> = BTreeMap::new();
        let mut duplicates: BTreeSet<String> = BTreeSet::new();
        let mut x210 = 0usize;
        let mut x220 = 0usize;
        let mut x220_names: Vec<String> = Vec::new();
        let mut x310 = 0usize;
        let mut type_names: Vec<String> = Vec::new();
        let mut item_names: Vec<String> = Vec::new();
        let mut out_of_order = 0usize;

        for read in reads {
            if !matches!(
                read.document.kind,
                Some(
                    CeKind::SpawnableTypes
                        | CeKind::RandomPresets
                        | CeKind::Events
                        | CeKind::EventSpawns
                )
            ) {
                continue;
            }

            for finding in schema_findings(&read.doc, &read.document) {
                if matches!(
                    finding.code.as_str(),
                    "MF-X021" | "MF-X022" | "MF-X030" | "MF-X031" | "MF-X040"
                ) {
                    *schema_codes.entry(finding.code.clone()).or_default() += 1;
                }
                if finding.code == "MF-X040" {
                    duplicates.insert(first_name(&finding.message));
                }
            }

            let keys = key_spans(&read.document);
            for finding in cross_findings(&read.document, &economy) {
                match finding.code.as_str() {
                    "MF-X210" => x210 += 1,
                    "MF-X220" => {
                        x220 += 1;
                        x220_names.push(first_name(&finding.message));
                    }
                    "MF-X310" => x310 += 1,
                    "MF-X320" => {
                        let span = finding.span.map(|span| (span.start, span.end));
                        if span.is_some_and(|span| keys.contains(&span)) {
                            type_names.push(first_name(&finding.message));
                        } else {
                            item_names.push(first_name(&finding.message));
                        }
                    }
                    _ => {}
                }
            }

            if read.document.kind == Some(CeKind::SpawnableTypes) {
                out_of_order += out_of_order_records(&read.doc, &read.document);
            }
        }

        for code in ["MF-X021", "MF-X022", "MF-X030", "MF-X031"] {
            assert_eq!(
                schema_codes.get(code).copied().unwrap_or(0),
                0,
                "{world} {code}"
            );
        }
        let mut expected = expected_duplicates(world);
        expected.sort_unstable();
        assert_eq!(
            duplicates.iter().map(String::as_str).collect::<Vec<_>>(),
            expected,
            "{world} MF-X040"
        );
        assert_eq!(x310, 0, "{world} MF-X310");
        assert_eq!(x210, 0, "{world} MF-X210");
        assert!(x220 <= 1, "{world} MF-X220 {x220}");
        assert!(
            x220_names.iter().all(|name| name == "VehicleTransitBus"),
            "{world} MF-X220 names {x220_names:?}"
        );

        type_names.sort();
        item_names.sort();
        println!(
            "{world}: MF-X040 {} {:?}; MF-X210 {x210}; MF-X220 {x220} {:?}; MF-X310 {x310}; MF-X320 {} type names {:?} {} item names {:?}; {out_of_order} records out of order",
            schema_codes.get("MF-X040").copied().unwrap_or(0),
            duplicates,
            x220_names,
            type_names.len(),
            type_names,
            item_names.len(),
            item_names,
        );
    }
}

#[test]
fn workshop_report() {
    let files = workshop();
    if files.is_empty() {
        println!("skipped: MF_WORKSHOP_PATH not set");
        return;
    }

    let mut scanned = 0usize;
    let mut codes: BTreeMap<String, usize> = BTreeMap::new();
    let mut unknown_children: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let read = read(file);
        if !matches!(
            read.document.kind,
            Some(
                CeKind::SpawnableTypes
                    | CeKind::RandomPresets
                    | CeKind::Events
                    | CeKind::EventSpawns
            )
        ) {
            continue;
        }
        scanned += 1;
        for finding in schema_findings(&read.doc, &read.document) {
            *codes.entry(finding.code.clone()).or_default() += 1;
            if finding.code == "MF-X021" {
                *unknown_children
                    .entry(first_name(&finding.message))
                    .or_default() += 1;
            }
        }
    }

    println!("workshop spawnable types, presets, events and event spawns: {scanned} files");
    for (code, count) in &codes {
        println!("  {code}  {count}");
    }
    for (name, count) in &unknown_children {
        println!("  MF-X021 unknown child {name}  {count}");
    }
}

struct Read {
    rel: String,
    text: String,
    doc: XmlDoc,
    document: Document,
}

fn read(file: &CorpusFile) -> Read {
    let doc = xml::parse(&file.text);
    let document = model::read_doc(&file.text, &doc, Some(&file.rel));
    Read {
        rel: file.rel.clone(),
        text: file.text.clone(),
        doc,
        document,
    }
}

fn first_name(message: &str) -> String {
    message.split('`').nth(1).unwrap_or("").to_string()
}

fn key_spans(document: &Document) -> BTreeSet<(usize, usize)> {
    document
        .sections
        .iter()
        .flat_map(|section| section.records.iter())
        .filter_map(|record| record.key_span)
        .map(|span| (span.start, span.end))
        .collect()
}

const VANILLA_WORLDS: [&str; 4] = ["chernarusplus", "enoch", "nasdara", "sakhal"];

fn expected_duplicates(world: &str) -> Vec<&'static str> {
    match world {
        "chernarusplus" => vec!["HuntingVest"],
        "enoch" => vec!["PM73Rak", "R12", "ZmbM_usSoldier_Heavy_Woodland"],
        "sakhal" => vec!["ScientificBriefcase", "ZmbM_Mummy"],
        _ => Vec::new(),
    }
}

fn out_of_order_records(doc: &XmlDoc, document: &Document) -> usize {
    let Some(section) = document.section("spawnabletypes") else {
        return 0;
    };
    let Some(order) = schema::schema(CeKind::SpawnableTypes)
        .and_then(|schema| schema.section("spawnabletypes"))
        .map(|section| section.record.order)
    else {
        return 0;
    };
    section
        .records
        .iter()
        .filter(|record| children_out_of_order(doc, record.node, order))
        .count()
}

fn children_out_of_order(doc: &XmlDoc, node: NodeId, order: &[&str]) -> bool {
    let mut last = 0usize;
    let mut seen = false;
    for child in doc.child_elements(node) {
        let Some(element) = doc.element(child) else {
            continue;
        };
        let Some(index) = order.iter().position(|name| element.name == *name) else {
            continue;
        };
        if seen && index < last {
            return true;
        }
        last = index;
        seen = true;
    }
    false
}

/// The record-files gate: every vanilla economy, globals, messages, ignore
/// list, limits, user limits and economy core file, with the file lints and the
/// cross lints against the world's own economy, must be clean.
#[test]
fn vanilla_record_files_gate() {
    let files = vanilla();
    if files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL not set");
        return;
    }

    let mut worlds: BTreeMap<String, Vec<Read>> = BTreeMap::new();
    for file in &files {
        let Some(world) = &file.world else {
            continue;
        };
        worlds.entry(world.clone()).or_default().push(read(file));
    }
    assert_eq!(
        worlds.keys().map(String::as_str).collect::<Vec<_>>(),
        VANILLA_WORLDS,
        "vanilla worlds"
    );

    for (world, reads) in &worlds {
        let sources: Vec<EconomySource<'_>> = reads
            .iter()
            .filter(|read| {
                matches!(
                    read.document.kind,
                    Some(
                        CeKind::Limits
                            | CeKind::LimitsUser
                            | CeKind::Types
                            | CeKind::RandomPresets
                            | CeKind::Events
                    )
                )
            })
            .map(|read| EconomySource {
                id: &read.rel,
                origin: Origin::Reference,
                document: &read.document,
            })
            .collect();
        let economy = Economy::build(&sources);

        let mut codes: BTreeMap<String, usize> = BTreeMap::new();
        let mut checked = 0usize;
        for read in reads {
            if !matches!(
                read.document.kind,
                Some(
                    CeKind::Economy
                        | CeKind::Globals
                        | CeKind::Messages
                        | CeKind::IgnoreList
                        | CeKind::Limits
                        | CeKind::LimitsUser
                        | CeKind::EconomyCore
                )
            ) {
                continue;
            }
            checked += 1;
            for finding in validate_text(&read.text, Some(&read.rel)) {
                *codes.entry(finding.code.clone()).or_default() += 1;
            }
            for finding in cross_findings(&read.document, &economy) {
                *codes.entry(finding.code.clone()).or_default() += 1;
            }
        }

        println!(
            "{world}: gate over {checked} files, {} findings {codes:?}",
            codes.values().sum::<usize>()
        );
        assert!(codes.is_empty(), "{world} findings {codes:?}");
    }
}

/// The workshop report: the economy core files and their include lints,
/// per code.
#[test]
fn workshop_economy_core_report() {
    let files = workshop();
    if files.is_empty() {
        println!("skipped: MF_WORKSHOP_PATH not set");
        return;
    }
    let root = std::env::var_os("MF_WORKSHOP_PATH").map(PathBuf::from);

    let mut scanned = 0usize;
    let mut codes: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let document = model::read(&file.text, Some(&file.rel));
        if document.kind != Some(CeKind::EconomyCore) {
            continue;
        }
        scanned += 1;
        for finding in validate_text(&file.text, Some(&file.rel)) {
            *codes.entry(finding.code.clone()).or_default() += 1;
        }
        if let Some(path) = root
            .as_deref()
            .and_then(|root| workshop_path(root, &file.label))
        {
            for finding in include_findings(&document, &include_facts(&path, &document)) {
                *codes.entry(finding.code.clone()).or_default() += 1;
            }
        }
    }

    println!("workshop economy core files: {scanned}");
    for (code, count) in &codes {
        println!("  {code}  {count}");
    }
}

fn workshop_path(root: &Path, label: &str) -> Option<PathBuf> {
    let rest = label.strip_prefix("workshop:")?;
    let (id, rel) = rest.split_once(':')?;
    Some(root.join(id).join(rel))
}

fn include_facts(path: &Path, document: &Document) -> Vec<IncludeFact> {
    let mut facts = Vec::new();
    let Some(section) = document.section("ce") else {
        return facts;
    };
    for record in &section.records {
        let Some(list) = record.list("file") else {
            continue;
        };
        let folder_exists = path
            .parent()
            .is_some_and(|base| base.join(&record.key).is_dir());
        for item in &list.items {
            let name = item
                .attrs
                .iter()
                .find(|attr| attr.name == "name")
                .map(|attr| attr.value.clone())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let resolved = path.parent().map(|base| base.join(&record.key).join(&name));
            let (exists, kind) = match &resolved {
                Some(resolved) if resolved.is_file() => {
                    let text = std::fs::read_to_string(resolved).unwrap_or_default();
                    (
                        true,
                        model::read(&text, resolved.file_name().and_then(|n| n.to_str())).kind,
                    )
                }
                _ => (false, None),
            };
            facts.push(IncludeFact {
                folder: record.key.clone(),
                name,
                folder_exists,
                exists,
                kind,
            });
        }
    }
    facts
}
