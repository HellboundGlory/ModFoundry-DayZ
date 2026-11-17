//! Corpus gate for the edit engine: no-op sets and rename/set round trips over real configs.
//! Skipped unless `MF_DAYZ_INSTALL` / `MF_WORKSHOP_PATH` are set. `MF_EDIT_SAMPLE` (default 6)
//! sets how many members per file get the reparse-heavy round trips.

use crate::common;
use crate::edit_fixtures as fixtures;

use common::corpus::{vanilla_configs, workshop_configs};
use fixtures::*;
use modfoundry_config::ast::{ConfigClass, ConfigFile, ConfigProperty, Member, Value};
use modfoundry_config::edit::*;
use modfoundry_config::parser::parse;
use modfoundry_config::rap::to_text;
use modfoundry_core::text::is_c_identifier;

const PROBE: &str = "__mf_probe";

#[derive(Default)]
struct Stats {
    files: usize,
    noop_checked: usize,
    noop_mismatch: usize,
    noop_unresolved: usize,
    round_trips: usize,
    round_trip_mismatch: usize,
    skipped_odd_names: usize,
    skipped_unresolved: usize,
    postcondition_failures: usize,
}

enum Item<'a> {
    Class(Vec<String>, &'a ConfigClass),
    Property(Vec<String>, &'a ConfigProperty),
}

fn collect<'a>(members: &'a [Member], path: &mut Vec<String>, out: &mut Vec<Item<'a>>) {
    for m in members {
        match m {
            Member::Class(c) => {
                path.push(c.name.clone());
                out.push(Item::Class(path.clone(), c));
                collect(&c.members, path, out);
                path.pop();
            }
            Member::Property(p) => out.push(Item::Property(path.clone(), p)),
            Member::Delete { .. } => {}
        }
    }
}

fn names(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

fn sample_size() -> usize {
    std::env::var("MF_EDIT_SAMPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6)
}

fn round_trip(text: &str, there: &EditOp, back: &EditOp, stats: &mut Stats, label: &str) {
    let Ok(edits) = plan(text, there) else {
        stats.skipped_unresolved += 1;
        return;
    };
    if check_postcondition(text, there, &edits).is_err() {
        stats.postcondition_failures += 1;
        eprintln!("postcondition failed: {label}: {there:?}");
    }
    let changed = apply(text, &edits);
    let restored = plan(&changed, back).map(|e| apply(&changed, &e));
    stats.round_trips += 1;
    if restored.as_deref() != Ok(text) {
        stats.round_trip_mismatch += 1;
        eprintln!("round trip mismatch: {label}: {there:?}");
    }
}

fn check_text(label: &str, text: &str, stats: &mut Stats) {
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return;
    }
    let file: &ConfigFile = &parsed.file;
    stats.files += 1;
    let mut items = Vec::new();
    collect(&file.members, &mut Vec::new(), &mut items);

    for item in &items {
        let Item::Property(path, p) = item else {
            continue;
        };
        let Some(value) = p.value.as_ref().and_then(to_new) else {
            continue;
        };
        stats.noop_checked += 1;
        match plan_in(text, file, &set(&names(path), &p.name, value)) {
            Ok(edits) if edits.is_empty() => {}
            Ok(_) => {
                stats.noop_mismatch += 1;
                eprintln!("no-op edit planned: {label}: {path:?}/{}", p.name);
            }
            Err(_) => stats.noop_unresolved += 1,
        }
    }

    let step = (items.len() / sample_size().max(1)).max(1);
    for item in items.iter().step_by(step) {
        match item {
            Item::Class(path, c) => {
                let spelled = &text[c.spans.name.start..c.spans.name.end];
                if !is_c_identifier(&c.name) || spelled != c.name {
                    stats.skipped_odd_names += 1;
                    continue;
                }
                let mut renamed = path.clone();
                *renamed.last_mut().expect("class path") = PROBE.into();
                round_trip(
                    text,
                    &rename(class(&names(path)), PROBE),
                    &rename(class(&names(&renamed)), &c.name),
                    stats,
                    label,
                );
            }
            Item::Property(path, p) => {
                let spelled = &text[p.spans.name.start..p.spans.name.end];
                if !is_c_identifier(&p.name) || spelled != p.name {
                    stats.skipped_odd_names += 1;
                    continue;
                }
                round_trip(
                    text,
                    &rename(prop(&names(path), &p.name), PROBE),
                    &rename(prop(&names(path), PROBE), &p.name),
                    stats,
                    label,
                );
                let scalar = matches!(
                    &p.value,
                    Some(Value::Str(_) | Value::Number(_) | Value::Ident(_))
                );
                let original = p.value.as_ref().and_then(to_new);
                if let (true, Some(original)) = (scalar && !p.declared_array, original) {
                    if render_value(&original, &infer_style("", None)).is_err() {
                        stats.skipped_odd_names += 1;
                        continue;
                    }
                    round_trip(
                        text,
                        &set(&names(path), &p.name, s(PROBE)),
                        &set(&names(path), &p.name, original),
                        stats,
                        label,
                    );
                }
            }
        }
    }
}

fn report(layer: &str, stats: &Stats) {
    eprintln!(
        "{layer}: {} files; no-op set {} checked / {} mismatched / {} unresolved; round trips {} / {} mismatched / {} skipped (odd names) / {} skipped (unresolved); postcondition failures {}",
        stats.files,
        stats.noop_checked,
        stats.noop_mismatch,
        stats.noop_unresolved,
        stats.round_trips,
        stats.round_trip_mismatch,
        stats.skipped_odd_names,
        stats.skipped_unresolved,
        stats.postcondition_failures,
    );
}

fn assert_clean(stats: &Stats) {
    assert_eq!(stats.noop_mismatch, 0);
    assert_eq!(stats.round_trip_mismatch, 0);
    assert_eq!(stats.postcondition_failures, 0);
}

#[test]
fn workshop_config_cpp_edits_round_trip() {
    let configs = workshop_configs();
    let mut stats = Stats::default();
    for c in configs.iter().filter(|c| !c.is_bin && c.issues == 0) {
        check_text(&c.label, c.text.as_deref().expect("cpp text"), &mut stats);
    }
    report("workshop", &stats);
    assert_clean(&stats);
}

#[test]
fn vanilla_derapified_edits_round_trip() {
    let configs = vanilla_configs();
    let mut stats = Stats::default();
    for c in configs.iter().filter(|c| c.is_bin) {
        let Some(tree) = c.file.as_ref() else {
            continue;
        };
        check_text(&c.label, &to_text(tree), &mut stats);
    }
    report("vanilla", &stats);
    assert_clean(&stats);
}
