//! Enforce linting with IO at the boundary (directory walk / file read).
//!
//! Parser is pure ([`parse_tree`]); module classification is pure ([`detect`]);
//! this module is the only part that touches the filesystem.

use crate::detect::{classify, FileClass};
use crate::parse::{parse_tree, syntax_diagnostics};
use crate::preproc::first_arms;
use crate::{Diagnostic, Severity, Tree};
use std::path::{Path, PathBuf};

/// `.c` outside the standard module folders, so it compiles only when
/// config.cpp `CfgMods` `files[]` lists its folder.
pub const E_UNBOUND: &str = "MF-E2001";

/// One analyzer finding tied to its source file (the shared [`Diagnostic`]
/// shape carries no path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    pub diagnostic: Diagnostic,
}

/// Lint `target` — a single `.c` file or a directory tree. `.c` files inside a
/// `Scripts/N_*` module dir are checked; stray `.c` files are flagged
/// `MF-E2001`. Non-`.c` files are ignored.
pub fn lint_target(target: &Path) -> std::io::Result<Vec<Finding>> {
    lint_target_with(target, &mut |_, _, _| Vec::new())
}

/// [`lint_target`] with `extra` run on every linted file's parse, so a caller
/// can add diagnostics from the tree the lint already built.
pub fn lint_target_with(
    target: &Path,
    extra: &mut dyn FnMut(&Path, &Tree, &str) -> Vec<Diagnostic>,
) -> std::io::Result<Vec<Finding>> {
    let mut findings = Vec::new();
    if target.is_file() {
        lint_file(target, &mut findings, extra)?;
    } else {
        walk(target, &mut findings, extra)?;
    }
    Ok(findings)
}

/// Recursively classify and lint every `.c` under `dir`.
fn walk(
    dir: &Path,
    out: &mut Vec<Finding>,
    extra: &mut dyn FnMut(&Path, &Tree, &str) -> Vec<Diagnostic>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                walk(&path, out, extra)?;
            }
        } else if file_type.is_file() {
            lint_file(&path, out, extra)?;
        }
    }
    Ok(())
}

fn lint_file(
    path: &Path,
    out: &mut Vec<Finding>,
    extra: &mut dyn FnMut(&Path, &Tree, &str) -> Vec<Diagnostic>,
) -> std::io::Result<()> {
    if classify(path) != FileClass::NotEnforce {
        let text = read_source(path)?;
        let source = first_arms(&text);
        let tree = parse_tree(&source);
        out.extend(
            lint_text(path, &tree, &source)
                .into_iter()
                .chain(extra(path, &tree, &source))
                .map(|diagnostic| Finding {
                    path: path.to_path_buf(),
                    diagnostic,
                }),
        );
    }
    Ok(())
}

/// Read `path` as lossy UTF-8: sources often carry Windows-1252 bytes in comments.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The diagnostics of one parsed file: syntax findings, duplicate locals
/// (`MF-E1002`), missing final returns (`MF-E1003`), `foreach` loops straight
/// over a call (`MF-W3005`) plus `MF-E2001` when `path` is not under a standard
/// module dir.
/// Pure — `tree` is `text`'s parse, `path` is used only for classification.
pub fn lint_text(path: &Path, tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut diagnostics = syntax_diagnostics(tree, text);
    diagnostics.extend(crate::locals::duplicate_locals(tree, text));
    diagnostics.extend(crate::returns::missing_returns(tree, text));
    diagnostics.extend(crate::loops::loops_over_calls(tree, text));
    if classify(path) == FileClass::Unbound {
        diagnostics.push(Diagnostic::new(
            E_UNBOUND,
            Severity::Warning,
            "this file is outside the standard script folders (`Scripts/1_Core` to `Scripts/5_Mission`, `Scripts/editor`); the engine compiles it only if config.cpp `CfgMods` lists its folder in `files[]`",
        ));
    }
    diagnostics
}
