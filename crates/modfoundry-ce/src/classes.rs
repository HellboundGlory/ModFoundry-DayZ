//! Class facts from loaded configs: whether a types key names a public class
//! (MF-X121, MF-X122, MF-X123) and which spawnable loot classes have no entry
//! (MF-X120).

use crate::economy::Economy;
use crate::fixes::did_you_mean;
use crate::kind::CeKind;
use crate::lints::span_start;
use crate::model::Document;
use modfoundry_core::{Diagnostic, Severity, Span};
use serde::{Deserialize, Serialize};

/// What a class is for, from the root class it descends from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassKind {
    Loot,
    House,
    Vehicle,
    Character,
    Other,
}

/// What a loaded config knows about one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassFact {
    /// The name as declared.
    pub name: String,
    pub scope: Option<i64>,
    pub kind: ClassKind,
}

/// The config facts a types file is checked against.
pub trait ClassLookup {
    /// The class named `name` in any loaded config, case-insensitively.
    fn class(&self, name: &str) -> Option<ClassFact>;

    /// A close class name worth suggesting, if any.
    fn nearest(&self, name: &str) -> Option<String>;
}

/// The kind of the root class the chain names, the class itself first.
pub fn class_kind(base_chain: &[String]) -> ClassKind {
    for name in base_chain {
        let kind = match name.to_ascii_lowercase().as_str() {
            "inventory_base" | "defaultweapon" | "defaultmagazine" => ClassKind::Loot,
            "housenodestruct" => ClassKind::House,
            "carscript" | "boatscript" => ClassKind::Vehicle,
            "survivorbase" | "dz_lightai" => ClassKind::Character,
            _ => continue,
        };
        return kind;
    }
    ClassKind::Other
}

/// The MF-X121, MF-X122 and MF-X123 findings for a types document against
/// `lookup`; empty for any other kind of document.
pub fn class_findings(document: &Document, lookup: &dyn ClassLookup) -> Vec<Diagnostic> {
    if document.kind != Some(CeKind::Types) {
        return Vec::new();
    }
    let Some(section) = document.section("types") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        if record.key.is_empty() {
            continue;
        }
        let Some(span) = record.key_span else {
            continue;
        };
        match lookup.class(&record.key) {
            None => findings.push(Diagnostic::at(
                "MF-X121",
                Severity::Warning,
                span,
                format!(
                    "`{}` isn't a class in any loaded config, so the server ignores this entry.{}",
                    record.key,
                    did_you_mean(lookup.nearest(&record.key).as_deref())
                ),
            )),
            Some(fact) => {
                if let Some(scope) = fact.scope {
                    if scope != 2 {
                        findings.push(Diagnostic::at(
                            "MF-X122",
                            Severity::Warning,
                            span,
                            format!(
                                "`{}` isn't public (scope {scope}), so the server ignores this entry.",
                                record.key
                            ),
                        ));
                    }
                }
                if fact.name != record.key {
                    findings.push(Diagnostic::at(
                        "MF-X123",
                        Severity::Hint,
                        span,
                        format!("`{}` is declared as `{}`.", record.key, fact.name),
                    ));
                }
            }
        }
    }
    findings.sort_by(|a, b| {
        span_start(a)
            .cmp(&span_start(b))
            .then_with(|| a.code.cmp(&b.code))
    });
    findings
}

/// One class a workspace config declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceClass {
    pub name: String,
    pub scope: Option<i64>,
    pub kind: ClassKind,
    pub file: String,
    pub span: Span,
    pub line: usize,
}

/// A workspace class and whether the economy has a types entry for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveredClass {
    #[serde(flatten)]
    pub class: WorkspaceClass,
    pub has_types: bool,
}

/// The spawnable loot classes a workspace declares, and how many the economy covers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub classes: Vec<CoveredClass>,
    pub covered: usize,
    pub missing: usize,
}

/// The spawnable loot classes of `classes` in order, each marked with whether
/// `economy` has a types entry for it.
pub fn coverage(classes: &[WorkspaceClass], economy: &Economy) -> Coverage {
    let mut result = Coverage::default();
    for class in classes {
        if class.scope != Some(2) || class.kind != ClassKind::Loot {
            continue;
        }
        let has_types = economy.has_type(&class.name);
        if has_types {
            result.covered += 1;
        } else {
            result.missing += 1;
        }
        result.classes.push(CoveredClass {
            class: class.clone(),
            has_types,
        });
    }
    result
}

/// One MF-X120 hint per missing class, paired with the file it is declared in.
pub fn coverage_findings(coverage: &Coverage) -> Vec<(String, Diagnostic)> {
    coverage
        .classes
        .iter()
        .filter(|entry| !entry.has_types)
        .map(|entry| {
            (
                entry.class.file.clone(),
                Diagnostic::at(
                    "MF-X120",
                    Severity::Hint,
                    entry.class.span,
                    format!(
                        "`{}` is spawnable (scope 2) but has no types entry, so it never spawns as loot.",
                        entry.class.name
                    ),
                ),
            )
        })
        .collect()
}
