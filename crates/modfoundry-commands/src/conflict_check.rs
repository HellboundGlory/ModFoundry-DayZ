//! What several mods declare, configure or ship in common, in load order.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::conflict_data::{self, DataMod, Finding, Packing};
use crate::conflict_script::script_conflicts;
use crate::game_diff::{name_shared, read_mod};
use crate::launch_sets::resolve_set;
use crate::settings_fill::{names_a_workshop_mod, steam_root};

/// One checked mod, as the report lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ConflictMod {
    pub name: String,
    pub path: String,
    pub scripts: usize,
    pub configs: usize,
}

/// Everything a conflict check found.
#[derive(Debug, Clone, Serialize)]
pub struct ConflictReport {
    pub mods: Vec<ConflictMod>,
    pub load_order: Vec<String>,
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
}

/// What a check of fewer than two mods says.
pub const NEEDS_TWO_MODS: &str = "a conflict check needs two or more mods";

/// `order` when it names each of `count` mods once.
fn checked_order(order: Vec<usize>, count: usize) -> Result<Vec<usize>, String> {
    let mut seen = vec![false; count];
    let each_once = order.len() == count
        && order
            .iter()
            .all(|&i| i < count && !std::mem::replace(&mut seen[i], true));
    if each_once {
        Ok(order)
    } else {
        Err(format!(
            "load order {order:?} does not name each of the {count} mods once"
        ))
    }
}

/// Check `roots`, which are loaded in the order given.
pub fn conflict_check(roots: &[PathBuf]) -> Result<ConflictReport, String> {
    if roots.len() < 2 {
        return Err(NEEDS_TWO_MODS.to_owned());
    }
    let mut read = Vec::new();
    for root in roots {
        // Every read_mod error already names the folder.
        read.push(read_mod(root)?);
    }
    name_shared(&mut read);
    let (inputs, infos): (Vec<_>, Vec<_>) = read.into_iter().unzip();
    let packings: Vec<Packing> = roots
        .iter()
        .map(|root| conflict_data::read_packing(root))
        .collect();
    let data_mods: Vec<DataMod> = inputs
        .iter()
        .zip(&infos)
        .zip(&packings)
        .map(|((input, info), packing)| DataMod {
            name: &info.name,
            configs: &input.configs,
            packing,
        })
        .collect();
    let order = checked_order(conflict_data::load_order(&data_mods), data_mods.len())?;
    let data = conflict_data::data_conflicts(&data_mods, &order);
    let mut findings = script_conflicts(&inputs, &order);
    findings.extend(data.findings);
    sort_findings(&mut findings);
    Ok(ConflictReport {
        mods: infos
            .iter()
            .map(|info| ConflictMod {
                name: info.name.clone(),
                path: info.path.to_string_lossy().into_owned(),
                scripts: info.scripts,
                configs: info.configs,
            })
            .collect(),
        load_order: order.iter().map(|&i| infos[i].name.clone()).collect(),
        findings,
        warnings: warnings(&infos, &packings, data.warnings),
    })
}

/// Every mod's and packing's warnings, then the data side's, duplicates dropped.
fn warnings(
    infos: &[crate::game_diff::ModInfo],
    packings: &[Packing],
    data: Vec<String>,
) -> Vec<String> {
    let mut all = Vec::new();
    for (info, packing) in infos.iter().zip(packings) {
        for warning in info.warnings.iter().chain(&packing.warnings) {
            all.push(format!("{}: {warning}", info.name));
        }
    }
    all.extend(data);
    let mut seen = HashSet::new();
    all.retain(|warning| seen.insert(warning.clone()));
    all
}

/// Errors first, then by code, then by the first place.
pub(crate) fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.diagnostic
            .severity
            .cmp(&a.diagnostic.severity)
            .then_with(|| a.diagnostic.code.cmp(&b.diagnostic.code))
            .then_with(|| place_key(a).cmp(&place_key(b)))
    });
}

fn place_key(finding: &Finding) -> (&str, &str, Option<usize>) {
    finding
        .places
        .first()
        .map(|place| (place.mod_name.as_str(), place.path.as_str(), place.line))
        .unwrap_or(("", "", None))
}

/// The roots a launch set loads: its mods, then its server mods.
pub fn set_roots(root: &Path, set: &str) -> Result<Vec<PathBuf>, String> {
    let workshop = names_a_workshop_mod(root, set).map_err(|err| match err {
        modfoundry_core::CommandError::InvalidArguments(message) => message,
        other => other.to_string(),
    })?;
    let steam = if workshop {
        steam_root(&json!({}))
    } else {
        None
    };
    let resolved = resolve_set(root, set, steam.as_deref(), Path::exists)?;
    Ok(resolved
        .mods
        .iter()
        .chain(&resolved.server_mods)
        .map(|entry| {
            entry
                .local_root
                .clone()
                .unwrap_or_else(|| entry.arg.path.clone())
        })
        .collect())
}

/// The text report: the load order, one block per finding, then the counts.
pub fn render_text(report: &Value) -> String {
    let mut lines = vec![format!(
        "load order: {}",
        names(&report["load_order"]).join(", ")
    )];
    let mut counts = [0usize; 3];
    for finding in findings(report) {
        lines.push(format!(
            "{} {} {}",
            text(&finding["severity"]),
            text(&finding["code"]),
            text(&finding["message"])
        ));
        match text(&finding["severity"]) {
            "hint" => counts[2] += 1,
            "warning" => counts[1] += 1,
            _ => counts[0] += 1,
        }
        for place in finding["places"].as_array().into_iter().flatten() {
            let line = match place["line"].as_u64() {
                Some(line) => format!("  {}: {}:{line}", text(&place["mod"]), text(&place["path"])),
                None => format!("  {}: {}", text(&place["mod"]), text(&place["path"])),
            };
            lines.push(line);
        }
    }
    lines.push(format!(
        "{} errors, {} warnings, {} hints across {} mods",
        counts[0],
        counts[1],
        counts[2],
        report["mods"].as_array().map(Vec::len).unwrap_or(0)
    ));
    lines.join("\n")
}

fn findings(report: &Value) -> impl Iterator<Item = &Value> {
    report["findings"].as_array().into_iter().flatten()
}

fn names(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_core::diagnostic::{Diagnostic, Severity};

    fn finding(code: &str, severity: Severity, mod_name: &str, line: usize) -> Finding {
        Finding {
            diagnostic: Diagnostic::new(code, severity, "message"),
            mods: vec![mod_name.to_owned()],
            places: vec![crate::conflict_data::FindingPlace {
                mod_name: mod_name.to_owned(),
                path: "a.c".to_owned(),
                line: Some(line),
            }],
        }
    }

    #[test]
    fn findings_come_out_errors_first_then_code_then_place() {
        let mut findings = vec![
            finding("MF-M103", Severity::Warning, "B", 1),
            finding("MF-M102", Severity::Hint, "A", 1),
            finding("MF-M104", Severity::Error, "A", 1),
            finding("MF-M101", Severity::Error, "B", 1),
            finding("MF-M101", Severity::Error, "B", 2),
            finding("MF-M101", Severity::Error, "A", 3),
        ];

        sort_findings(&mut findings);

        let order: Vec<(&str, &str, Option<usize>)> = findings
            .iter()
            .map(|f| {
                (
                    f.diagnostic.code.as_str(),
                    f.places[0].mod_name.as_str(),
                    f.places[0].line,
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                ("MF-M101", "A", Some(3)),
                ("MF-M101", "B", Some(1)),
                ("MF-M101", "B", Some(2)),
                ("MF-M104", "A", Some(1)),
                ("MF-M103", "B", Some(1)),
                ("MF-M102", "A", Some(1)),
            ]
        );
    }

    #[test]
    fn the_text_report_lists_the_load_order_each_finding_and_the_counts() {
        let report = json!({
            "mods": [{"name": "A"}, {"name": "B"}],
            "load_order": ["A", "B"],
            "findings": [
                {
                    "code": "MF-M101",
                    "severity": "error",
                    "message": "Shared is declared by A, B",
                    "mods": ["A", "B"],
                    "places": [
                        {"mod": "A", "path": "a.c", "line": 1},
                        {"mod": "B", "path": "b.c", "line": null},
                    ],
                },
                {
                    "code": "MF-M102",
                    "severity": "hint",
                    "message": "class C is modded by A, B",
                    "mods": ["A", "B"],
                    "places": [],
                },
                {
                    "code": "MF-M000",
                    "severity": "fatal",
                    "message": "unreadable",
                    "mods": [],
                    "places": [],
                },
            ],
            "warnings": [],
        });

        let text = render_text(&report);

        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "load order: A, B",
                "error MF-M101 Shared is declared by A, B",
                "  A: a.c:1",
                "  B: b.c",
                "hint MF-M102 class C is modded by A, B",
                "fatal MF-M000 unreadable",
                "2 errors, 0 warnings, 1 hints across 2 mods",
            ]
        );
    }

    #[test]
    fn a_report_with_no_findings_prints_the_load_order_and_zero_counts() {
        let report = json!({
            "mods": [{"name": "A"}, {"name": "B"}],
            "load_order": ["A", "B"],
            "findings": [],
            "warnings": [],
        });

        assert_eq!(
            render_text(&report),
            "load order: A, B\n0 errors, 0 warnings, 0 hints across 2 mods"
        );
    }

    #[test]
    fn two_roots_are_the_least_a_check_takes() {
        let err = conflict_check(&[PathBuf::from("/no/such/mod")]).unwrap_err();
        assert_eq!(err, "a conflict check needs two or more mods");
    }

    #[test]
    fn an_order_that_is_not_one_of_each_mod_is_refused() {
        assert_eq!(checked_order(vec![1, 0], 2), Ok(vec![1, 0]));
        for order in [vec![0], vec![0, 0], vec![0, 2], vec![0, 1, 2]] {
            let refused = checked_order(order.clone(), 2).unwrap_err();
            assert!(refused.starts_with("load order "), "{order:?}: {refused}");
        }
    }
}
