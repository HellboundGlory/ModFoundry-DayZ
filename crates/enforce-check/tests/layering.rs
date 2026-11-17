use std::path::{Path, PathBuf};

// The two modules that read files. Every other source file is the core, so a
// new module is held to the rule until it is named here.
const EDGE: [&str; 2] = ["loader", "probes"];

// The parts of the standard library that touch the machine.
const MACHINE: [&str; 6] = ["fs", "path", "env", "process", "net", "os"];

// The stages in the order they run. Each may name only the ones before it, and
// the plain data they share names none.
const STAGES: [&str; 4] = ["preproc", "parser", "index", "bodies"];
const PLAIN_DATA: [&str; 3] = ["diag", "input", "target"];

// The stages a file may not import.
fn closed_stages(file: &str) -> &'static [&'static str] {
    let module = module_of(file);
    match STAGES.iter().position(|stage| *stage == module) {
        Some(at) => &STAGES[at + 1..],
        None if PLAIN_DATA.contains(&module) => &STAGES,
        None => &[],
    }
}

// The module a source file belongs to: its first folder, or its own name.
fn module_of(file: &str) -> &str {
    let first = file.split('/').next().unwrap_or(file);
    first.strip_suffix(".rs").unwrap_or(first)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|word| !word.is_empty())
}

// What follows each `prefix` in the text: a name, or `{` where a group opens.
fn after<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    let rests = text.match_indices(prefix).map(|(at, _)| &text[at + prefix.len()..]);
    rests.map(|rest| if rest.starts_with('{') { "{" } else { words(rest).next().unwrap_or_default() }).collect()
}

// The code of a file, comments dropped, with the line each piece starts on. A
// `use` that runs over several lines is one piece, so a group is read whole.
fn statements(text: &str) -> Vec<(usize, String)> {
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut open = false;
    for (number, line) in text.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default().trim();
        if code.is_empty() {
            continue;
        }
        match found.last_mut() {
            Some((_, statement)) if open => statement.push_str(code),
            _ => found.push((number + 1, code.to_string())),
        }
        let starts_use = code.starts_with("use ") || (code.starts_with("pub") && code.contains(" use "));
        open = (open || starts_use) && !code.contains(';');
    }
    found
}

fn breach(statement: &str, closed: &[&str]) -> Option<String> {
    let names: Vec<&str> = words(statement).collect();
    for edge in EDGE {
        if names.contains(&edge) && statement != format!("pub mod {edge};") {
            return Some(format!("names `{edge}`"));
        }
    }
    let later = after(statement, "crate::").into_iter().find_map(|next| match next {
        "{" => closed.iter().find(|stage| names.contains(stage)),
        _ => closed.iter().find(|stage| **stage == next),
    });
    if let Some(stage) = later {
        return Some(format!("imports the later stage `{stage}`"));
    }
    let machine = after(statement, "std::").into_iter().find_map(|next| match next {
        "{" => MACHINE.into_iter().find(|part| names.contains(part)),
        _ => MACHINE.into_iter().find(|part| *part == next),
    });
    machine.map(|part| format!("uses `std::{part}`"))
}

fn breaches(file: &str, text: &str) -> Vec<String> {
    let closed = closed_stages(file);
    let found = statements(text).into_iter().filter_map(|(number, statement)| Some((number, breach(&statement, closed)?)));
    found.map(|(number, what)| format!("{file}:{number}: {what}")).collect()
}

fn sources(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            sources(&entry.unwrap().path(), out);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        out.push(path.to_path_buf());
    }
}

#[test]
fn the_core_reads_no_files_and_its_stages_point_one_way() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    files.sort();
    let mut scanned = 0;
    let mut found = Vec::new();
    for file in files {
        let name = file.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if EDGE.contains(&module_of(&name)) {
            continue;
        }
        scanned += 1;
        found.extend(breaches(&name, &std::fs::read_to_string(&file).unwrap()));
    }
    assert!(scanned > STAGES.len() + PLAIN_DATA.len(), "scanned only {scanned} files under {}", src.display());
    assert!(found.is_empty(), "\n{}", found.join("\n"));
}

#[test]
fn the_scan_sees_through_groups_aliases_and_comments() {
    let refused = [
        "use std::{fs, path::PathBuf};",
        "use std::{\n    collections::HashMap,\n    fs,\n};",
        "use std::os::unix::fs::MetadataExt;",
        "let text = std::fs::read(path);",
        "use crate::loader as disk;",
        "use crate::{diag, loader};",
        "use crate::{\n    diag,\n    loader,\n};",
        "use super::probes;",
        "pub use loader::load_input;",
    ];
    for text in refused {
        assert!(!breaches("lib.rs", text).is_empty(), "passed: {text}");
    }
    let allowed = [
        "// never calls loader::load_tree or std::fs",
        "pub mod loader;",
        "use std::collections::{HashMap, HashSet};",
        "use std::{cmp::Ordering, fmt};",
        "let reloader = downloader::get();",
    ];
    for text in allowed {
        assert_eq!(breaches("lib.rs", text), Vec::<String>::new(), "refused: {text}");
    }
}

#[test]
fn the_scan_holds_each_stage_to_the_stages_before_it() {
    let refused = [
        ("preproc/lexer.rs", "use crate::parser::Node;"),
        ("parser/expr.rs", "use crate::{diag, index};"),
        ("parser/expr.rs", "use crate::{\n    diag,\n    index::Known,\n};"),
        ("index/merge.rs", "let world = crate::bodies::World::default();"),
        ("diag.rs", "use crate::parser::Node;"),
    ];
    for (file, text) in refused {
        assert!(!breaches(file, text).is_empty(), "passed in {file}: {text}");
    }
    let allowed = [
        ("parser/expr.rs", "use crate::preproc::Token;"),
        ("bodies/calls.rs", "use crate::index::Known;"),
        ("index/merge.rs", "let index = files.len();"),
        ("lib.rs", "use crate::bodies::World;"),
    ];
    for (file, text) in allowed {
        assert_eq!(breaches(file, text), Vec::<String>::new(), "refused in {file}: {text}");
    }
}
