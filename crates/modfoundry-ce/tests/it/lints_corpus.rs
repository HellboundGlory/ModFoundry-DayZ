//! Corpus gate: the CE lint set over every vanilla CE file, and the
//! workshop report.
//!
//! Opt-in: vanilla needs `MF_DAYZ_INSTALL`, the report needs `MF_WORKSHOP_PATH`;
//! each prints `skipped:` and passes when its variable is unset.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- lints_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop};
use modfoundry_ce::kind::{detect, CeKind};
use modfoundry_ce::lints::validate_text;
use modfoundry_ce::xml::parse;
use modfoundry_ce::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

const ZERO_IN_VANILLA: [&str; 9] = [
    "MF-X001", "MF-X002", "MF-X010", "MF-X020", "MF-X021", "MF-X022", "MF-X030", "MF-X031",
    "MF-X112",
];

#[derive(Default)]
struct Report {
    count: usize,
    files: BTreeSet<String>,
    examples: Vec<String>,
}

impl Report {
    fn add(&mut self, file: &str, diagnostic: &Diagnostic, text: &str) {
        self.count += 1;
        self.files.insert(file.to_string());
        if self.examples.len() < 5 {
            let line = diagnostic.span.map_or(0, |span| {
                text[..span.start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1
            });
            self.examples
                .push(format!("{file}:{line} {}", diagnostic.message));
        }
    }
}

fn quoted(message: &str) -> &str {
    message
        .strip_prefix('`')
        .unwrap_or(message)
        .split('`')
        .next()
        .unwrap_or("")
}

#[test]
fn vanilla_gate() {
    let files = vanilla();
    if files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL not set");
        return;
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut structured_x040 = 0usize;
    let mut x110: Vec<(String, String)> = Vec::new();
    let mut x111: Vec<(String, String)> = Vec::new();
    for file in &files {
        let doc = parse(&file.text);
        let kind = detect(Some(&file.rel), &doc).kind;
        for diagnostic in validate_text(&file.text, Some(&file.rel)) {
            *counts.entry(diagnostic.code.clone()).or_default() += 1;
            match diagnostic.code.as_str() {
                "MF-X040"
                    if kind.is_some_and(CeKind::structured)
                        && kind != Some(CeKind::SpawnableTypes) =>
                {
                    structured_x040 += 1
                }
                "MF-X110" => {
                    x110.push((file.label.clone(), quoted(&diagnostic.message).to_string()))
                }
                "MF-X111" => {
                    x111.push((file.label.clone(), quoted(&diagnostic.message).to_string()))
                }
                _ => {}
            }
        }
    }

    println!("vanilla lint gate over {} files", files.len());
    for (code, count) in &counts {
        println!("  {code}  {count}");
    }
    println!("  MF-X040 in structured kinds: {structured_x040}");
    for (file, name) in &x110 {
        println!("  MF-X110 {name} ({file})");
    }
    for (file, name) in &x111 {
        println!("  MF-X111 {name} ({file})");
    }

    for code in ZERO_IN_VANILLA {
        assert_eq!(
            counts.get(code).copied().unwrap_or(0),
            0,
            "{code} fires in vanilla"
        );
    }
    assert_eq!(
        structured_x040, 0,
        "MF-X040 fires in a structured vanilla kind other than spawnable types (record_files_corpus pins those)"
    );
    assert!(x110.len() <= 1, "MF-X110: {x110:?}");
    assert!(
        x110.iter().all(|(_, name)| name == "BatteryCharger"),
        "MF-X110 outside Sakhal's BatteryCharger: {x110:?}"
    );
    assert!(x111.len() <= 2, "MF-X111: {x111:?}");
    assert!(
        x111.iter().all(|(_, name)| name == "Crossbow_Black"),
        "MF-X111 outside Crossbow_Black: {x111:?}"
    );
}

#[test]
fn workshop_report() {
    let files = workshop();
    if files.is_empty() {
        println!("skipped: MF_WORKSHOP_PATH not set");
        return;
    }

    let mut reports: BTreeMap<String, Report> = BTreeMap::new();
    let mut scanned = 0usize;
    for file in &files {
        let doc = parse(&file.text);
        if detect(Some(&file.rel), &doc).kind != Some(CeKind::Types) {
            continue;
        }
        scanned += 1;
        for diagnostic in validate_text(&file.text, Some(&file.rel)) {
            reports.entry(diagnostic.code.clone()).or_default().add(
                &file.label,
                &diagnostic,
                &file.text,
            );
        }
    }

    println!("workshop types report over {scanned} types files");
    for (code, report) in &reports {
        println!(
            "  {code}  {:>5} in {:>4} files",
            report.count,
            report.files.len()
        );
        for example in &report.examples {
            println!("      {example}");
        }
    }
}
