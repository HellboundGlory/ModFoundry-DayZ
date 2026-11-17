//! The five guess lints against the game check, on unpacked mods, opt-in.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use enforce_check::loader::{load_input, load_mod_dir, LoadedMod};
use enforce_check::{check, CheckResult, Layer, Target};
use modfoundry_commands::registry::FindingReport;
use modfoundry_commands::script_check::merge_findings;
use modfoundry_core::text::line_col;
use modfoundry_core::{Actor, Core, Outcome};
use serde_json::{json, Value};

use crate::common;

struct ModAddons {
    addons: Vec<String>,
    required: Vec<String>,
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

/// The mods `index` needs, directly or through others, in ascending order.
fn needed_mods(mods: &[ModAddons], index: usize) -> Vec<usize> {
    let mut found = vec![false; mods.len()];
    found[index] = true;
    let mut pending = vec![index];
    while let Some(current) = pending.pop() {
        for required in &mods[current].required {
            for (provider, lib) in mods.iter().enumerate() {
                let provides = lib
                    .addons
                    .iter()
                    .any(|addon| addon.eq_ignore_ascii_case(required));
                if provides && !found[provider] {
                    found[provider] = true;
                    pending.push(provider);
                }
            }
        }
    }
    (0..mods.len())
        .filter(|&i| found[i] && i != index)
        .collect()
}

#[derive(Clone)]
struct LintLine {
    code: String,
    path: PathBuf,
    line: u32,
    message: String,
}

struct GameLine {
    path: PathBuf,
    line: u32,
    message: String,
}

#[derive(Default)]
struct Tally {
    codes: BTreeMap<String, CodeTally>,
    game_only: usize,
    game_only_shapes: BTreeMap<String, usize>,
}

#[derive(Default)]
struct CodeTally {
    lint: usize,
    same_line: usize,
    /// In a file of a module the game gave up on, or of a later one: not compiled, so not comparable.
    not_reached: usize,
    lint_only: Vec<LintLine>,
    game_shapes: BTreeMap<String, usize>,
}

fn tally(lints: &[LintLine], game: &[GameLine], unreached: &HashSet<PathBuf>) -> Tally {
    let mut game_at: HashMap<(&Path, u32), Vec<usize>> = HashMap::new();
    for (index, line) in game.iter().enumerate() {
        game_at
            .entry((line.path.as_path(), line.line))
            .or_default()
            .push(index);
    }
    let mut result = Tally::default();
    let mut matched = vec![false; game.len()];
    let mut counted: HashSet<(&str, usize)> = HashSet::new();
    for lint in lints {
        let code = result.codes.entry(lint.code.clone()).or_default();
        code.lint += 1;
        let Some(hits) = game_at.get(&(lint.path.as_path(), lint.line)) else {
            if unreached.contains(&lint.path) {
                code.not_reached += 1;
            } else {
                code.lint_only.push(lint.clone());
            }
            continue;
        };
        code.same_line += 1;
        for &index in hits {
            matched[index] = true;
            if counted.insert((lint.code.as_str(), index)) {
                *code
                    .game_shapes
                    .entry(shape(&game[index].message))
                    .or_default() += 1;
            }
        }
    }
    for (index, line) in game.iter().enumerate() {
        if !matched[index] {
            result.game_only += 1;
            *result
                .game_only_shapes
                .entry(shape(&line.message))
                .or_default() += 1;
        }
    }
    result
}

/// Quoted parts become `…`, so messages that differ only by a name count together.
/// A quote after a letter is an apostrophe (`Can't`), and one with no partner stays.
fn shape(message: &str) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some(open) = rest.find('\'') {
        let after = &rest[open + 1..];
        let apostrophe = rest[..open]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        match after.find('\'') {
            Some(close) if !apostrophe => {
                out.push_str(&rest[..open]);
                out.push_str("'…'");
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn needed_mods_follows_requirements_through_other_mods() {
    let mods = [
        ModAddons {
            addons: names(&["A"]),
            required: names(&["B"]),
        },
        ModAddons {
            addons: names(&["B"]),
            required: names(&["C"]),
        },
        ModAddons {
            addons: names(&["C"]),
            required: Vec::new(),
        },
        ModAddons {
            addons: names(&["D"]),
            required: Vec::new(),
        },
    ];
    assert_eq!(needed_mods(&mods, 0), [1, 2]);
    assert!(needed_mods(&mods, 2).is_empty());
}

#[test]
fn needed_mods_ignores_letter_case_of_addons() {
    let mods = [
        ModAddons {
            addons: names(&["A"]),
            required: names(&["cf_core"]),
        },
        ModAddons {
            addons: names(&["CF_Core"]),
            required: Vec::new(),
        },
    ];
    assert_eq!(needed_mods(&mods, 0), [1]);
}

#[test]
fn needed_mods_skips_an_addon_no_mod_provides() {
    let mods = [
        ModAddons {
            addons: names(&["A"]),
            required: names(&["DZ_Vanilla"]),
        },
        ModAddons {
            addons: names(&["B"]),
            required: Vec::new(),
        },
    ];
    assert!(needed_mods(&mods, 0).is_empty());
}

#[test]
fn needed_mods_ends_a_circle_of_requirements() {
    let mods = [
        ModAddons {
            addons: names(&["A"]),
            required: names(&["B"]),
        },
        ModAddons {
            addons: names(&["B"]),
            required: names(&["A"]),
        },
    ];
    assert_eq!(needed_mods(&mods, 0), [1]);
    assert_eq!(needed_mods(&mods, 1), [0]);
}

#[test]
fn shape_replaces_each_quoted_part_and_keeps_the_rest() {
    assert_eq!(shape("Unknown type 'Foo'"), "Unknown type '…'");
    assert_eq!(shape("Unknown type 'Bar'"), shape("Unknown type 'Foo'"));
    assert_eq!(shape("Missing 'a' in 'b' here"), "Missing '…' in '…' here");
    assert_eq!(shape("Broken expression"), "Broken expression");
    assert_eq!(shape("it's bad"), "it's bad");
    assert_eq!(shape("Can't find variable 'x'"), "Can't find variable '…'");
}

#[test]
fn tally_splits_lint_lines_by_game_match() {
    let lints = [
        LintLine {
            code: "MF-E1001".into(),
            path: "m/a.c".into(),
            line: 3,
            message: "Broken".into(),
        },
        LintLine {
            code: "MF-E1001".into(),
            path: "m/b.c".into(),
            line: 7,
            message: "Broken".into(),
        },
    ];
    let game = [
        GameLine {
            path: "m/a.c".into(),
            line: 3,
            message: "Broken expression".into(),
        },
        GameLine {
            path: "m/c.c".into(),
            line: 1,
            message: "Unknown type 'Foo'".into(),
        },
    ];
    let result = tally(&lints, &game, &HashSet::new());
    let code = &result.codes["MF-E1001"];
    assert_eq!(code.lint, 2);
    assert_eq!(code.same_line, 1);
    assert_eq!(code.not_reached, 0);
    assert_eq!(code.lint_only.len(), 1);
    assert_eq!(code.lint_only[0].line, 7);
    assert_eq!(
        code.game_shapes,
        BTreeMap::from([("Broken expression".to_string(), 1)])
    );
    assert_eq!(result.game_only, 1);
    assert_eq!(
        result.game_only_shapes,
        BTreeMap::from([("Unknown type '…'".to_string(), 1)])
    );
}

#[test]
fn tally_counts_a_game_line_once_per_code() {
    let lints = [
        LintLine {
            code: "MF-E1001".into(),
            path: "m/a.c".into(),
            line: 3,
            message: "Broken".into(),
        },
        LintLine {
            code: "MF-E1001".into(),
            path: "m/a.c".into(),
            line: 3,
            message: "Broken again".into(),
        },
    ];
    let game = [GameLine {
        path: "m/a.c".into(),
        line: 3,
        message: "Broken expression".into(),
    }];
    let result = tally(&lints, &game, &HashSet::new());
    let code = &result.codes["MF-E1001"];
    assert_eq!(code.same_line, 2);
    assert_eq!(
        code.game_shapes,
        BTreeMap::from([("Broken expression".to_string(), 1)])
    );
    assert_eq!(result.game_only, 0);
}

#[test]
fn tally_matches_by_file_as_well_as_line() {
    let lints = [LintLine {
        code: "MF-E1002".into(),
        path: "m/a.c".into(),
        line: 3,
        message: "twice".into(),
    }];
    let game = [GameLine {
        path: "m/other.c".into(),
        line: 3,
        message: "Multiple declaration".into(),
    }];
    let result = tally(&lints, &game, &HashSet::new());
    assert_eq!(result.codes["MF-E1002"].same_line, 0);
    assert_eq!(result.codes["MF-E1002"].lint_only.len(), 1);
    assert_eq!(result.game_only, 1);
}

#[test]
fn tally_sets_aside_lint_lines_in_files_the_game_never_compiled() {
    let line = |path: &str| LintLine {
        code: "MF-E1003".into(),
        path: path.into(),
        line: 5,
        message: "no return".into(),
    };
    let lints = [line("m/world.c"), line("m/game.c"), line("m/mission.c")];
    let game = [GameLine {
        path: "m/world.c".into(),
        line: 5,
        message: "Missing return".into(),
    }];
    let unreached = HashSet::from([PathBuf::from("m/world.c"), PathBuf::from("m/mission.c")]);
    let result = tally(&lints, &game, &unreached);
    let code = &result.codes["MF-E1003"];
    assert_eq!((code.lint, code.same_line, code.not_reached), (3, 1, 1));
    assert_eq!(code.lint_only.len(), 1);
    assert_eq!(code.lint_only[0].path, PathBuf::from("m/game.c"));
}

const GUESS_LINTS: [&str; 5] = ["MF-E1001", "MF-E1002", "MF-E1003", "MF-W3002", "MF-E3003"];

const BUILDS: [Target; 3] = [Target::RetailServer, Target::DiagServer, Target::DiagClient];

#[test]
fn game_check_compare_on_unpacked_mods() {
    let Some(mods_dir) = env_path("MF_UNPACKED_MODS") else {
        return;
    };
    let Some(vanilla) = env_path("MF_VANILLA_SCRIPTS") else {
        return;
    };
    let Some(install) = env_path("MF_DAYZ_INSTALL") else {
        return;
    };
    let folders = mod_folders(&mods_dir);
    let mod_names: Vec<String> = folders.iter().map(|dir| file_name(dir)).collect();
    let loaded: Vec<LoadedMod> = folders
        .iter()
        .zip(&mod_names)
        .map(|(dir, name)| load_mod_dir(dir, name))
        .collect();
    let addons: Vec<ModAddons> = loaded
        .iter()
        .map(|one| ModAddons {
            addons: one.addons.clone(),
            required: one.required.clone(),
        })
        .collect();
    let core = common::core();
    let mut run = Run::default();
    let mut lints = Vec::new();
    let mut game = Vec::new();
    let mut unreached = HashSet::new();
    for (index, dir) in folders.iter().enumerate() {
        if loaded[index].packs.iter().all(|pack| pack.files.is_empty()) {
            run.without_scripts += 1;
            continue;
        }
        let deps = needed_mods(&addons, index);
        let dep_dirs: Vec<PathBuf> = deps.iter().map(|&d| folders[d].clone()).collect();
        let mut mods: Vec<(String, PathBuf)> = deps
            .iter()
            .map(|&d| (mod_names[d].clone(), folders[d].clone()))
            .collect();
        mods.push((mod_names[index].clone(), dir.clone()));
        let Some(mod_lints) = lint_lines(&core, dir, &dep_dirs, &install, &mut run) else {
            run.no_lint_result += 1;
            continue;
        };
        run.checked += 1;
        lints.extend(mod_lints);
        let side = game_side(&vanilla, &mods, dir);
        run.module_failed += usize::from(!side.unreached.is_empty());
        run.shared_game_paths += usize::from(side.shared_game_paths > 0);
        game.extend(side.lines);
        unreached.extend(side.unreached);
    }
    print_tally(&tally(&lints, &game, &unreached), &run);
    assert!(
        run.checked > 0,
        "no mod with scripts under {}",
        mods_dir.display()
    );
}

/// What the run met besides the lines it compares; each can skew the table.
#[derive(Default)]
struct Run {
    checked: usize,
    without_scripts: usize,
    no_lint_result: usize,
    /// Mods where the game gave up on a module, so later files were never compiled.
    module_failed: usize,
    /// Mods where two loaded files have one game path, so a finding can land on the wrong one.
    shared_game_paths: usize,
    /// Mods whose index could not be built: their unknown-type lines are missing.
    index_failed: usize,
    index_warnings: Vec<String>,
}

/// The folder a variable names, resolved, so both sides build their paths from one spelling.
fn env_path(name: &str) -> Option<PathBuf> {
    let Some(value) = std::env::var_os(name) else {
        eprintln!("skipped: {name} not set");
        return None;
    };
    let path = PathBuf::from(value);
    Some(fs::canonicalize(&path).unwrap_or_else(|e| panic!("{name} {}: {e}", path.display())))
}

fn mod_folders(dir: &Path) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    folders
}

fn file_name(dir: &Path) -> String {
    dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

struct GameSide {
    lines: Vec<GameLine>,
    unreached: Vec<PathBuf>,
    shared_game_paths: usize,
}

/// The game check's lines in one mod's own files, and the own files it never compiled.
fn game_side(vanilla: &Path, mods: &[(String, PathBuf)], own: &Path) -> GameSide {
    let mut loaded = load_input(Target::RetailServer, Some(vanilla), mods);
    let results: Vec<(Target, CheckResult)> = BUILDS
        .into_iter()
        .map(|target| {
            loaded.input.target = target;
            (target, check(&loaded.input))
        })
        .collect();
    let (findings, failed) = merge_findings(&results, &loaded.input.files, &loaded.disk);
    // The game stops at the first module with an error, in any build.
    let module_of = |layer: Layer| enforce_check::Module::from(layer).as_str();
    let first_failed = Layer::ALL.iter().position(|&layer| {
        failed
            .iter()
            .any(|module| module.module == module_of(layer))
    });
    let files = loaded.input.files.iter().zip(&loaded.disk);
    let unreached = files
        .clone()
        .filter(|(file, disk)| {
            let at = Layer::ALL.iter().position(|&layer| layer == file.layer);
            first_failed.is_some_and(|failed| at >= Some(failed)) && disk.starts_with(own)
        })
        .map(|(_, disk)| disk.clone())
        .collect();
    let mut disks_of: HashMap<&str, HashSet<&Path>> = HashMap::new();
    for (file, disk) in files {
        disks_of.entry(&file.path).or_default().insert(disk);
    }
    GameSide {
        lines: findings
            .into_iter()
            .filter(|finding| finding.report.path.starts_with(own))
            .map(|finding| GameLine {
                path: finding.report.path,
                line: finding.line,
                message: finding.report.diagnostic.message,
            })
            .collect(),
        unreached,
        shared_game_paths: disks_of.values().filter(|disks| disks.len() > 1).count(),
    }
}

/// The guess lints' findings in one mod folder, as `mf lint` gets them.
fn lint_lines(
    core: &Core,
    dir: &Path,
    deps: &[PathBuf],
    install: &Path,
    seen: &mut Run,
) -> Option<Vec<LintLine>> {
    // As `mf lint` does: without an index only the unknown-type lines are lost.
    match run(
        core,
        "script.index",
        json!({ "root": dir, "dependencies": deps, "game": install }),
    ) {
        Ok(report) => {
            let warnings = report["warnings"].as_array().into_iter().flatten();
            seen.index_warnings
                .extend(warnings.filter_map(|warning| warning.as_str().map(str::to_owned)));
        }
        Err(e) => {
            seen.index_failed += 1;
            eprintln!("warning: no script index for {}: {e}", dir.display());
        }
    }
    let findings = run(core, "script.diagnostics", json!({ "path": dir })).and_then(|value| {
        serde_json::from_value::<Vec<FindingReport>>(value).map_err(|e| e.to_string())
    });
    let findings = match findings {
        Ok(findings) => findings,
        Err(e) => {
            eprintln!("skipped {}, no lint result: {e}", dir.display());
            return None;
        }
    };
    let mut texts: HashMap<PathBuf, String> = HashMap::new();
    Some(
        findings
            .into_iter()
            .filter(|finding| GUESS_LINTS.contains(&finding.diagnostic.code.as_str()))
            .map(|finding| {
                let line = finding.diagnostic.span.map_or(0, |span| {
                    let text = texts
                        .entry(finding.path.clone())
                        .or_insert_with(|| read_lossy(&finding.path));
                    line_col(text, span.start).0 as u32
                });
                LintLine {
                    code: finding.diagnostic.code,
                    path: finding.path,
                    line,
                    message: finding.diagnostic.message,
                }
            })
            .collect(),
    )
}

fn read_lossy(path: &Path) -> String {
    fs::read(path).map_or_else(
        |_| String::new(),
        |bytes| String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn run(core: &Core, id: &str, args: Value) -> Result<Value, String> {
    match core.dispatch(Actor::CliSession, id, args) {
        Ok(Outcome::Data(value)) => Ok(value),
        other => Err(format!("{id} returned no data: {other:?}")),
    }
}

fn print_tally(tally: &Tally, run: &Run) {
    println!(
        "game check against the guess lints: {} mods checked, {} without scripts, {} with no lint result",
        run.checked, run.without_scripts, run.no_lint_result
    );
    println!(
        "the game gave up on a module in {} mods (lint lines in files it never compiled are \"not reached\")",
        run.module_failed
    );
    println!(
        "two files share a game path in {} mods (a finding there can land on the wrong file)",
        run.shared_game_paths
    );
    println!(
        "no script index for {} mods (their unknown-type lines are missing); {} index warnings",
        run.index_failed,
        run.index_warnings.len()
    );
    for warning in run.index_warnings.iter().take(3) {
        println!("  {warning}");
    }
    println!(
        "{:<10}{:>6}{:>11}{:>13}{:>11}",
        "code", "lint", "same line", "not reached", "lint only"
    );
    for (code, row) in &tally.codes {
        println!(
            "{code:<10}{:>6}{:>11}{:>13}{:>11}",
            row.lint,
            row.same_line,
            row.not_reached,
            row.lint_only.len()
        );
    }
    for (code, row) in &tally.codes {
        println!("{code} same line, the game's lines there (top 5):");
        print_top(&row.game_shapes, 5);
        println!("{code} lint only (first 10):");
        for lint in row.lint_only.iter().take(10) {
            println!("  {}:{}  {}", lint.path.display(), lint.line, lint.message);
        }
    }
    println!("game lines with no lint on them: {}", tally.game_only);
    print_top(&tally.game_only_shapes, 15);
}

fn print_top(shapes: &BTreeMap<String, usize>, n: usize) {
    let mut sorted: Vec<(&String, &usize)> = shapes.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (shape, count) in sorted.into_iter().take(n) {
        println!("{count:>8}  {shape}");
    }
}
