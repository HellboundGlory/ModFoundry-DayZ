//! Error→file mapping.
//!
//! The engine logs script/config failures against *virtual* source paths (the
//! module-relative path inside a PBO or a `-mod` tree). This module turns a
//! parsed [`LogRecord`] into a [`MappedError`] that carries the source location
//! the failure points at, and resolves that virtual path to a host file under
//! the launched mod's source root where one exists (so GUI/MCP can jump).

use crate::fingerprint::{error_fingerprint, is_generic, short_id};
use crate::parse::{Level, LogRecord};
use crate::run::{run_log_text, LogStates};
use modfoundry_core::Severity;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A log line turned into a file-attributable diagnostic. Serializable so it
/// crosses the command/MCP boundary with its resolved host file intact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappedError {
    /// Source file the error points at, resolved to a host path when the
    /// resolver found one; otherwise `None`.
    pub file: Option<PathBuf>,
    /// The virtual source path the engine logged (kept even when unresolved).
    pub logged_file: Option<String>,
    pub line: Option<u32>,
    pub severity: Severity,
    pub message: String,
    /// Normalised identity that survives another run.
    #[serde(default)]
    pub fingerprint: String,
    /// Short form of `fingerprint`, the handle `--learn` takes.
    #[serde(default)]
    pub id: String,
    /// Whether a baseline run without this project's mods logged the same error.
    #[serde(default)]
    pub baseline: bool,
    /// The known-error entry that explains this error, when the catalogue holds one.
    #[serde(default)]
    pub known: Option<KnownRef>,
}

/// The known-error entry an error matched: its id, what it means and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownRef {
    pub id: String,
    pub meaning: String,
    pub hint: String,
}

impl MappedError {
    /// An error with its identity filled in and nothing matched against it yet.
    pub fn new(
        file: Option<PathBuf>,
        logged_file: Option<String>,
        line: Option<u32>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        let message = message.into();
        let fingerprint = error_fingerprint(&message, logged_file.as_deref());
        Self {
            file,
            logged_file,
            line,
            severity,
            id: short_id(&fingerprint),
            fingerprint,
            message,
            baseline: false,
            known: None,
        }
    }
}

/// Mark every error whose fingerprint a baseline run also logged. A generic
/// error (a VM header, a stack trace) carries no identity of its own, so one
/// baseline trace must not hide every later one.
pub fn mark_baseline(errors: &mut [MappedError], baseline: &[MappedError]) {
    let known: std::collections::HashSet<&str> = baseline
        .iter()
        .map(|error| error.fingerprint.as_str())
        .collect();
    for error in errors {
        error.baseline = !is_generic(&error.message) && known.contains(error.fingerprint.as_str());
    }
}

/// Resolves an engine-logged virtual source path to a host file under a source
/// root. Backed by an index of the files that exist under that root.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    /// Known files under the root as `(lowercased path segments, original relative
    /// path)`. The original preserves case so the resolved host path is openable
    /// on case-sensitive filesystems.
    pub(crate) files: Vec<(Vec<String>, String)>,
    pub(crate) root: Option<PathBuf>,
}

impl Resolver {
    /// Index every file under `root` outside hidden folders; links are not followed.
    /// Scans recursively; pure results, IO at this boundary.
    pub fn scan(root: &Path) -> Self {
        let mut files = Vec::new();
        collect(root, root, &mut files);
        // `read_dir` order is unspecified; sorting keeps the tie-break in `resolve` stable.
        files.sort_by(|a, b| a.1.cmp(&b.1));
        Resolver {
            files,
            root: Some(root.to_path_buf()),
        }
    }

    /// Index a pre-known set of relative source paths.
    #[cfg(test)]
    fn from_paths(root: Option<PathBuf>, rel_paths: &[&str]) -> Self {
        Self {
            root,
            files: rel_paths
                .iter()
                .map(|p| (segments(p), p.replace('\\', "/")))
                .collect(),
        }
    }

    /// Map a logged virtual path to a host file. A file matches when one path is
    /// the other's trailing segments; the longest shared tail wins.
    pub fn resolve(&self, logged: &str) -> Option<PathBuf> {
        let needle = segments(logged);
        if needle.is_empty() {
            return None;
        }
        let mut best: Option<(usize, &str)> = None;
        for (key, rel) in &self.files {
            let shared = key
                .iter()
                .rev()
                .zip(needle.iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let matched = shared == key.len() || shared == needle.len();
            if matched && best.is_none_or(|(s, _)| shared > s) {
                best = Some((shared, rel.as_str()));
            }
        }
        best.map(|(_, rel)| {
            // Return the original-case relative path so the resolved file exists on
            // a case-sensitive filesystem.
            let mut base = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
            base.push(rel);
            base
        })
    }
}

/// Location identity of an error: its resolved host file if any, else its
/// logged virtual path, else nothing (a source-less diagnostic).
fn loc_key(e: &MappedError) -> Option<String> {
    match &e.file {
        Some(p) => Some(norm(&p.to_string_lossy())),
        None => e.logged_file.as_deref().map(norm),
    }
}

/// Recursively collect every file under `root` as `(segments, original rel)`.
/// The segments are lowercased for comparison; the original relative path
/// preserves case so the resolved host path is openable on case-sensitive
/// filesystems.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(Vec<String>, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        // `file_type` does not follow links, so a link back to an ancestor cannot recurse.
        let Ok(kind) = e.file_type() else {
            continue;
        };
        let path = e.path();
        if kind.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
            collect(root, &path, out);
        } else if kind.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                out.push((segments(&rel), rel));
            }
        }
    }
}

/// Map parsed log records to file-attributable errors. Only `Error`/`Warning`
/// records are surfaced; `Info` lines are dropped.
pub fn map_logs(records: &[LogRecord], resolver: &Resolver) -> Vec<MappedError> {
    let mut out = Vec::new();
    for rec in records {
        let severity = match rec.level {
            Level::Info => continue,
            Level::Warning => Severity::Warning,
            Level::Error => Severity::Error,
        };
        let logged_file = rec.file.clone();
        let resolved = logged_file.as_deref().and_then(|lf| resolver.resolve(lf));
        out.push(MappedError::new(
            resolved,
            logged_file,
            rec.line,
            severity,
            rec.message.clone(),
        ));
    }
    out
}

/// Drop duplicate representations of the same underlying error while keeping
/// first-seen order. The engine mirrors one compile/script failure across the
/// RPT, script.log and error.log, so `errors_from_dir` over several files would
/// otherwise surface each error once per file that carries it. Two errors are
/// "the same" when their severity, source location (resolved file or logged
/// virtual path), line and message all agree.
pub fn dedup_errors(errors: Vec<MappedError>) -> Vec<MappedError> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(errors.len());
    for e in errors {
        let key = (
            loc_key(&e).unwrap_or_default(),
            e.line.unwrap_or(0),
            e.severity as u8,
            norm(&e.message),
        );
        if seen.insert(key) {
            out.push(e);
        }
    }
    out
}

/// The line count from which a flood of storage-load errors is worth a hint.
const STORAGE_FLOOD_LINES: usize = 100;

/// A likely cause for a saved world the engine could not load, when the flood is
/// large enough to point at a different mod list than the one that wrote it.
pub fn storage_flood_hint(corrupted_lines: usize) -> Option<String> {
    (corrupted_lines >= STORAGE_FLOOD_LINES).then(|| {
        format!(
            "the server logged {corrupted_lines} \"Scripted variables corrupted\" lines: its saved world was probably written with a different mod list or an older version of a mod. Load what wrote it, or move the mission's storage folder aside to start a new world."
        )
    })
}

/// The phrase the engine repeats for every saved variable it cannot load.
const STORAGE_ERROR_MARKER: &str = "Scripted variables corrupted";

/// Count `Scripted variables corrupted` lines across the RPT files in `logs`, reading
/// only the part this run wrote and decoded lossily so a legacy code-page byte costs
/// no line.
pub fn count_storage_errors(logs: &[PathBuf], earlier: &LogStates) -> usize {
    logs.iter()
        .filter(|log| {
            log.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rpt"))
        })
        .filter_map(|log| run_log_text(log, earlier).ok())
        .map(|text| {
            text.lines()
                .filter(|line| line.contains(STORAGE_ERROR_MARKER))
                .count()
        })
        .sum()
}

/// Normalize an engine/virtual path: `\` → `/`, lowercase (comparisons only).
pub(crate) fn norm(p: &str) -> String {
    p.replace('\\', "/").to_ascii_lowercase()
}

fn segments(p: &str) -> Vec<String> {
    norm(p)
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{map_logs, LogRecord, MappedError, Resolver};
    use crate::parse::{parse_log, Channel, Level};
    use crate::run::LogStates;
    use std::path::{Path, PathBuf};

    fn rec(
        level: Level,
        channel: Channel,
        msg: &str,
        file: Option<&str>,
        line: Option<u32>,
    ) -> LogRecord {
        LogRecord {
            time: None,
            channel,
            level,
            message: msg.into(),
            file: file.map(str::to_owned),
            line,
        }
    }

    #[test]
    fn info_records_are_dropped_errors_and_warnings_kept() {
        let recs = vec![
            rec(Level::Info, Channel::Script, "Loaded Core", None, None),
            rec(
                Level::Error,
                Channel::Script,
                "Virtual Machine Exception",
                Some("Mod/scripts/3_Game/a.c"),
                Some(12),
            ),
            rec(
                Level::Warning,
                Channel::Script,
                "FIX-ME",
                Some("Mod/scripts/3_Game/b.c"),
                Some(4),
            ),
        ];
        let r = Resolver::from_paths(None, &["mod/scripts/3_Game/a.c"]);
        let out = map_logs(&recs, &r);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].severity, modfoundry_core::Severity::Error);
        assert_eq!(out[1].severity, modfoundry_core::Severity::Warning);
    }

    #[test]
    fn resolves_virtual_path_to_host_file_case_and_separator_insensitively() {
        // Host tree uses forward slashes + one different-case segment.
        let recs = vec![rec(
            Level::Error,
            Channel::Script,
            "boom",
            Some("MyMod/scripts/3_Game/Foo.c"),
            Some(7),
        )];
        let resolver =
            Resolver::from_paths(Some(PathBuf::from("/src/MyMod")), &["scripts/3_game/foo.c"]);
        let out = map_logs(&recs, &resolver);
        assert_eq!(
            out[0].file.as_deref(),
            Some(Path::new("/src/MyMod/scripts/3_game/foo.c"))
        );
    }

    #[test]
    fn unresolved_file_stays_none_but_keeps_logged_path_and_line() {
        let recs = vec![rec(
            Level::Error,
            Channel::Entity,
            "Type must be inherited",
            Some("Other/scripts/3_Game/x.c"),
            Some(4),
        )];
        let r = Resolver::from_paths(None, &["scripts/3_game/zz.c"]);
        let out = map_logs(&recs, &r);
        assert_eq!(out.len(), 1);
        assert!(out[0].file.is_none());
        assert_eq!(
            out[0].logged_file.as_deref(),
            Some("Other/scripts/3_Game/x.c")
        );
        assert_eq!(out[0].line, Some(4));
    }

    #[test]
    fn end_to_end_parse_then_map_produces_file_mapped_error() {
        // A real-shaped VM-exception block + a warning with @source prefix.
        let body = "15:52:04.974  SCRIPT    (E): Virtual Machine Exception\r\n\r\nReason: Function 'SetTransform' not linked\r\n\r\nStack trace:\r\nAreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1123 Function ApplyTextStyle\r\n";
        let records = parse_log(body);
        let resolver = Resolver::from_paths(
            Some(PathBuf::from("/mods/AA")),
            &["scripts/5_mission/areaannouncements/aa_areaannouncementmanager.c"],
        );
        let out = map_logs(&records, &resolver);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, Some(1123));
        assert_eq!(
            out[0].file.as_deref(),
            Some(Path::new(
                "/mods/AA/scripts/5_mission/areaannouncements/aa_areaannouncementmanager.c"
            ))
        );
    }

    #[test]
    fn scan_resolves_to_real_case_host_path() {
        // Regression: `scan` must return the on-disk path with its original case,
        // not the lowercased match key (a lowercased path is unopenable on
        // case-sensitive filesystems).
        let root = std::env::temp_dir().join(format!("mf-map-scan-{}", std::process::id()));
        let file = root.join("Scripts/3_Game/Broken.c");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "class X {}\n").unwrap();
        let resolver = Resolver::scan(&root);
        let recs = vec![rec(
            Level::Error,
            Channel::Script,
            "boom",
            Some("p3diag/Scripts/3_Game/Broken.c"),
            Some(5),
        )];
        let out = map_logs(&recs, &resolver);
        assert_eq!(out[0].file.as_deref(), Some(file.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_resolves_a_tie_to_the_first_file_by_sorted_path() {
        let root = std::env::temp_dir().join(format!("mf-map-tie-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/Scripts")).unwrap();
        std::fs::write(root.join("a/Scripts/X.c"), "class A {}\n").unwrap();
        std::fs::create_dir_all(root.join("b/Scripts")).unwrap();
        std::fs::write(root.join("b/Scripts/X.c"), "class B {}\n").unwrap();
        let resolver = Resolver::scan(&root);
        let recs = vec![rec(
            Level::Error,
            Channel::Script,
            "boom",
            Some("Scripts/X.c"),
            Some(3),
        )];
        let out = map_logs(&recs, &resolver);
        let expected = root.join("a/Scripts/X.c");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(out[0].file.as_deref(), Some(expected.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_links_and_hidden_folders() {
        let root = std::env::temp_dir().join(format!("mf-map-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("scripts/a.c"), "class A {}\n").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::os::unix::fs::symlink(".", root.join("loop")).unwrap();
        let resolver = Resolver::scan(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(resolver.files.len(), 1);
        assert_eq!(resolver.files[0].1, "scripts/a.c");
    }

    fn err(
        file: Option<&str>,
        logged_file: Option<&str>,
        line: Option<u32>,
        message: &str,
    ) -> MappedError {
        MappedError::new(
            file.map(PathBuf::from),
            logged_file.map(str::to_owned),
            line,
            modfoundry_core::Severity::Error,
            message,
        )
    }

    #[test]
    fn dedup_collapses_mirrored_error_across_log_files_keeps_distinct_ones() {
        // RPT + error.log carry the same compile error; a second distinct
        // failure on a different file must survive.
        let host = "/src/MyMod/scripts/3_game/broken.c";
        let errors = vec![
            err(
                Some(host),
                Some("MyMod/scripts/3_Game/broken.c"),
                Some(5),
                "Can't find variable 'undefined_symbol_x'",
            ),
            err(
                Some(host),
                Some("MyMod/scripts/3_Game/broken.c"),
                Some(5),
                "Can't find variable 'undefined_symbol_x'",
            ),
            err(
                Some("/src/MyMod/scripts/4_World/other.c"),
                Some("MyMod/scripts/4_World/other.c"),
                Some(3),
                "Undefined function 'boom'",
            ),
        ];
        let out = super::dedup_errors(errors);
        assert_eq!(out.len(), 2);
        // First-seen order preserved; mirrored duplicate dropped.
        assert_eq!(out[0].line, Some(5));
        assert_eq!(out[1].line, Some(3));
    }

    #[test]
    fn dedup_keeps_same_location_different_message() {
        // Two genuine failures on one line must not collapse.
        let errors = vec![
            err(Some("a.c"), Some("a.c"), Some(9), "first problem"),
            err(Some("a.c"), Some("a.c"), Some(9), "second problem"),
        ];
        assert_eq!(super::dedup_errors(errors).len(), 2);
    }

    #[test]
    fn storage_errors_count_rpt_lines_and_ignore_other_logs() {
        let dir = std::env::temp_dir().join(format!("mf-storage-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rpt = dir.join("DayZServer_1.RPT");
        let log = dir.join("script_1.log");
        std::fs::write(
            &rpt,
            b"a\r\nplayer \xE9\r\nScripted variables corrupted: 3\r\nScripted variables corrupted: 4\r\n",
        )
        .unwrap();
        std::fs::write(&log, "Scripted variables corrupted: 5\r\n").unwrap();

        assert_eq!(
            super::count_storage_errors(&[rpt, log], &LogStates::new()),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storage_errors_count_only_the_lines_this_run_appended() {
        let dir = std::env::temp_dir().join(format!("mf-storage-appended-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rpt = dir.join("DayZServer_1.RPT");
        std::fs::write(
            &rpt,
            "Scripted variables corrupted: 1\r\nScripted variables corrupted: 2\r\n",
        )
        .unwrap();
        let earlier = crate::run::log_states(&dir);
        std::fs::write(
            &rpt,
            "Scripted variables corrupted: 1\r\nScripted variables corrupted: 2\r\nScripted variables corrupted: 3\r\n",
        )
        .unwrap();

        assert_eq!(super::count_storage_errors(&[rpt], &earlier), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flood_of_storage_errors_names_the_saved_world_at_the_threshold() {
        assert_eq!(super::storage_flood_hint(99), None);
        assert_eq!(
            super::storage_flood_hint(100),
            Some(
                "the server logged 100 \"Scripted variables corrupted\" lines: its saved world was probably written with a different mod list or an older version of a mod. Load what wrote it, or move the mission's storage folder aside to start a new world."
                    .to_owned()
            )
        );
    }

    #[test]
    fn dedup_collapses_unresolved_logged_paths_without_source_root() {
        // `errors_from_dir(dir, None)` maps nothing to a host file, so every
        // error carries file: None and only the logged virtual path. The same
        // mirrored error across RPT/error.log must still collapse.
        let errors = vec![
            err(
                None,
                Some("p3diag/scripts/3_Game/Broken.c"),
                Some(1),
                "Can't find variable 'undefined_symbol_x'",
            ),
            err(
                None,
                Some("p3diag/scripts/3_Game/Broken.c"),
                Some(1),
                "Can't find variable 'undefined_symbol_x'",
            ),
        ];
        assert_eq!(super::dedup_errors(errors).len(), 1);
    }

    #[test]
    fn a_source_root_above_the_mod_still_resolves() {
        let r = Resolver::from_paths(
            Some(PathBuf::from("/repo")),
            &["src/MyMod/scripts/3_Game/a.c"],
        );
        assert_eq!(
            r.resolve("MyMod/scripts/3_Game/a.c"),
            Some(PathBuf::from("/repo/src/MyMod/scripts/3_Game/a.c"))
        );
        assert_eq!(r.resolve("Other/a.c"), None);
        assert_eq!(r.resolve("OtherMod/scripts/3_Game/a.c"), None);
    }

    #[test]
    fn a_framework_json_error_is_a_mapped_error_with_a_stable_id() {
        let resolver = Resolver::default();
        let read = |time: &str| {
            let text = format!(
                " {time} SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46\r\n"
            );
            map_logs(&parse_log(&text), &resolver)
        };
        let first = read("3:45:51.567");
        assert_eq!(first.len(), 1);
        assert!(first[0].file.is_none());
        assert!(first[0].message.starts_with("JSON ERROR"), "{:?}", first[0]);
        assert_eq!(first[0].id.len(), 8);
        assert_eq!(first[0].id, read("9:02:11.001")[0].id);
    }

    fn mapped(message: &str, logged_file: Option<&str>) -> MappedError {
        MappedError::new(
            None,
            logged_file.map(str::to_owned),
            None,
            modfoundry_core::Severity::Error,
            message,
        )
    }

    #[test]
    fn a_baseline_marks_the_errors_it_shares_and_leaves_the_rest() {
        let baseline = vec![
            mapped("JSON ERROR JM/CF/a.c:46", None),
            mapped("Virtual Machine Exception", Some("Mod/scripts/a.c")),
            mapped("-- Stack trace --", Some("Mod/scripts/a.c")),
        ];
        let mut errors = vec![
            mapped("JSON ERROR JM/CF/a.c:47", None),
            mapped("something new", Some("Mod/scripts/b.c")),
            mapped("Virtual Machine Exception", Some("Mod/scripts/a.c")),
            mapped("-- Stack trace --", Some("Mod/scripts/a.c")),
        ];
        super::mark_baseline(&mut errors, &baseline);
        assert!(errors[0].baseline);
        assert!(!errors[1].baseline);
        assert!(
            !errors[2].baseline,
            "a VM header is never matched by fingerprint"
        );
        assert!(
            !errors[3].baseline,
            "a stack trace is never matched by fingerprint"
        );
    }

    #[test]
    fn an_empty_baseline_marks_nothing() {
        let mut errors = vec![mapped("JSON ERROR a.c:1", None)];
        super::mark_baseline(&mut errors, &[]);
        assert!(!errors[0].baseline);
    }
}
