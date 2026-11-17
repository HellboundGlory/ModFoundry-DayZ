//! A project's known errors: the framework and vanilla noise every run logs,
//! kept in one file with what it means and how to fix it, so `mf errors` shows
//! only what is new and the run verdicts stop failing on it.

use std::path::{Path, PathBuf};

use crate::registry::LearnErrorArgs;
use modfoundry_diag::{is_generic, KnownRef, LinePattern, MappedError};
use serde::{Deserialize, Serialize};

/// Where a project's known errors live, relative to the source root.
pub const KNOWN_ERRORS_FILE: &str = "server/known-errors.json";

/// One entry as the file holds it: an id, a matcher and what it tells a reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub meaning: String,
    #[serde(default)]
    pub hint: String,
}

/// The validated entries, patterns parsed and matched in file order.
#[derive(Debug, Default)]
pub struct KnownErrors {
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    id: String,
    matcher: Matcher,
    meaning: String,
    hint: String,
}

#[derive(Debug)]
enum Matcher {
    Pattern(LinePattern),
    Fingerprint(String),
}

impl KnownErrors {
    /// The file's entries; an explicit path wins over the project default, and
    /// a missing file is an empty catalogue.
    pub fn load(explicit: Option<&Path>, source_root: Option<&Path>) -> anyhow::Result<Self> {
        let Some(path) = Self::path(explicit, source_root) else {
            return Ok(Self::default());
        };
        let file = path.display();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => anyhow::bail!("{file}: cannot read the known-errors file: {e}"),
        };
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut entries: Vec<KnownEntry> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{file}: malformed known-errors file: {e}"))?;
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            if !seen.insert(entry.id.clone()) {
                anyhow::bail!("{file}: entry {} appears twice", entry.id);
            }
        }
        let mut validated = Vec::with_capacity(entries.len());
        for entry in entries.drain(..) {
            let matcher = match (entry.pattern, entry.fingerprint) {
                (Some(pattern), None) => {
                    let parsed = LinePattern::parse(&pattern)
                        .map_err(|e| anyhow::anyhow!("{file}: entry {}: {e}", entry.id))?;
                    Matcher::Pattern(parsed)
                }
                (None, Some(fingerprint)) => Matcher::Fingerprint(fingerprint),
                _ => anyhow::bail!(
                    "{file}: entry {} needs exactly one of pattern and fingerprint",
                    entry.id
                ),
            };
            validated.push(Entry {
                id: entry.id,
                matcher,
                meaning: entry.meaning,
                hint: entry.hint,
            });
        }
        Ok(Self { entries: validated })
    }

    /// Where the catalogue would live, when either a path or a source root says.
    pub fn path(explicit: Option<&Path>, source_root: Option<&Path>) -> Option<PathBuf> {
        explicit
            .map(Path::to_path_buf)
            .or_else(|| source_root.map(|root| root.join(KNOWN_ERRORS_FILE)))
    }

    /// Mark every error a catalogue entry explains, first entry in file order winning.
    pub fn mark(&self, errors: &mut [MappedError]) {
        for error in errors {
            if is_generic(&error.message) {
                continue;
            }
            error.known = self.entries.iter().find_map(|entry| {
                let matched = match &entry.matcher {
                    Matcher::Pattern(pattern) => pattern.matches(&error.message),
                    Matcher::Fingerprint(fingerprint) => *fingerprint == error.fingerprint,
                };
                matched.then(|| KnownRef {
                    id: entry.id.clone(),
                    meaning: entry.meaning.clone(),
                    hint: entry.hint.clone(),
                })
            });
        }
    }

    /// The pattern entries, for a run's allow list.
    pub fn patterns(&self) -> Vec<LinePattern> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.matcher {
                Matcher::Pattern(pattern) => Some(pattern.clone()),
                Matcher::Fingerprint(_) => None,
            })
            .collect()
    }

    /// The fingerprint entries, for a run's allowed fingerprints.
    pub fn fingerprints(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.matcher {
                Matcher::Fingerprint(fingerprint) => Some(fingerprint.clone()),
                Matcher::Pattern(_) => None,
            })
            .collect()
    }
}

/// The file a learn call writes, its new text, and the revision it was read at.
pub fn plan_learn(args: &LearnErrorArgs) -> anyhow::Result<(PathBuf, String, Option<String>)> {
    let Some(path) = KnownErrors::path(args.known_errors.as_deref(), args.source_root.as_deref())
    else {
        anyhow::bail!(
            "learning {} needs known_errors or source_root to say where the file is",
            args.id
        );
    };
    let (old, expected) = match std::fs::read(&path) {
        Ok(bytes) => {
            let sha = modfoundry_vfs::sha256_hex(&bytes);
            let text = String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("{}: not UTF-8", path.display()))?;
            (text, Some(sha))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => anyhow::bail!("{}: cannot read the known-errors file: {e}", path.display()),
    };
    let mut entries: Vec<KnownEntry> = if old.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&old)
            .map_err(|e| anyhow::anyhow!("{}: malformed known-errors file: {e}", path.display()))?
    };
    let errors = crate::errors_from_dir(&args.dir, args.source_root.as_deref())?;
    let error = errors
        .iter()
        .find(|error| error.id == args.id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}: no error with id {} in {}",
                path.display(),
                args.id,
                args.dir.display()
            )
        })?;
    if is_generic(&error.message) {
        anyhow::bail!(
            "{}: error {} is a {} header, which carries no identity of its own; write a pattern entry by hand",
            path.display(),
            args.id,
            error.message.trim()
        );
    }
    if entries.iter().any(|entry| entry.id == args.id) {
        anyhow::bail!(
            "{}: entry {} already exists; learning the error under that id would repeat it",
            path.display(),
            args.id
        );
    }
    if let Some(existing) = entries
        .iter()
        .find(|entry| entry.fingerprint.as_deref() == Some(error.fingerprint.as_str()))
    {
        anyhow::bail!(
            "{}: error {} is already learned as entry {}",
            path.display(),
            args.id,
            existing.id
        );
    }
    entries.push(KnownEntry {
        id: args.id.clone(),
        pattern: None,
        fingerprint: Some(error.fingerprint.clone()),
        meaning: args.meaning.clone(),
        hint: args.hint.clone(),
    });
    let mut text = serde_json::to_string_pretty(&entries)?;
    text.push('\n');
    Ok((path, text, expected))
}
