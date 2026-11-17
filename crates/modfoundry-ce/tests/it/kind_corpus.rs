//! Corpus gate: kind detection over the measured corpus. The
//! vanilla gate requires every file to have a kind and none to be a fragment;
//! the workshop side only reports, since 37 of its files are rootless fragments.
//!
//! Opt-in: skipped unless the env vars are set.
//!
//! ```text
//! MF_DAYZ_INSTALL=... MF_WORKSHOP_PATH=... \
//!   cargo test -p modfoundry-ce --release --test it -- kind_corpus --nocapture
//! ```

use crate::common;

use common::corpus::{vanilla, workshop};
use modfoundry_ce::kind::{detect, CeKind, Detected};
use modfoundry_ce::xml::parse;
use std::collections::BTreeMap;

#[test]
fn kind_corpus_gate() {
    let vanilla = vanilla();
    let mut kinds: BTreeMap<CeKind, usize> = BTreeMap::new();
    let mut fragments = 0;
    let mut unkinded = Vec::new();
    for file in &vanilla {
        let got = detect(Some(&file.rel), &parse(&file.text));
        if got.fragment {
            fragments += 1;
        }
        match got.kind {
            Some(kind) => *kinds.entry(kind).or_default() += 1,
            None => unkinded.push(format!("  {} [{}]", file.label, describe(&got))),
        }
    }

    let workshop = workshop();
    let mut ws_kinds: BTreeMap<CeKind, usize> = BTreeMap::new();
    let mut ws_fragment_kinds: BTreeMap<CeKind, usize> = BTreeMap::new();
    let mut ws_fragments = 0;
    let mut ws_unkinded = Vec::new();
    for file in &workshop {
        let got = detect(Some(&file.rel), &parse(&file.text));
        if got.fragment {
            ws_fragments += 1;
        }
        match (got.kind, got.fragment) {
            (Some(kind), true) => *ws_fragment_kinds.entry(kind).or_default() += 1,
            (Some(kind), false) => *ws_kinds.entry(kind).or_default() += 1,
            (None, _) => ws_unkinded.push(format!("  {} [{}]", file.label, describe(&got))),
        }
    }

    println!("vanilla: {} files, {} fragments", vanilla.len(), fragments);
    for kind in CeKind::ALL {
        if let Some(n) = kinds.get(&kind) {
            println!("  {:>4}  {}", n, kind.label());
        }
    }
    println!("vanilla: {} files with no kind", unkinded.len());
    for line in &unkinded {
        println!("{line}");
    }

    println!(
        "workshop: {} files, {} fragments",
        workshop.len(),
        ws_fragments
    );
    for kind in CeKind::ALL {
        if let Some(n) = ws_kinds.get(&kind) {
            println!("  {:>4}  {}", n, kind.label());
        }
    }
    println!("workshop fragments:");
    for kind in CeKind::ALL {
        if let Some(n) = ws_fragment_kinds.get(&kind) {
            println!("  {:>4}  {}", n, kind.label());
        }
    }
    println!("workshop: {} files with no kind", ws_unkinded.len());
    for line in &ws_unkinded {
        println!("{line}");
    }

    assert!(vanilla.is_empty() || fragments == 0, "vanilla fragment(s)");
    assert!(
        unkinded.is_empty(),
        "vanilla files with no kind (a detection bug — do not relax the gate):\n{}",
        unkinded.join("\n")
    );
}

fn describe(got: &Detected) -> String {
    match &got.root_mismatch {
        Some(root) => format!("root_mismatch={root}"),
        None => "no root".to_string(),
    }
}
