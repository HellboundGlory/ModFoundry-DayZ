//! Corpus gate: the schema's array-ness must hold on every measured
//! occurrence, and every listed name must occur in the corpus.

use crate::common;

use common::corpus::{vanilla_configs, workshop_configs, CorpusConfig};
use modfoundry_config::ast::{ConfigFile, Member};
use modfoundry_config::schema;
use std::collections::HashMap;

type Counts = HashMap<(String, String), (u64, u64)>;

fn walk(root: &str, members: &[Member], counts: &mut Counts) {
    for member in members {
        match member {
            Member::Class(class) => walk(root, &class.members, counts),
            Member::Property(property) => {
                let entry = counts
                    .entry((
                        root.to_ascii_lowercase(),
                        property.name.to_ascii_lowercase(),
                    ))
                    .or_default();
                if property.declared_array {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
            Member::Delete { .. } => {}
        }
    }
}

fn collect(configs: &[CorpusConfig]) -> Counts {
    let mut counts = Counts::new();
    for config in configs {
        let Some(file) = &config.file else { continue };
        walk_file(file, &mut counts);
    }
    counts
}

fn walk_file(file: &ConfigFile, counts: &mut Counts) {
    for member in &file.members {
        if let Member::Class(class) = member {
            walk(&class.name, &class.members, counts);
        }
    }
}

fn occurrences(counts: &Counts, root: &str, name: &str) -> (u64, u64) {
    counts
        .get(&(root.to_ascii_lowercase(), name.to_ascii_lowercase()))
        .copied()
        .unwrap_or((0, 0))
}

#[test]
fn vanilla_array_consistency() {
    let configs = vanilla_configs();
    if configs.is_empty() {
        return;
    }
    let counts = collect(&configs);

    let mut vehicles: Vec<_> = counts
        .iter()
        .filter(|((root, _), _)| root == "cfgvehicles")
        .map(|((_, name), seen)| (name.clone(), *seen))
        .collect();
    vehicles.sort_by_key(|a| std::cmp::Reverse(a.1 .0 + a.1 .1));
    eprintln!("vanilla CfgVehicles names, top 150 by occurrence (array, scalar):");
    for (name, seen) in vehicles.iter().take(150) {
        eprintln!("  {name}\t{}\t{}", seen.0, seen.1);
    }
    for name in [
        "inventorySlot",
        "inputRange",
        "selectionRLegWound",
        "selectionRArmWound",
    ] {
        eprintln!(
            "  spelled both ways: {name} {:?}",
            occurrences(&counts, "CfgVehicles", name)
        );
    }

    let mut violations = Vec::new();
    for ((root, name), (arrays, scalars)) in &counts {
        let Some(info) = schema::lookup(root, name) else {
            continue;
        };
        match info.array {
            Some(true) if *scalars > 0 => {
                violations.push(format!("{root}:{name} has {scalars} scalar occurrences"))
            }
            Some(false) if *arrays > 0 => {
                violations.push(format!("{root}:{name} has {arrays} array occurrences"))
            }
            _ => {}
        }
    }
    assert!(
        violations.is_empty(),
        "schema disagrees with the corpus:\n{}",
        violations.join("\n")
    );

    for (root, info) in schema::rows() {
        if root != "*" {
            continue;
        }
        let seen = occurrences(&counts, "CfgVehicles", info.name);
        assert!(
            seen.0 + seen.1 > 0,
            "{} never occurs in vanilla CfgVehicles",
            info.name
        );
    }
}

#[test]
fn workshop_patch_and_mod_arrays() {
    let configs = workshop_configs();
    if configs.is_empty() {
        return;
    }
    let counts = collect(&configs);
    let mut checked = 0;
    for (root, info) in schema::rows() {
        if root != "CfgPatches" && root != "CfgMods" {
            continue;
        }
        if info.array != Some(true) {
            continue;
        }
        let seen = occurrences(&counts, root, info.name);
        assert!(
            seen.0 > 0,
            "{root}:{} never occurs in the workshop corpus",
            info.name
        );
        assert_eq!(seen.1, 0, "{root}:{} has scalar occurrences", info.name);
        checked += 1;
    }
    eprintln!("workshop: {checked} CfgPatches/CfgMods array rows verified");
    assert_eq!(checked, 6, "expected six root-specific array rows");
}
