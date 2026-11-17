//! Vanilla corpus gate for the resolver; skipped unless `MF_DAYZ_INSTALL` is set.

use crate::common;

use common::corpus::vanilla_configs;
use modfoundry_config::ast::{ConfigClass, Member};
use modfoundry_config::resolve::{ConfigOrigin, Resolver, SourceConfig};
use std::time::Instant;

const FAMILIES: [&str; 4] = ["CfgVehicles", "CfgWeapons", "CfgMagazines", "CfgAmmo"];

fn vanilla_sources() -> Vec<SourceConfig> {
    vanilla_configs()
        .into_iter()
        .filter_map(|c| {
            let file = c.file?;
            let vpath = c
                .label
                .splitn(3, ':')
                .nth(2)
                .unwrap_or_default()
                .to_string();
            Some(SourceConfig {
                id: c.label.clone(),
                display: c.label,
                vpath,
                origin: ConfigOrigin::Vanilla,
                file,
            })
        })
        .collect()
}

fn walk(
    path: &mut Vec<String>,
    class: &ConfigClass,
    visit: &mut dyn FnMut(&[String], &ConfigClass),
) {
    path.push(class.name.clone());
    visit(path, class);
    for m in &class.members {
        if let Member::Class(child) = m {
            walk(path, child, visit);
        }
    }
    path.pop();
}

#[test]
fn vanilla_configs_resolve() {
    let sources = vanilla_sources();
    if sources.is_empty() {
        return;
    }
    let files: Vec<_> = sources.iter().map(|s| s.file.clone()).collect();
    let count = sources.len();

    let started = Instant::now();
    let resolver = Resolver::new(sources);
    let built = started.elapsed();
    let probe = resolver.class(&["CfgVehicles".into(), "Canteen".into()]);
    eprintln!(
        "resolver over {count} configs: new {built:?}, new + one query {:?}",
        started.elapsed()
    );

    let order = resolver.load_order();
    assert!(
        order[0].starts_with("vanilla:bin.pbo:"),
        "first in load order is {}",
        order[0]
    );

    let canteen = probe.expect("Canteen resolves");
    assert_eq!(canteen.unresolved_base, None);
    assert_eq!(canteen.base_chain[0].path.last().unwrap(), "Bottle_Base");
    for name in ["scope", "varQuantityMax"] {
        assert!(
            canteen.properties.iter().any(|p| p.name == name && p.own),
            "{name} is not own"
        );
    }

    let mut top_total = 0;
    let mut top_unresolved = Vec::new();
    let mut nested_total = 0;
    let mut nested_unresolved = Vec::new();
    for file in &files {
        for member in &file.members {
            let Member::Class(root) = member else {
                continue;
            };
            let family = FAMILIES.iter().any(|f| f.eq_ignore_ascii_case(&root.name));
            walk(&mut Vec::new(), root, &mut |path, class| {
                if class.parents.is_empty() || class.forward {
                    return;
                }
                let unresolved = resolver
                    .class(path)
                    .map(|c| c.unresolved_base.is_some())
                    .unwrap_or(true);
                if family && path.len() == 2 {
                    top_total += 1;
                    if unresolved {
                        top_unresolved.push(path.join("/"));
                    }
                } else {
                    nested_total += 1;
                    if unresolved {
                        nested_unresolved.push(path.join("/"));
                    }
                }
            });
        }
    }
    eprintln!(
        "top-level family classes with a base: {top_total}, unresolved {}",
        top_unresolved.len()
    );
    eprintln!(
        "other classes with a base: {nested_total}, unresolved {}",
        nested_unresolved.len()
    );
    for path in nested_unresolved.iter().take(20) {
        eprintln!("  unresolved nested: {path}");
    }
    assert!(
        top_unresolved.is_empty(),
        "{:?}",
        &top_unresolved[..top_unresolved.len().min(20)]
    );
}
