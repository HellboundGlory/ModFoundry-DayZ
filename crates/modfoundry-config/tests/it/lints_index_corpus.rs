//! Corpus gate (release; skipped without the env vars): vanilla must raise no
//! MF-C410, and the addons vanilla requires but no readable vanilla config declares
//! are exactly the known protected-package ones. The workshop pass indexes vanilla only, so its hits (mods' own
//! dependencies) are reported, not gated.

use crate::common;

use common::corpus::{vanilla_configs, workshop_configs, CorpusConfig};
use modfoundry_config::lints::index_findings;
use modfoundry_config::rap::to_text;
use modfoundry_config::resolve::{ConfigOrigin, Resolver, SourceConfig};

fn readable(configs: &[CorpusConfig]) -> Vec<&CorpusConfig> {
    configs.iter().filter(|c| c.file.is_some()).collect()
}

fn resolver_of(configs: &[&CorpusConfig]) -> Resolver {
    let sources = configs
        .iter()
        .filter_map(|config| config.file.as_ref().map(|file| (config, file)))
        .enumerate()
        .map(|(i, (config, file))| SourceConfig {
            id: format!("vanilla:{i}"),
            display: config.label.clone(),
            vpath: config.label.clone(),
            origin: ConfigOrigin::Vanilla,
            file: file.clone(),
        })
        .collect();
    Resolver::new(sources)
}

fn text_of(config: &CorpusConfig) -> Option<String> {
    let file = config.file.as_ref()?;
    Some(match &config.text {
        Some(text) => text.clone(),
        None => to_text(file),
    })
}

#[test]
fn vanilla_has_no_unknown_required_addons() {
    let configs = vanilla_configs();
    if configs.is_empty() {
        return;
    }
    let readable = readable(&configs);
    let resolver = resolver_of(&readable);
    let mut files = 0u64;
    let mut found_in = Vec::new();
    for config in &readable {
        let Some(text) = text_of(config) else {
            continue;
        };
        files += 1;
        for diagnostic in index_findings(&text, &resolver, None) {
            eprintln!("  {}: {}", config.label, diagnostic.message);
            found_in.push(config.label.clone());
        }
    }
    eprintln!(
        "vanilla index lint: {files} files, {} MF-C410",
        found_in.len()
    );
    assert!(files >= 190, "only {files} vanilla configs");
    assert!(
        found_in.is_empty(),
        "vanilla MF-C410 findings: {found_in:?}"
    );

    let declared: std::collections::HashSet<String> = resolver
        .patches()
        .into_iter()
        .map(|patch| patch.name.to_ascii_lowercase())
        .collect();
    let mut hidden: Vec<String> = resolver
        .vanilla_required_addons()
        .into_iter()
        .filter(|name| !declared.contains(name))
        .collect();
    hidden.sort();
    // Declared inside the protected worlds_nasdara.ebo, which is not readable.
    assert_eq!(
        hidden,
        ["dz_worlds_nasdara"],
        "required by vanilla, declared nowhere readable"
    );
}

#[test]
fn workshop_unknown_addons_against_vanilla() {
    let vanilla = vanilla_configs();
    let configs = workshop_configs();
    if vanilla.is_empty() || configs.is_empty() {
        return;
    }
    let resolver = resolver_of(&readable(&vanilla));
    let mut files = 0u64;
    let mut count = 0u64;
    let mut examples = Vec::new();
    for config in configs.iter().filter(|c| !c.is_bin && c.issues == 0) {
        let Some(text) = config.text.as_deref() else {
            continue;
        };
        files += 1;
        for diagnostic in index_findings(text, &resolver, None) {
            count += 1;
            if examples.len() < 5 {
                examples.push(format!("{}: {}", config.label, diagnostic.message));
            }
        }
    }
    eprintln!("workshop index lint (vanilla-only index): {files} raw config.cpp, {count} MF-C410");
    for example in &examples {
        eprintln!("  {example}");
    }
}
