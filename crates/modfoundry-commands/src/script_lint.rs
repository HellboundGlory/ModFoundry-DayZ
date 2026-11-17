//! `script.lint`: the lint findings merged with the game check's, so `mf lint` and agents get one answer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use modfoundry_core::{Command, CommandError, CommandSpec, EffectKind, Outcome};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::resolved_or_given;
use crate::registry::FindingReport;
use crate::script_check::{target_name, FailedModule, GameCheck, GameFinding, TARGETS};

/// The result of `script.lint`: the merged findings and what the game check did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptLintReport {
    pub findings: Vec<FindingReport>,
    /// Why `script.index` failed, when it did.
    pub index_error: Option<String>,
    pub game: GameReport,
    /// The saved settings the index filled in.
    pub settings_used: Vec<String>,
}

/// The game check's share of a lint: skipped with a reason, or the modules it refused and its notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameReport {
    pub skipped: Option<String>,
    pub failed_modules: Vec<FailedModule>,
    pub notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LintArgs {
    path: PathBuf,
    game: Option<PathBuf>,
    dependencies: Option<Vec<PathBuf>>,
    #[serde(default)]
    targets: Vec<String>,
    cache: Option<PathBuf>,
}

pub(crate) struct ScriptLint {
    index: Box<dyn Command>,
    check: Box<dyn Command>,
    diagnostics: Box<dyn Command>,
}

impl ScriptLint {
    pub(crate) fn new(
        index: Box<dyn Command>,
        check: Box<dyn Command>,
        diagnostics: Box<dyn Command>,
    ) -> Self {
        Self {
            index,
            check,
            diagnostics,
        }
    }

    fn lint(&self, args: &LintArgs) -> Result<ScriptLintReport, CommandError> {
        let root = index_root(&args.path);
        let dependencies = args.dependencies.as_deref();
        let index: Result<Value, CommandError> = run(
            self.index.as_ref(),
            source_args(&root, args.game.as_deref(), dependencies),
        );
        let index_error = index.as_ref().err().map(ToString::to_string);
        let settings_used = index.as_ref().map(settings_used_by).unwrap_or_default();
        let dependencies_known =
            dependencies.is_some() || settings_used.iter().any(|key| key == "dependencies");
        let findings: Vec<FindingReport> =
            run(self.diagnostics.as_ref(), json!({ "path": args.path }))?;
        let findings = hide_unknown_types(findings, dependencies_known);
        let game = self.game_check(args, &root, &index, dependencies_known);
        let mut findings = hide_game_covered(findings, &game);
        findings.extend(game.findings);
        Ok(ScriptLintReport {
            findings,
            index_error,
            game: game.report,
            settings_used,
        })
    }

    fn game_check(
        &self,
        args: &LintArgs,
        root: &Path,
        index: &Result<Value, CommandError>,
        dependencies_known: bool,
    ) -> GameLint {
        let dependencies = args.dependencies.as_deref();
        if let (Err(error), None) = (index, dependencies) {
            return GameLint::skipped(format!("no script index: {error}"));
        }
        if !dependencies_known {
            return GameLint::skipped(DEPENDENCIES_UNKNOWN.into());
        }
        let mut check_args = source_args(root, args.game.as_deref(), dependencies);
        if !args.targets.is_empty() {
            check_args["targets"] = json!(args.targets);
        }
        if let Some(cache) = &args.cache {
            check_args["cache"] = json!(cache);
        }
        match run::<GameCheck>(self.check.as_ref(), check_args) {
            Ok(check) => game_lint(check, &args.path),
            Err(error) => GameLint::skipped(error.to_string()),
        }
    }
}

impl Command for ScriptLint {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: "script.lint",
            effect: EffectKind::IndexWrite,
            summary: "Lint Enforce Script merged with the game check: the findings `mf lint` prints, with the guesses the game covers hidden.",
        }
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "a .c file or a folder to lint" },
                "game": { "type": "string", "description": "DayZ install dir (default: host discovery)" },
                "dependencies": { "type": "array", "items": { "type": "string" }, "description": "folders of the mods it needs, as source trees or packed or unpacked mods; given as [] it means none; left out, the saved dependency mods of the project" },
                "targets": { "type": "array", "items": { "type": "string", "enum": TARGETS.map(target_name) }, "description": "the game builds to check; all three when left out" },
                "cache": { "type": "string", "description": "folder for unpacked copies (default: mf's cache folder)" },
            },
            "required": ["path"],
        })
    }

    fn quiet(&self) -> bool {
        true
    }

    fn runs(&self) -> &'static [&'static str] {
        &["script.index", "script.diagnostics", "script.check"]
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        let args: LintArgs = serde_json::from_value(args)
            .map_err(|e| CommandError::InvalidArguments(e.to_string()))?;
        let report = self.lint(&args)?;
        let value =
            serde_json::to_value(report).map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok(Outcome::Data(value))
    }
}

const DEPENDENCIES_UNKNOWN: &str = "the mod's dependencies are not known; save them with mf deps add (MCP: project.edit_dependencies), or pass --dependency or --no-dependencies";

/// Runs a sibling command's logic and decodes its data.
fn run<T: DeserializeOwned>(command: &dyn Command, args: Value) -> Result<T, CommandError> {
    match command.execute(args)? {
        Outcome::Data(value) => {
            serde_json::from_value(value).map_err(|e| CommandError::Failed(e.to_string()))
        }
        other => Err(CommandError::Failed(format!(
            "unexpected outcome {other:?}"
        ))),
    }
}

/// The arguments `script.index` and `script.check` share.
fn source_args(root: &Path, game: Option<&Path>, dependencies: Option<&[PathBuf]>) -> Value {
    let mut args = json!({ "root": root });
    if let Some(game) = game {
        args["game"] = json!(game);
    }
    if let Some(dependencies) = dependencies {
        args["dependencies"] = json!(dependencies);
    }
    args
}

fn settings_used_by(index: &Value) -> Vec<String> {
    index["settings_used"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

struct GameLint {
    /// The folder the game check read, when it ran: a packed mod's unpacked copy, else the target.
    checked_under: Option<PathBuf>,
    unchecked: HashSet<PathBuf>,
    findings: Vec<FindingReport>,
    report: GameReport,
}

impl GameLint {
    fn skipped(reason: String) -> Self {
        Self {
            checked_under: None,
            unchecked: HashSet::new(),
            findings: Vec::new(),
            report: GameReport {
                skipped: Some(reason),
                failed_modules: Vec::new(),
                notes: Vec::new(),
            },
        }
    }
}

fn game_lint(check: GameCheck, target: &Path) -> GameLint {
    let checked_under = resolved_or_given(check.checked_from.as_deref().unwrap_or(target));
    let findings = narrow_game_findings(check.findings, &checked_under)
        .into_iter()
        .map(|finding| {
            let mut report = finding.report;
            report.path = shown_under(&report.path, target);
            if finding.targets.len() < check.targets.len() {
                report.diagnostic.message = format!(
                    "{} [{}]",
                    report.diagnostic.message,
                    finding.targets.join(", ")
                );
            }
            report
        })
        .collect();
    GameLint {
        checked_under: Some(checked_under),
        unchecked: check
            .unchecked
            .iter()
            .map(|path| resolved_or_given(path))
            .collect(),
        findings,
        report: GameReport {
            skipped: None,
            failed_modules: check.failed_modules,
            notes: check.notes,
        },
    }
}

const UNKNOWN_TYPE: &str = "MF-W3002";

// The game check reports these exactly, so the lint's guess is dropped for every file the game looked at.
const GAME_COVERED_CODES: [&str; 6] = [
    "MF-E1001",
    "MF-E1002",
    "MF-E1003",
    "MF-E3001",
    "MF-E3003",
    UNKNOWN_TYPE,
];

fn hide_game_covered(findings: Vec<FindingReport>, game: &GameLint) -> Vec<FindingReport> {
    let Some(checked_under) = &game.checked_under else {
        return findings;
    };
    findings
        .into_iter()
        .filter(|finding| {
            let path = resolved_or_given(&finding.path);
            !GAME_COVERED_CODES.contains(&finding.diagnostic.code.as_str())
                || !path.starts_with(checked_under)
                || game.unchecked.contains(&path)
        })
        .collect()
}

/// Drops the unknown-type warnings unless the index knew the project's dependencies, since
/// without them every type a dependency defines looks unknown.
fn hide_unknown_types(
    findings: Vec<FindingReport>,
    dependencies_known: bool,
) -> Vec<FindingReport> {
    if dependencies_known {
        return findings;
    }
    findings
        .into_iter()
        .filter(|finding| finding.diagnostic.code != UNKNOWN_TYPE)
        .collect()
}

/// The root to index for a lint target: the nearest folder holding `config.cpp`, else above the
/// module folder, else the target's own folder.
fn index_root(target: &Path) -> PathBuf {
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let start = if target.is_dir() {
        target.as_path()
    } else {
        target.parent().unwrap_or(&target)
    };
    let holding_config = start
        .ancestors()
        .find(|dir| dir.join("config.cpp").is_file());
    // A file's module comes from its path under the root, so the root sits above the module folder.
    let above_module = || {
        start
            .ancestors()
            .find(|dir| is_module_folder(dir))
            .and_then(Path::parent)
    };
    holding_config
        .or_else(above_module)
        .unwrap_or(start)
        .to_path_buf()
}

fn is_module_folder(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            ["1_core", "2_gamelib", "3_game", "4_world", "5_mission"].contains(&name.as_str())
        })
}

fn narrow_game_findings(findings: Vec<GameFinding>, target: &Path) -> Vec<GameFinding> {
    let target = resolved_or_given(target);
    findings
        .into_iter()
        .filter(|finding| resolved_or_given(&finding.report.path).starts_with(&target))
        .collect()
}

/// `path` written under `target` as the user gave it, so game findings read like the lint's own.
fn shown_under(path: &Path, target: &Path) -> PathBuf {
    match resolved_or_given(path).strip_prefix(resolved_or_given(target)) {
        Ok(rest) if rest.as_os_str().is_empty() => target.to_path_buf(),
        Ok(rest) => target.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_findings_are_narrowed_to_the_path_given() {
        let base = std::env::temp_dir().join(format!("mf-narrow-{}", std::process::id()));
        let file = base.join("m/Scripts/3_Game/a.c");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(base.join("m/Scripts/4_World")).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        std::fs::write(&file, "").unwrap();
        let finding: GameFinding = serde_json::from_value(json!({
            "code": "MF-E4001",
            "severity": "error",
            "message": "m",
            "path": file,
            "line": 1,
            "targets": ["retail-server"],
        }))
        .unwrap();
        let kept =
            |target: PathBuf| !narrow_game_findings(vec![finding.clone()], &target).is_empty();
        let results = [
            kept(base.join("m")),
            kept(base.join("m/Scripts")),
            kept(file.clone()),
            kept(base.join("m/Scripts/4_World")),
            kept(base.join("other")),
        ];
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(results, [true, true, true, false, false]);
    }

    #[test]
    fn unknown_type_warnings_are_hidden_unless_the_dependencies_are_known() {
        let finding = |code: &str, severity: &str| {
            serde_json::from_value::<FindingReport>(json!({
                "code": code,
                "severity": severity,
                "message": "m",
                "path": "/mods/A/Scripts/3_Game/a.c",
            }))
            .unwrap()
        };
        let findings = vec![finding("MF-W3002", "warning"), finding("MF-E3003", "error")];
        let cases: [(bool, &[&str]); 2] =
            [(false, &["MF-E3003"]), (true, &["MF-W3002", "MF-E3003"])];
        for (dependencies_known, wanted) in cases {
            let kept = hide_unknown_types(findings.clone(), dependencies_known);
            let codes: Vec<&str> = kept
                .iter()
                .map(|finding| finding.diagnostic.code.as_str())
                .collect();
            assert_eq!(codes, wanted, "dependencies_known {dependencies_known}");
        }
    }
}
