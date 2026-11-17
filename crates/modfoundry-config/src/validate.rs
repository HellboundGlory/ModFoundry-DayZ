//! config.cpp validation → [`Diagnostic`]s.
//!
//! Codes (config.cpp owns `MF-C1xx`..`MF-C4xx`):
//! - `MF-C100` — lexical error (unterminated string/comment).
//! - `MF-C101` — syntax error (bad member shape, unbalanced braces).
//! - `MF-C200` — config is not a DayZ *mod* config: missing `CfgPatches`
//!   and/or `CfgMods` where a mod config requires them.
//! - `MF-C220` — a script module declares no `files[]` (nothing to bind).
//! - `MF-C300` — `files[]` entry does not end in a canonical script-module dir
//!   (`1_Core`..`5_Mission`) and no such folder exists in the mod, i.e. it won't
//!   resolve to a loadable module layer.
//! - `MF-C310` — `files[]` leaf module (e.g. `4_World`) mismatches its `defs`
//!   slot (`worldScriptModule` → must be `4_World`). Wrong-folder class bug.
//! - `MF-C320` — a `files[]` directory does not exist under the source root
//!   (only checked when a source root is supplied).
//!
//! File lints ([`crate::lints`], run only on a clean parse):
//! - `MF-C330` — a property is set twice in one class body.
//! - `MF-C331` — a child class is defined twice in one parent body.
//! - `MF-C340` — a class's base is not declared in scope.
//! - `MF-C341` — the base's casing differs from its declaration.
//! - `MF-C350` — a known property's array-ness disagrees with the schema.
//!
//! The two module-path checks (`MF-C300`/`MF-C310`) are the "catch it before
//! the game silently loads nothing" value.

use crate::ast::{ConfigFile, Member};
use crate::model::{DayZConfig, ModuleSlot, ScriptModule};
use crate::parser::parse;
use modfoundry_core::{Diagnostic, Severity, Span};
use std::path::Path;

/// Canonical script module directory names, engine order.
const CANONICAL_MODULE_DIRS: [&str; 5] = ["1_Core", "2_GameLib", "3_Game", "4_World", "5_Mission"];

/// Result of validating one config source.
#[derive(Debug, Default)]
pub struct Validation {
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse and validate `text` (the contents of a `config.cpp`).
///
/// `source_root` is optional: when given (the directory the config's module
/// tree lives under), `files[]` entries are additionally checked to exist.
/// Existence checking is deliberately tolerant of prefix nesting: a config's
/// declared virtual path may include a prefix like `JM/CF/...` that the root
/// does not repeat, so the entry is resolved against the root by suffix.
pub fn validate_text(text: &str, source_root: Option<&Path>) -> Validation {
    let mut v = Validation::default();
    let parsed = parse(text);
    for diag in parsed.diagnostics() {
        v.diagnostics.push(diag);
    }
    if !parsed.issues.is_empty() {
        return v;
    }
    let dz = DayZConfig::from_file(&parsed.file);
    validate_dayz(&parsed.file, &dz, source_root, &mut v);
    v.diagnostics.extend(crate::lints::run(&parsed.file, text));
    v
}

/// Build a diagnostic, dropping the span when the source position is unknown.
fn at(code: &str, severity: Severity, span: Option<Span>, message: String) -> Diagnostic {
    match span {
        Some(span) => Diagnostic::at(code, severity, span, message),
        None => Diagnostic::new(code, severity, message),
    }
}

fn first_top_level_span(file: &ConfigFile) -> Option<Span> {
    file.members.iter().find_map(|m| match m {
        Member::Class(c) => Some(c.spans.name),
        _ => None,
    })
}

fn validate_dayz(
    file: &ConfigFile,
    dz: &DayZConfig,
    source_root: Option<&Path>,
    v: &mut Validation,
) {
    let has_cfgpatches = file.find_class("CfgPatches").is_some();
    let has_cfgmods = file.find_class("CfgMods").is_some();
    let top_span = first_top_level_span(file);

    if has_cfgpatches && has_cfgmods {
        // Canonical mod config. Nothing extra required structurally.
    } else if has_cfgmods && dz.mods.is_empty() {
        v.diagnostics.push(at(
            "MF-C200",
            Severity::Error,
            top_span,
            "CfgMods class exists but declares no mod entries; DayZ will bind nothing from this config".into(),
        ));
    } else if has_cfgmods && dz.patches.is_empty() {
        v.diagnostics.push(at(
            "MF-C200",
            Severity::Error,
            top_span,
            "CfgMods declares mod(s) but the config has no CfgPatches entry; DayZ will not treat this file as a loadable addon config".into(),
        ));
    } else if !has_cfgpatches && !has_cfgmods {
        v.diagnostics.push(at(
            "MF-C200",
            Severity::Error,
            top_span,
            "config declares neither CfgPatches nor CfgMods — not a valid DayZ config.cpp".into(),
        ));
    }

    for m in &dz.mods {
        for sm in &m.script_modules {
            validate_module(sm, source_root, v);
        }
    }
}

fn validate_module(sm: &ScriptModule, source_root: Option<&Path>, v: &mut Validation) {
    let slot = sm.slot;
    let defs_name = sm.defs_name.as_str();
    if sm.files.is_empty() {
        v.diagnostics.push(at(
            "MF-C220",
            Severity::Error,
            Some(sm.span),
            format!("`{defs_name}` declares no files[]; the module will bind no source"),
        ));
        return;
    }
    for (i, path) in sm.files.iter().enumerate() {
        let span = sm.file_spans.get(i).copied();
        check_module_path(slot, defs_name, path, span, source_root, v);
    }
}

fn check_module_path(
    slot: ModuleSlot,
    defs_name: &str,
    path: &str,
    span: Option<Span>,
    source_root: Option<&Path>,
    v: &mut Validation,
) {
    // Virtual paths use `/`. Reject Windows/backslash forms that would never
    // resolve in the engine's module model.
    if path.contains('\\') {
        v.diagnostics.push(at(
            "MF-C300",
            Severity::Error,
            span,
            format!("module path `{path}` uses backslashes; DayZ module paths must use `/`"),
        ));
    }
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    let leaf = trimmed.rsplit('/').next().unwrap_or(trimmed);

    let is_canonical = CANONICAL_MODULE_DIRS.contains(&leaf);
    let folder_found = source_root.is_some_and(|root| module_dir_exists(root, trimmed));
    if is_canonical {
        // Slot↔leaf consistency (only meaningful when leaf is canonical).
        let expected = slot.canonical_dir();
        if leaf != expected {
            v.diagnostics.push(at(
                "MF-C310",
                Severity::Error,
                span,
                format!(
                    "`{defs_name}` binds module `{leaf}` but slot expects `{expected}` \
                     (files[] points at the wrong DayZ module folder — scripts will not load)"
                ),
            ));
        }
    } else if !folder_found {
        v.diagnostics.push(at(
            "MF-C300",
            Severity::Warning,
            span,
            format!(
                "module path `{path}` does not end in a canonical DayZ script module dir \
                 ({}) — the engine may not load these scripts",
                CANONICAL_MODULE_DIRS.join(", ")
            ),
        ));
    }

    if let Some(root) = source_root.filter(|_| !folder_found && !trimmed.is_empty()) {
        v.diagnostics.push(at(
            "MF-C320",
            Severity::Error,
            span,
            format!(
                "module directory `{trimmed}` not found under source root `{}`",
                root.display()
            ),
        ));
    }
}

/// Confirm a declared module path exists somewhere under `root`. The path is
/// accepted when some suffix of it is a directory under `root`: the full path
/// first, then with leading components dropped one at a time. This keeps the
/// check strict enough to catch a real typo yet tolerant of prefix nesting.
fn module_dir_exists(root: &Path, module_path: &str) -> bool {
    let components: Vec<&str> = module_path.split('/').filter(|s| !s.is_empty()).collect();
    // Try full path first (root + all components), then drop leading components
    // one at a time to tolerate a prefix the source root does not repeat.
    (0..components.len()).any(|start| {
        let mut p = root.to_path_buf();
        for c in &components[start..] {
            p.push(c);
        }
        p.is_dir()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_core::Severity;

    fn codes(text: &str) -> Vec<String> {
        let v = validate_text(text, None);
        v.diagnostics
            .iter()
            .map(|d| d.code.clone())
            .collect::<Vec<_>>()
    }

    fn has_code(text: &str, code: &str) -> bool {
        codes(text).iter().any(|c| c == code)
    }

    fn valid_mod() -> &'static str {
        r#"class CfgPatches { class MyMod { requiredAddons[] = { "DZ_Data" }; }; };
class CfgMods {
    class MyMod {
        dir = "MyMod";
        type = "mod";
        class defs {
            class worldScriptModule { files[] = { "MyMod/Scripts/4_World" }; };
        };
    };
};
"#
    }

    #[test]
    fn clean_mod_has_no_errors() {
        let v = validate_text(valid_mod(), None);
        assert!(
            v.diagnostics.iter().all(|d| d.severity < Severity::Error),
            "{:?}",
            v.diagnostics
        );
    }

    #[test]
    fn syntax_error_yields_mf_c101() {
        assert!(has_code("class A { x = 1 }", "MF-C101"));
    }

    #[test]
    fn cfgmods_without_cfgpatches_is_error() {
        assert!(has_code(
            "class CfgMods { class M { type = \"mod\"; }; };",
            "MF-C200"
        ));
    }

    #[test]
    fn non_canonical_module_dir_is_warning() {
        let bad = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M {
    class defs {
        class worldScriptModule { files[] = { "M/NotAScriptDir" }; };
    };
}; };
"#;
        let v = validate_text(bad, None);
        let d = v
            .diagnostics
            .iter()
            .find(|d| d.code == "MF-C300")
            .expect("MF-C300 expected");
        assert_eq!(d.severity, Severity::Warning);
    }

    #[test]
    fn existing_module_dir_passes_when_root_given() {
        let dir = std::env::temp_dir().join(format!("mf-config-exists-{}", std::process::id()));
        // valid_mod() declares MyMod/Scripts/4_World. Build that under root.
        let mod_root = dir.join("MyMod/Scripts/4_World");
        std::fs::create_dir_all(&mod_root).unwrap();
        let v = validate_text(valid_mod(), Some(&dir));
        assert!(
            !v.diagnostics.iter().any(|d| d.code == "MF-C320"),
            "{:?}",
            v.diagnostics
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn severity_of_module_mismatch_is_error() {
        let bad = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M {
    class defs {
        class worldScriptModule { files[] = { "M/Scripts/3_Game" }; };
    };
}; };
"#;
        let v = validate_text(bad, None);
        let d = v
            .diagnostics
            .iter()
            .find(|d| d.code == "MF-C310")
            .expect("MF-C310 expected");
        assert_eq!(d.severity, Severity::Error);
    }

    fn span_of<'a>(text: &'a str, code: &str) -> &'a str {
        let v = validate_text(text, None);
        let d = v
            .diagnostics
            .iter()
            .find(|d| d.code == code)
            .unwrap_or_else(|| panic!("{code} expected in {:?}", v.diagnostics));
        let span = d.span.unwrap_or_else(|| panic!("{code} has no span"));
        &text[span.start..span.end]
    }

    #[test]
    fn c200_span_is_the_first_top_level_class_name() {
        let text = "class CfgMods { class M { type = \"mod\"; }; };";
        assert_eq!(span_of(text, "MF-C200"), "CfgMods");
        let v = validate_text("", None);
        assert!(v.diagnostics.iter().any(|d| d.code == "MF-C200"));
    }

    #[test]
    fn c220_span_is_the_module_name() {
        let no_files = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M { class defs { class worldScriptModule { }; }; }; };
"#;
        assert_eq!(span_of(no_files, "MF-C220"), "worldScriptModule");
    }

    #[test]
    fn files_item_diagnostics_span_the_item() {
        let wrong_folder = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M { class defs { class worldScriptModule { files[] = { "M/Scripts/5_Mission" }; }; }; }; };
"#;
        assert_eq!(span_of(wrong_folder, "MF-C310"), "\"M/Scripts/5_Mission\"");
        let non_canonical = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M { class defs { class worldScriptModule { files[] = { "M/Nope" }; }; }; }; };
"#;
        assert_eq!(span_of(non_canonical, "MF-C300"), "\"M/Nope\"");
    }

    #[test]
    fn c320_is_an_error_on_the_files_item() {
        let dir = std::env::temp_dir().join(format!("mf-config-span-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = valid_mod();
        let v = validate_text(text, Some(&dir));
        let d = v
            .diagnostics
            .iter()
            .find(|d| d.code == "MF-C320")
            .expect("MF-C320 expected");
        assert_eq!(d.severity, Severity::Error);
        let span = d.span.expect("span");
        assert_eq!(&text[span.start..span.end], "\"MyMod/Scripts/4_World\"");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn module_dir_warning_follows_the_source_root() {
        let cases: [(&str, &[&str], bool, bool); 4] = [
            (
                "M/Scripts/0_Preload",
                &["M/Scripts/0_Preload"],
                false,
                false,
            ),
            (
                "JM/CF/Scripts/0_Preload",
                &["Scripts/0_Preload"],
                false,
                false,
            ),
            ("M/Scripts/4_worl", &["M/Scripts/4_World"], true, true),
            ("M/Scripts/0_Preload", &[], true, true),
        ];
        for (i, (entry, folders, warned, missing)) in cases.into_iter().enumerate() {
            let root =
                std::env::temp_dir().join(format!("mf-config-c300-{}-{i}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            for folder in folders {
                std::fs::create_dir_all(root.join(folder)).unwrap();
            }
            let text = r#"class CfgPatches { class M { requiredAddons[] = {"DZ_Data"}; }; };
class CfgMods { class M { class defs { class gameScriptModule { files[] = { "ENTRY" }; }; }; }; };
"#
            .replace("ENTRY", entry);
            let v = validate_text(&text, Some(&root));
            std::fs::remove_dir_all(&root).ok();
            let severity = |code: &str| {
                v.diagnostics
                    .iter()
                    .find(|d| d.code == code)
                    .map(|d| d.severity)
            };
            assert_eq!(
                severity("MF-C300"),
                warned.then_some(Severity::Warning),
                "MF-C300 for `{entry}`: {:?}",
                v.diagnostics
            );
            assert_eq!(
                severity("MF-C320"),
                missing.then_some(Severity::Error),
                "MF-C320 for `{entry}`: {:?}",
                v.diagnostics
            );
            assert_eq!(
                severity("MF-C310"),
                None,
                "MF-C310 for `{entry}`: {:?}",
                v.diagnostics
            );
        }
    }
}

#[cfg(test)]
/// External-corpus smoke: parse+validate every `config.cpp` under
/// `MF_CORPUS_PATH` (never committed). Fails if any corpus
/// file lexes or parses with a syntax-level issue, proving the parser accepts
/// real DayZ configs (CommunityFramework etc.). Skipped when the env is unset.
#[test]
fn parses_external_config_corpus() {
    let Ok(root) = std::env::var("MF_CORPUS_PATH") else {
        eprintln!("skipped: MF_CORPUS_PATH not set");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let (found, failures) = scan_corpus(&root);
    assert!(
        found > 0,
        "MF_CORPUS_PATH={} contained no config.cpp files",
        root.display()
    );
    assert!(
        failures.is_empty(),
        "parser rejected {} corpus config(s): {failures:?}",
        failures.len()
    );
}

#[cfg(test)]
fn scan_corpus(dir: &std::path::Path) -> (usize, Vec<(String, usize)>) {
    let mut found = 0usize;
    let mut failures = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            let (f, fail) = scan_corpus(&p);
            found += f;
            failures.extend(fail);
        } else if p.file_name().and_then(|s| s.to_str()) == Some("config.cpp") {
            found += 1;
            let text = std::fs::read_to_string(&p).unwrap();
            let res = parse(&text);
            if res
                .issues
                .iter()
                .any(|i| i.kind == crate::parser::ParseErrorKind::Syntax)
            {
                failures.push((p.display().to_string(), res.issues.len()));
            }
        }
    }
    (found, failures)
}
