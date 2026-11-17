//! The config facts a Central Economy types file is checked against:
//! the workspace's own classes and the [`ConfigLookup`] the `ce.*` queries use
//! to see every class the config index knows.

use std::sync::{Arc, OnceLock};

use modfoundry_ce::classes::{self, ClassFact, ClassLookup, WorkspaceClass};
use modfoundry_config::ast::{Member, Value as AstValue};
use modfoundry_config::resolve::ConfigOrigin;
use modfoundry_core::text::{edit_distance_ignore_ascii_case, line_col};

use crate::config_host::{ConfigHost, Loaded};

const SPAWNABLE_ROOTS: [&str; 3] = ["CfgVehicles", "CfgWeapons", "CfgMagazines"];

/// The config index viewed as the [`ClassLookup`] a types file is linted against.
pub(crate) struct ConfigLookup {
    loaded: Arc<Loaded>,
    workspace_names: OnceLock<Vec<String>>,
}

impl ConfigLookup {
    fn new(loaded: Arc<Loaded>) -> Self {
        Self {
            loaded,
            workspace_names: OnceLock::new(),
        }
    }

    /// The session's config index as a lookup, or `None` before it is loaded.
    pub(crate) fn from_host(host: &ConfigHost) -> Option<Self> {
        host.loaded().map(Self::new)
    }
}

impl ClassLookup for ConfigLookup {
    fn class(&self, name: &str) -> Option<ClassFact> {
        for root in SPAWNABLE_ROOTS {
            let path = [root.to_string(), name.to_string()];
            let Some(resolved) = self.loaded.resolver.class(&path) else {
                continue;
            };
            let own = resolved
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| name.to_string());
            let scope = resolved
                .properties
                .iter()
                .find(|property| property.name.eq_ignore_ascii_case("scope"))
                .and_then(|property| whole_number(&property.value));
            let mut chain = Vec::with_capacity(1 + resolved.base_chain.len());
            chain.push(own.clone());
            chain.extend(
                resolved
                    .base_chain
                    .iter()
                    .filter_map(|base| base.path.last().cloned()),
            );
            return Some(ClassFact {
                name: own,
                scope,
                kind: classes::class_kind(&chain),
            });
        }
        None
    }

    fn nearest(&self, name: &str) -> Option<String> {
        let workspace_names = self.workspace_names.get_or_init(|| {
            let mut names = Vec::new();
            for source in self.loaded.sources.values() {
                if source.config.origin == ConfigOrigin::Workspace {
                    collect_names(&source.config.file.members, &mut names);
                }
            }
            names.sort();
            names.dedup();
            names
        });
        workspace_names
            .iter()
            .map(|candidate| (edit_distance_ignore_ascii_case(name, candidate), candidate))
            .filter(|(distance, _)| *distance <= 2)
            .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .map(|(_, candidate)| candidate.clone())
    }
}

/// Every spawnable class the workspace's own configs declare, in source-file order.
pub(crate) fn workspace_classes(host: &ConfigHost) -> Vec<WorkspaceClass> {
    let Some(lookup) = ConfigLookup::from_host(host) else {
        return Vec::new();
    };
    let loaded = &lookup.loaded;
    let mut sources: Vec<_> = loaded
        .sources
        .iter()
        .filter(|(_, source)| source.config.origin == ConfigOrigin::Workspace)
        .collect();
    sources.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::new();
    for (id, source) in sources {
        let Some(text) = &source.text else {
            continue;
        };
        let file = loaded
            .workspace
            .join(id.strip_prefix("workspace:").unwrap_or(id));
        for member in &source.config.file.members {
            let Member::Class(root_class) = member else {
                continue;
            };
            if !is_root_class(&root_class.name) {
                continue;
            }
            for child in &root_class.members {
                let Member::Class(child) = child else {
                    continue;
                };
                if child.forward {
                    continue;
                }
                let Some(fact) = lookup.class(&child.name) else {
                    continue;
                };
                out.push(WorkspaceClass {
                    name: child.name.clone(),
                    scope: fact.scope,
                    kind: fact.kind,
                    file: file.to_string_lossy().into_owned(),
                    span: child.spans.name,
                    line: line_col(text, child.spans.name.start).0,
                });
            }
        }
    }
    out
}

fn is_root_class(name: &str) -> bool {
    SPAWNABLE_ROOTS
        .iter()
        .any(|root| root.eq_ignore_ascii_case(name))
}

fn whole_number(value: &AstValue) -> Option<i64> {
    match value {
        AstValue::Number(text) => text.trim().parse().ok(),
        AstValue::Float(float) if float.is_finite() && float.fract() == 0.0 => Some(*float as i64),
        _ => None,
    }
}

fn collect_names(members: &[Member], out: &mut Vec<String>) {
    for member in members {
        if let Member::Class(class) = member {
            out.push(class.name.clone());
            collect_names(&class.members, out);
        }
    }
}
