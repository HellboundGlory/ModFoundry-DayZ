use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::diag::Severity;
use crate::input::Module;

use super::{ExpectedLine, Observed, Verdict};

pub fn load_observed(path: &Path) -> HashMap<String, Observed> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    rows(&text)
        .filter_map(|columns| {
            let id = columns.first()?;
            let first_line = column(&columns, 3);
            Some((
                id.to_string(),
                Observed {
                    module: Module::from_name(column(&columns, 1)),
                    severity: severity(column(&columns, 2)),
                    first_line: (!first_line.is_empty()).then(|| first_line_of(first_line)),
                    verdict: Verdict::from_name(column(&columns, 5)),
                    recorded: true,
                },
            ))
        })
        .collect()
}

pub fn load_probe_list(path: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    rows(&text)
        .filter_map(|columns| columns.first().map(|id| id.to_string()))
        .collect()
}

pub fn load_diag(path: &Path) -> Option<Observed> {
    let text = fs::read_to_string(path).ok()?;
    let mut observed = Observed::default();
    for line in text.lines() {
        if observed.first_line.is_none()
            && let Some(expected) = game_line(line)
            && expected
                .file
                .as_deref()
                .is_some_and(|file| file.to_ascii_lowercase().starts_with("probe_"))
        {
            observed.recorded = true;
            observed.severity = severity_mark(line);
            observed.first_line = Some(expected);
        }
        if observed.module.is_none()
            && let Some(module) = failed_module(line)
        {
            observed.module = Some(module);
        }
    }
    observed.recorded.then_some(observed)
}

// What a result file holds for the probe's own pack: every line the game
// printed that points into it, in order, and the modules it gave up on.
pub struct ResultFile {
    pub lines: Vec<(Severity, ExpectedLine)>,
    pub failed: Vec<Module>,
}

pub fn load_lines(path: &Path) -> Option<ResultFile> {
    let text = fs::read_to_string(path).ok()?;
    let lines = text
        .lines()
        .filter_map(|line| {
            let expected = game_line(line)?;
            let file = expected.file.as_deref()?;
            if !file.to_ascii_lowercase().starts_with("probe_") {
                return None;
            }
            Some((severity_mark(line)?, expected))
        })
        .collect();
    let failed = text.lines().filter_map(failed_module).collect();
    Some(ResultFile { lines, failed })
}

fn rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|columns| columns.first().is_some_and(|id| !id.is_empty() && *id != "id"))
}

fn column<'a>(columns: &[&'a str], index: usize) -> &'a str {
    columns.get(index).copied().unwrap_or("").trim()
}

fn severity(name: &str) -> Option<Severity> {
    match name {
        "E" => Some(Severity::Error),
        "W" => Some(Severity::Warning),
        _ => None,
    }
}

fn severity_mark(line: &str) -> Option<Severity> {
    if line.contains("(W): ") {
        Some(Severity::Warning)
    } else if line.contains("(E): ") {
        Some(Severity::Error)
    } else {
        None
    }
}

fn first_line_of(text: &str) -> ExpectedLine {
    game_line(text).unwrap_or_else(|| ExpectedLine {
        file: None,
        line: None,
        fixme: false,
        message: text.to_string(),
    })
}

fn game_line(text: &str) -> Option<ExpectedLine> {
    let (_, after_quote) = text.split_once("@\"")?;
    let (location, message) = after_quote.split_once("\": ")?;
    let (file, line) = location.rsplit_once(',')?;
    let (fixme, message) = match message.strip_prefix("FIX-ME: ") {
        Some(message) => (true, message),
        None => (false, message),
    };
    Some(ExpectedLine {
        file: Some(file.to_string()),
        line: line.trim().parse().ok(),
        fixme,
        message: message.trim().to_string(),
    })
}

fn failed_module(line: &str) -> Option<Module> {
    let (_, after_prefix) = line.split_once("Can't compile \"")?;
    Module::from_name(after_prefix.split_once('"')?.0)
}
