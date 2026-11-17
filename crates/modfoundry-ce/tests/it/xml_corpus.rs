//! Corpus gate: parse losslessness over the measured corpus,
//! plus the vanilla "zero issues" gate. Opt-in: skipped unless both env vars
//! are set.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- xml_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop, CorpusFile};
use modfoundry_ce::xml::{decode, parse, NodeKind, XmlDoc};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const MALFORMED: [&str; 2] = ["MF-X001", "MF-X002"];
const TOLERATED: &str = "MF-X010";

#[derive(Default)]
struct Totals {
    files: u64,
    elements: u64,
    attrs: u64,
    bytes: u64,
}

#[test]
fn xml_corpus_gate() {
    let mut totals = Totals::default();
    let vanilla = vanilla();
    let mut vanilla_issues = Vec::new();
    let mut largest: Option<(String, usize, Duration)> = None;
    for file in &vanilla {
        let (doc, elapsed) = check(file, &mut totals);
        if !doc.issues.is_empty() {
            vanilla_issues.push(format!("  {} [{}]", file.label, codes(&doc)));
        }
        keep_largest(&mut largest, file, elapsed);
    }

    let workshop = workshop();
    let mut malformed = Vec::new();
    let mut tolerated = Vec::new();
    for file in &workshop {
        let (doc, elapsed) = check(file, &mut totals);
        let has = |code: &str| doc.issues.iter().any(|d| d.code == code);
        if MALFORMED.iter().any(|code| has(code)) {
            malformed.push(format!("  {} [{}]", file.label, codes(&doc)));
        }
        if has(TOLERATED) {
            tolerated.push(format!("  {}", file.label));
        }
        keep_largest(&mut largest, file, elapsed);
    }

    println!(
        "corpus: {} files, {} elements, {} attributes, {} bytes",
        totals.files, totals.elements, totals.attrs, totals.bytes
    );
    println!("vanilla: {} files", vanilla.len());
    println!("workshop: {} files", workshop.len());
    println!("workshop: {} files with MF-X001/MF-X002", malformed.len());
    for line in &malformed {
        println!("{line}");
    }
    println!("workshop: {} files with {TOLERATED}", tolerated.len());
    for line in &tolerated {
        println!("{line}");
    }
    if let Some((label, len, elapsed)) = &largest {
        println!("largest file: {label} ({len} bytes) parsed in {elapsed:?}");
    }

    assert!(
        vanilla_issues.is_empty(),
        "vanilla files with issues (a parser bug — do not relax the gate):\n{}",
        vanilla_issues.join("\n")
    );
}

fn check(file: &CorpusFile, totals: &mut Totals) -> (XmlDoc, Duration) {
    let start = Instant::now();
    let doc = parse(&file.text);
    let elapsed = start.elapsed();
    assert_eq!(
        doc.rebuild(&file.text),
        file.text,
        "rebuild mismatch in {}",
        file.label
    );
    for node in &doc.nodes {
        let NodeKind::Element(element) = &node.kind else {
            continue;
        };
        totals.elements += 1;
        assert_eq!(
            &file.text[element.name_span.start..element.name_span.end],
            element.name,
            "name_span mismatch in {}",
            file.label
        );
        for attr in &element.attrs {
            totals.attrs += 1;
            let raw = &file.text[attr.value_span.start..attr.value_span.end];
            assert_eq!(
                decode(raw),
                attr.value,
                "value_span mismatch for `{}` in {}",
                attr.name,
                file.label
            );
        }
    }
    totals.files += 1;
    totals.bytes += file.text.len() as u64;
    (doc, elapsed)
}

fn codes(doc: &XmlDoc) -> String {
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for issue in &doc.issues {
        *counts.entry(issue.code.as_str()).or_default() += 1;
    }
    counts
        .iter()
        .map(|(code, n)| {
            if *n > 1 {
                format!("{code}x{n}")
            } else {
                (*code).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn keep_largest(
    slot: &mut Option<(String, usize, Duration)>,
    file: &CorpusFile,
    elapsed: Duration,
) {
    let len = file.text.len();
    if slot.as_ref().is_none_or(|(_, best, _)| len > *best) {
        *slot = Some((file.label.clone(), len, elapsed));
    }
}
