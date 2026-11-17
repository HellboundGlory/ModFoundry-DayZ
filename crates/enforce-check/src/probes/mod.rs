mod report;
mod table;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use report::{Report, ScoreRow, Totals};
pub use table::load_observed;
pub use crate::loader::load_tree;

use crate::check;
use crate::diag::{CheckResult, Diagnostic, Severity};
use crate::input::{CheckInput, Layer, ModInfo, Module, SourceFile};
use crate::target::Target;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Mismatch,
    Open,
    Crash,
}

impl Verdict {
    fn from_name(name: &str) -> Option<Verdict> {
        match name {
            "OK" => Some(Verdict::Ok),
            "MISMATCH" => Some(Verdict::Mismatch),
            "OPEN" => Some(Verdict::Open),
            "CRASH" => Some(Verdict::Crash),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpectedLine {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub fixme: bool,
    pub message: String,
}

impl ExpectedLine {
    pub fn to_text(&self) -> String {
        let message = match self.fixme {
            true => format!("FIX-ME: {}", self.message),
            false => self.message.clone(),
        };
        match (&self.file, self.line) {
            (Some(file), Some(line)) => format!("@\"{file},{line}\": {message}"),
            _ => message,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Observed {
    pub module: Option<Module>,
    pub severity: Option<Severity>,
    pub first_line: Option<ExpectedLine>,
    pub verdict: Option<Verdict>,
    pub recorded: bool,
}

impl Observed {
    pub fn is_crash(&self) -> bool {
        self.verdict == Some(Verdict::Crash)
    }

    pub fn expected_text(&self) -> String {
        if let Some(first_line) = &self.first_line {
            return first_line.to_text();
        }
        if !self.recorded {
            return "no verdict".to_string();
        }
        if self.is_crash() {
            return "game crash".to_string();
        }
        "compiles".to_string()
    }

    pub fn agrees(&self, got: Option<&Diagnostic>) -> bool {
        if !self.recorded {
            return false;
        }
        if self.is_crash() {
            return got.is_some_and(|got| got.game_crash);
        }
        let Some(first_line) = &self.first_line else {
            return got.is_none();
        };
        let Some(got) = got else {
            return false;
        };
        fold_message(&got.message) == fold_message(&first_line.message)
            && got.fixme == first_line.fixme
            && first_line
                .file
                .as_ref()
                .is_none_or(|file| same_path(file, &got.file))
            && first_line.line.is_none_or(|line| line == got.line)
            && self.severity.is_none_or(|severity| severity == got.severity)
            && self.module.is_none_or(|module| module == got.module)
    }
}

#[derive(Clone, Debug)]
pub struct ProbeCase {
    pub id: String,
    pub cfg_class: String,
    pub files: Vec<SourceFile>,
    pub observed: Observed,
    pub diag: Option<Observed>,
}

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub id: String,
    pub agree: bool,
    pub expected: Observed,
    pub got: Option<Diagnostic>,
}

pub fn load_probe(dir: &Path) -> ProbeCase {
    let observed = dir
        .parent()
        .and_then(Path::parent)
        .map(|base| load_observed(&base.join("observed.tsv")))
        .unwrap_or_default();
    probe_at(dir, &observed)
}

fn probe_at(dir: &Path, observed: &HashMap<String, Observed>) -> ProbeCase {
    let id = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut files = load_tree(&dir.join("Scripts"), Layer::from_folder_name);
    // The game ran each probe as a pack whose prefix is its config class, and
    // prints a file by its path inside the pack.
    let cfg_class = format!("Probe_{}", id.replace('-', "_"));
    for file in &mut files {
        file.mod_index = 1;
        file.path = format!("{cfg_class}/{}", file.path);
    }
    let diag = dir
        .parent()
        .and_then(Path::parent)
        .and_then(|base| table::load_diag(&base.join("results-diag").join(format!("{id}.txt"))));
    ProbeCase {
        cfg_class,
        observed: observed.get(&id).cloned().unwrap_or_default(),
        id,
        files,
        diag,
    }
}

pub fn load_probes(root: &Path) -> Vec<ProbeCase> {
    let observed = load_observed(&root.join("observed.tsv"));
    let mut ids = table::load_probe_list(&root.join("probes.tsv"));
    for dir in directories(&root.join("probes")) {
        let Some(id) = dir.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            continue;
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.iter()
        .map(|id| root.join("probes").join(id))
        .filter(|dir| dir.is_dir())
        .map(|dir| probe_at(&dir, &observed))
        .collect()
}

pub fn score(root: &Path, vanilla: &[SourceFile], target: Target, filter: Option<&str>) -> Report {
    let probes: Vec<ProbeCase> = load_probes(root)
        .into_iter()
        .filter(|case| filter.is_none_or(|prefix| case.id.starts_with(prefix)))
        .collect();
    Report::build(target, &run_cases(&probes, vanilla, target))
}

// One probe's whole list: every line that points into the probe, as the game
// printed them and as the checker did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListResult {
    pub id: String,
    pub agree: bool,
    pub expected: Vec<String>,
    pub got: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lists {
    pub results: Vec<ListResult>,
    // Probes with no result file for the target; they are left out of `results`.
    pub without_result: Vec<String>,
}

impl Lists {
    // The probes that differ, each with the game's lines and ours, then the count.
    pub fn to_text(&self) -> String {
        let mut lines = Vec::new();
        for result in self.results.iter().filter(|result| !result.agree) {
            lines.push(result.id.clone());
            lines.extend(result.expected.iter().map(|line| format!("  game  {line}")));
            lines.extend(result.got.iter().map(|line| format!("  ours  {line}")));
            lines.push(String::new());
        }
        let agree = self.results.iter().filter(|result| result.agree).count();
        lines.push(format!("lists agree {agree}/{}", self.results.len()));
        if !self.without_result.is_empty() {
            lines.push(format!(
                "no result file, not compared ({}): {}",
                self.without_result.len(),
                self.without_result.join(" ")
            ));
        }
        lines.join("\n")
    }
}

// Compares every probe that has a result file for the target, line for line.
pub fn compare_lists(root: &Path, vanilla: &[SourceFile], target: Target, filter: Option<&str>) -> Lists {
    let folder = match target {
        Target::RetailServer => "results",
        _ => "results-diag",
    };
    let mut without_result = Vec::new();
    let mut probes: Vec<(ProbeCase, Vec<String>, bool)> = Vec::new();
    for case in load_probes(root)
        .into_iter()
        .filter(|case| filter.is_none_or(|prefix| case.id.starts_with(prefix)))
    {
        let Some(file) = table::load_lines(&root.join(folder).join(format!("{}.txt", case.id))) else {
            without_result.push(case.id);
            continue;
        };
        let expected_result = case.expected(target);
        let (crash, recorded) = (expected_result.is_crash(), expected_result.recorded);
        let mut expected: Vec<String> = file.lines.iter().map(|(severity, line)| list_line(*severity, &line.to_text())).collect();
        // A crash leaves no module line to read; the verdict says it happened.
        expected.extend(match crash {
            true => vec![CRASH_LINE.to_string()],
            false => file.failed.iter().map(|module| module_line(*module)).collect(),
        });
        probes.push((case, expected, recorded));
    }
    let results = in_parallel(&probes, |(case, expected, recorded)| {
        let result = check(&case.input(vanilla, target));
        let prefix = format!("{}/", case.cfg_class);
        let mut got: Vec<String> = result
            .diagnostics
            .iter()
            .filter(|diagnostic| !diagnostic.game_crash && diagnostic.file.starts_with(&prefix))
            .flat_map(printed_lines)
            .collect();
        if result.diagnostics.iter().any(|diagnostic| diagnostic.game_crash) {
            // A probe with no recorded verdict says nothing about a crash.
            if *recorded {
                got.push(CRASH_LINE.to_string());
            }
        } else {
            got.extend(result.failed_modules.iter().map(|module| module_line(*module)));
        }
        ListResult {
            id: case.id.clone(),
            agree: got.len() == expected.len() && got.iter().zip(expected).all(|(got, expected)| same_line(got, expected)),
            expected: expected.clone(),
            got,
        }
    });
    Lists { results, without_result }
}

const CRASH_LINE: &str = "game crash";

fn module_line(module: Module) -> String {
    format!("Can't compile \"{}\" script module!", module.as_str())
}

// The game prints a path in lower case; a name in a message keeps its case.
fn same_line(left: &str, right: &str) -> bool {
    fn fold_path(line: &str) -> String {
        let Some((head, rest)) = line.split_once("@\"") else {
            return line.to_string();
        };
        let (path, tail) = rest.split_once("\": ").unwrap_or((rest, ""));
        format!("{head}@\"{}\": {}", path.to_ascii_lowercase(), fold_message(tail))
    }
    fold_path(left) == fold_path(right)
}

// The open-string message ends in the file's path, which the game prints in
// lower case like any other path.
fn fold_message(message: &str) -> String {
    match message.split_once(", module ") {
        Some((text, file)) if text.starts_with("CParser: quoted string not closed") => {
            format!("{text}, module {}", file.to_ascii_lowercase())
        }
        _ => message.to_string(),
    }
}

// A diagnostic as the lines the game prints for it: its own, with the leading
// tab of a candidate line dropped as the result files drop it, then the place
// a duplicate was first declared.
fn printed_lines(diagnostic: &Diagnostic) -> Vec<String> {
    let own = Diagnostic {
        message: diagnostic.message.trim().to_string(),
        ..diagnostic.clone()
    };
    let mut lines = vec![list_line(own.severity, &own.to_game_line())];
    if let Some((file, line)) = &diagnostic.related {
        lines.push(list_line(Severity::Error, &format!("@\"{file},{line}\": Previously declared here")));
    }
    lines
}

fn list_line(severity: Severity, text: &str) -> String {
    let mark = match severity {
        Severity::Error => 'E',
        Severity::Warning => 'W',
    };
    format!("{mark} {text}")
}

const THREAD_STACK: usize = 16 * 1024 * 1024;

// Each probe is checked on its own against the whole of vanilla, so the cases
// run side by side. The results keep the probes' order.
fn run_cases(probes: &[ProbeCase], vanilla: &[SourceFile], target: Target) -> Vec<CaseResult> {
    in_parallel(probes, |case| run_case(case, vanilla, target))
}

fn in_parallel<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    let share = items.len().div_ceil(threads).max(1);
    let work = &work;
    std::thread::scope(|scope| {
        let running: Vec<_> = items
            .chunks(share)
            .map(|chunk| {
                let work = move || chunk.iter().map(work).collect::<Vec<_>>();
                // The checker recurses as deep as a script nests, which the
                // main thread's stack allows and a default thread's may not.
                std::thread::Builder::new().stack_size(THREAD_STACK).spawn_scoped(scope, work)
            })
            .collect();
        running
            .into_iter()
            .flat_map(|thread| match thread.map(|thread| thread.join()) {
                Ok(Ok(results)) => results,
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(error) => panic!("could not start a probe thread: {error}"),
            })
            .collect()
    })
}

fn prefix_of(id: &str) -> &str {
    id.split_once('-').map_or(id, |(prefix, _)| prefix)
}

impl ProbeCase {
    fn input(&self, vanilla: &[SourceFile], target: Target) -> CheckInput {
        CheckInput {
            target,
            mods: vec![ModInfo {
                name: self.id.clone(),
                cfg_class: self.cfg_class.clone(),
            }],
            files: layered_files(vanilla, &self.files),
            extra_defines: Vec::new(),
            input_names: Vec::new(),
        }
    }

    pub fn expected(&self, target: Target) -> Observed {
        match target {
            Target::RetailServer => self.observed.clone(),
            _ => self.diag.clone().unwrap_or_default(),
        }
    }
}

pub fn run_case(case: &ProbeCase, vanilla: &[SourceFile], target: Target) -> CaseResult {
    let expected = case.expected(target);
    let got = match expected.recorded {
        true => check_case(case, vanilla, target, &expected),
        false => None,
    };
    CaseResult {
        id: case.id.clone(),
        agree: expected.agrees(got.as_ref()),
        expected,
        got,
    }
}

fn check_case(
    case: &ProbeCase,
    vanilla: &[SourceFile],
    target: Target,
    expected: &Observed,
) -> Option<Diagnostic> {
    reported(&check(&case.input(vanilla, target)), expected)
}

fn reported(result: &CheckResult, expected: &Observed) -> Option<Diagnostic> {
    if expected.is_crash()
        && let Some(crash) = result.diagnostics.iter().find(|diagnostic| diagnostic.game_crash)
    {
        return Some(crash.clone());
    }
    result.diagnostics.first().cloned()
}

fn layered_files(vanilla: &[SourceFile], probe: &[SourceFile]) -> Vec<SourceFile> {
    let mut files = Vec::with_capacity(vanilla.len() + probe.len());
    for layer in Layer::ALL {
        files.extend(vanilla.iter().filter(|file| file.layer == layer).cloned());
        files.extend(probe.iter().filter(|file| file.layer == layer).cloned());
    }
    files
}

fn same_path(left: &str, right: &str) -> bool {
    from_scripts(left).eq_ignore_ascii_case(from_scripts(right))
}

fn from_scripts(path: &str) -> &str {
    let lowered = path.to_ascii_lowercase();
    match lowered.find("scripts/") {
        Some(start) => &path[start..],
        None => path,
    }
}

fn directories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}
