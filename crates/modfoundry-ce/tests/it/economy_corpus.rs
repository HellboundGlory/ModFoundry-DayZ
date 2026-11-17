//! Corpus gate: the vanilla cross-file gate and the workshop report.
//!
//! Opt-in: the gate needs `MF_DAYZ_INSTALL`, the report needs both
//! `MF_DAYZ_INSTALL` (for the Chernarus vocabulary) and `MF_WORKSHOP_PATH`;
//! each prints `skipped:` and passes when its variables are unset.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- economy_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop};
use modfoundry_ce::economy::{cross_findings, Economy, EconomySource, Origin};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::model::{self, Document};
use std::collections::BTreeMap;

#[test]
fn vanilla_gate() {
    let files = vanilla();
    if files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL not set");
        return;
    }

    let mut worlds: BTreeMap<String, Vec<(String, Document)>> = BTreeMap::new();
    for file in &files {
        let Some(world) = &file.world else {
            continue;
        };
        let document = model::read(&file.text, Some(&file.rel));
        worlds
            .entry(world.clone())
            .or_default()
            .push((file.rel.clone(), document));
    }

    let mut checked = 0usize;
    let mut findings_total = 0usize;
    for (world, docs) in &worlds {
        let sources: Vec<EconomySource<'_>> = docs
            .iter()
            .filter(|(_, document)| {
                matches!(
                    document.kind,
                    Some(CeKind::Limits | CeKind::LimitsUser | CeKind::Types)
                )
            })
            .map(|(rel, document)| EconomySource {
                id: rel,
                origin: Origin::Reference,
                document,
            })
            .collect();
        let economy = Economy::build(&sources);
        assert!(economy.has_limits, "{world} has no limits document");

        for (rel, document) in docs {
            if document.kind != Some(CeKind::Types) {
                continue;
            }
            checked += 1;
            for finding in cross_findings(document, &economy) {
                findings_total += 1;
                println!("  {world} {rel} {} {}", finding.code, finding.message);
            }
        }
    }

    println!(
        "vanilla cross-file gate over {checked} types files in {} worlds: {findings_total} findings",
        worlds.len()
    );
    assert_eq!(findings_total, 0, "cross-file findings in vanilla");
}

#[test]
fn workshop_report() {
    let reference = vanilla();
    let files = workshop();
    if reference.is_empty() || files.is_empty() {
        println!("skipped: MF_DAYZ_INSTALL / MF_WORKSHOP_PATH not set");
        return;
    }

    let docs: Vec<(String, Document)> = reference
        .iter()
        .filter(|file| file.world.as_deref() == Some("chernarusplus"))
        .map(|file| (file.rel.clone(), model::read(&file.text, Some(&file.rel))))
        .collect();
    let sources: Vec<EconomySource<'_>> = docs
        .iter()
        .filter(|(_, document)| {
            matches!(
                document.kind,
                Some(CeKind::Limits | CeKind::LimitsUser | CeKind::Types)
            )
        })
        .map(|(rel, document)| EconomySource {
            id: rel,
            origin: Origin::Reference,
            document,
        })
        .collect();
    let economy = Economy::build(&sources);
    assert!(economy.has_limits, "chernarusplus has no limits document");

    let mut codes: BTreeMap<String, usize> = BTreeMap::new();
    let mut names: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut scanned = 0usize;
    for file in &files {
        let document = model::read(&file.text, Some(&file.rel));
        if document.kind != Some(CeKind::Types) {
            continue;
        }
        scanned += 1;
        for finding in cross_findings(&document, &economy) {
            *codes.entry(finding.code.clone()).or_default() += 1;
            *names
                .entry(finding.code.clone())
                .or_default()
                .entry(first_name(&finding.message))
                .or_default() += 1;
        }
    }

    println!("workshop types report over {scanned} types files against chernarusplus");
    for code in ["MF-X130", "MF-X131", "MF-X132"] {
        println!("  {code}  {:>5}", codes.get(code).copied().unwrap_or(0));
        let mut top: Vec<(&String, &usize)> = names
            .get(code)
            .map(|counts| counts.iter().collect())
            .unwrap_or_default();
        top.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (name, count) in top.into_iter().take(10) {
            println!("      {name:>24}  {count}");
        }
    }
}

fn first_name(message: &str) -> String {
    message.split('`').nth(1).unwrap_or("").to_string()
}
