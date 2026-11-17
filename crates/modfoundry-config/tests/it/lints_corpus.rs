//! Corpus gate: the file lints must stay at the counts
//! measured on a real install (run with `--release`; skipped without the env).
//!
//! The counts were measured on `config.cpp` from `parser::parse` and `config.bin` from
//! `read_rap`, so the counts here run the lints on the same trees. The vanilla
//! gate additionally runs the product path (`validate_text` on text) to prove
//! no code fires there either.
//!
//! Gate: `MF-C340` and `MF-C331` are 0 everywhere, every code is 0 on vanilla,
//! and the workshop's clean `config.cpp` and readable `config.bin` shares each
//! stay at or above their floor. `MF-C330`/`MF-C341` counts are printed with
//! examples — a count far from the measured one is a rule bug, not something to
//! tune away.

use crate::common;

use common::corpus::{vanilla_configs, workshop_configs, CorpusConfig};
use modfoundry_config::rap::to_text;
use modfoundry_config::validate::validate_text;
use modfoundry_core::Diagnostic;
use std::collections::BTreeMap;

const CODES: [&str; 5] = ["MF-C330", "MF-C331", "MF-C340", "MF-C341", "MF-C350"];

#[derive(Default)]
struct Tally {
    files: u64,
    counts: BTreeMap<String, u64>,
    examples: BTreeMap<String, Vec<String>>,
}

impl Tally {
    fn add(&mut self, label: &str, diagnostics: &[Diagnostic]) {
        for d in diagnostics {
            if !CODES.contains(&d.code.as_str()) {
                continue;
            }
            *self.counts.entry(d.code.clone()).or_default() += 1;
            let examples = self.examples.entry(d.code.clone()).or_default();
            if examples.len() < 5 {
                examples.push(format!("{label}: {}", d.message));
            }
        }
    }

    fn count(&self, code: &str) -> u64 {
        self.counts.get(code).copied().unwrap_or(0)
    }

    fn report(&self, layer: &str) {
        eprintln!("{layer}: {} files linted", self.files);
        for code in CODES {
            eprintln!("  {code}: {}", self.count(code));
            for example in self.examples.get(code).map(Vec::as_slice).unwrap_or(&[]) {
                eprintln!("    {example}");
            }
        }
    }
}

fn text_of(config: &CorpusConfig) -> Option<String> {
    let file = config.file.as_ref()?;
    Some(match &config.text {
        Some(text) => text.clone(),
        None => to_text(file),
    })
}

/// The lint rules on the parsed tree, the way the counts were measured.
fn lint_tree(layer: &str, configs: &[&CorpusConfig]) -> Tally {
    let mut tally = Tally::default();
    for config in configs {
        let Some(file) = config.file.as_ref() else {
            continue;
        };
        let text = text_of(config).unwrap_or_default();
        tally.files += 1;
        tally.add(&config.label, &modfoundry_config::lints::run(file, &text));
    }
    tally.report(layer);
    tally
}

/// The product path: everything `validate_text` reports for the file's text.
fn lint_text(layer: &str, configs: &[&CorpusConfig]) -> Tally {
    let mut tally = Tally::default();
    for config in configs {
        let Some(text) = text_of(config) else {
            continue;
        };
        tally.files += 1;
        tally.add(&config.label, &validate_text(&text, None).diagnostics);
    }
    tally.report(layer);
    tally
}

fn readable_bins(configs: &[CorpusConfig]) -> Vec<&CorpusConfig> {
    configs
        .iter()
        .filter(|c| c.is_bin && c.file.is_some())
        .collect()
}

fn clean_cpps(configs: &[CorpusConfig]) -> Vec<&CorpusConfig> {
    configs
        .iter()
        .filter(|c| !c.is_bin && c.issues == 0 && c.file.is_some())
        .collect()
}

#[test]
fn vanilla_is_clean_for_every_lint() {
    let configs = vanilla_configs();
    if configs.is_empty() {
        return;
    }
    let bins = readable_bins(&configs);
    let tree = lint_tree("vanilla (read_rap AST)", &bins);
    assert!(tree.files >= 190, "only {} vanilla configs", tree.files);
    for code in CODES {
        assert_eq!(
            tree.count(code),
            0,
            "vanilla {code}: {:?}",
            tree.examples.get(code)
        );
    }

    let text = lint_text("vanilla (validate_text)", &bins);
    for code in CODES {
        assert_eq!(
            text.count(code),
            0,
            "vanilla {code}: {:?}",
            text.examples.get(code)
        );
    }
}

const WORKSHOP_CPP_FLOOR_PERCENT: u64 = 90;
const WORKSHOP_BIN_FLOOR_PERCENT: u64 = 90;
// Below this the shares say nothing about the parser.
const WORKSHOP_MIN_CONFIGS: usize = 1000;

fn share_percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        return 100;
    }
    part * 100 / whole
}

#[test]
fn workshop_lint_counts_hold_the_gate() {
    let configs = workshop_configs();
    if configs.is_empty() {
        return;
    }

    assert!(
        configs.len() >= WORKSHOP_MIN_CONFIGS,
        "only {} workshop configs; point MF_WORKSHOP_PATH at a real workshop",
        configs.len()
    );

    let cpps = clean_cpps(&configs);
    let cpp_tree = lint_tree("workshop config.cpp (issue-free)", &cpps);
    let cpp_total = configs.iter().filter(|c| !c.is_bin).count() as u64;
    let cpp_share = share_percent(cpps.len() as u64, cpp_total);
    eprintln!(
        "workshop config.cpp: {} clean of {cpp_total} read ({cpp_share}%)",
        cpps.len()
    );
    assert!(
        cpp_share >= WORKSHOP_CPP_FLOOR_PERCENT,
        "only {cpp_share}% clean config.cpp ({} of {cpp_total})",
        cpps.len()
    );

    let bins = readable_bins(&configs);
    let bin_tree = lint_tree("workshop config.bin (readable)", &bins);
    let bin_total = configs.iter().filter(|c| c.is_bin).count() as u64;
    let bin_share = share_percent(bins.len() as u64, bin_total);
    eprintln!(
        "workshop config.bin: {} readable of {bin_total} read ({bin_share}%)",
        bins.len()
    );
    assert!(
        bin_share >= WORKSHOP_BIN_FLOOR_PERCENT,
        "only {bin_share}% readable config.bin ({} of {bin_total})",
        bins.len()
    );

    for tally in [&cpp_tree, &bin_tree] {
        for code in ["MF-C340", "MF-C331"] {
            assert_eq!(
                tally.count(code),
                0,
                "{code}: {:?}",
                tally.examples.get(code)
            );
        }
    }

    let bin_text = lint_text("workshop config.bin (validate_text)", &bins);
    eprintln!(
        "config.bin text path: MF-C341 {} (to_text rewrites base names to the declaration's \
         spelling, so the editor path cannot see the binary's casing)",
        bin_text.count("MF-C341")
    );
}
