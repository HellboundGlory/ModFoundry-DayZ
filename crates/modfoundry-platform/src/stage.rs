//! Launch staging: the links, files and folders a launch puts into an install.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The install folder that holds one journal file per guard.
pub const JOURNAL_NAME: &str = ".modfoundry-staged";

static NEXT_JOURNAL: AtomicU64 = AtomicU64::new(0);

/// What a launch put into an install; dropping it removes exactly that, newest first.
#[derive(Debug, Default)]
pub struct Staged {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    journal: Option<Journal>,
}

/// Where a guard records what it stages: one file of its own, made on the first record.
#[derive(Debug, Default)]
struct Journal {
    dir: PathBuf,
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl Journal {
    fn append(&mut self, line: &[u8]) -> std::io::Result<()> {
        if self.file.is_none() {
            if let Err(error) = self.create() {
                return Err(named(self.described(), error));
            }
        }
        let Some(file) = self.file.as_mut() else {
            return Err(std::io::Error::other("journal file is gone"));
        };
        file.write_all(line)
            .map_err(|error| named(&self.path, error))
    }

    /// This guard's file, or the folder until there is one, for an error that names it.
    fn described(&self) -> &Path {
        if self.path.as_os_str().is_empty() {
            &self.dir
        } else {
            &self.path
        }
    }

    fn create(&mut self) -> std::io::Result<()> {
        match self.claim() {
            // A clear removed the empty folder between the two calls.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.claim(),
            other => other,
        }
    }

    /// Takes the next free name with `create_new`, so no other guard or run can share it.
    fn claim(&mut self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        for _ in 0..16 {
            let n = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
            let path = self.dir.join(format!("{}-{n}.jsonl", std::process::id()));
            match std::fs::OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    self.path = path;
                    self.file = Some(file);
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("no free journal name in {}", self.dir.display()),
        ))
    }
}

/// What a guard made at a path, enough to tell its own file from one the user replaced.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Entry {
    Link { path: PathBuf, target: PathBuf },
    File { path: PathBuf, len: u64, hash: u64 },
    Dir { path: PathBuf },
}

/// The record a guard writes before it makes something at a path.
enum Making<'a> {
    Link(&'a Path),
    File(&'a [u8]),
    Dir,
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl Entry {
    fn path(&self) -> &Path {
        match self {
            Entry::Link { path, .. } | Entry::File { path, .. } | Entry::Dir { path } => path,
        }
    }
}

impl Staged {
    /// A guard that journals what it stages in `install`, so a later run can remove it.
    pub fn in_install(install: &Path) -> Staged {
        Staged {
            files: Vec::new(),
            dirs: Vec::new(),
            journal: Some(Journal {
                dir: install.join(JOURNAL_NAME),
                file: None,
                path: PathBuf::new(),
            }),
        }
    }

    /// The absolute path to stage at, journaled before it is made so a kill between
    /// the two steps leaves only an entry with nothing to remove.
    fn record(&mut self, path: &Path, making: Making<'_>) -> std::io::Result<PathBuf> {
        let path = journal_path(path)?;
        let Some(journal) = &mut self.journal else {
            return Ok(path);
        };
        let entry = match making {
            Making::Link(target) => Entry::Link {
                path: path.clone(),
                target: target.to_path_buf(),
            },
            Making::File(bytes) => Entry::File {
                path: path.clone(),
                len: bytes.len() as u64,
                hash: hash(bytes),
            },
            Making::Dir => Entry::Dir { path: path.clone() },
        };
        // A path JSON cannot hold is staged unjournaled rather than not at all.
        let Ok(mut line) = serde_json::to_vec(&entry) else {
            return Ok(path);
        };
        line.push(b'\n');
        journal.append(&line)?;
        Ok(path)
    }

    /// Creates the symlink `link -> target` and records it.
    pub fn link(&mut self, link: &Path, target: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            let link = self.record(link, Making::Link(target))?;
            std::os::unix::fs::symlink(target, &link)?;
            self.files.push(link);
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (link, target);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "symlinks are only staged on unix hosts",
            ))
        }
    }

    /// Writes a new file at `path` and records it.
    pub fn write(&mut self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = self.record(path, Making::File(contents))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        self.files.push(path);
        file.write_all(contents)
    }

    /// Replaces the symlink at `link` with one to `target` and records it.
    pub fn relink(&mut self, link: &Path, target: &Path) -> std::io::Result<()> {
        std::fs::remove_file(link)?;
        self.link(link, target)
    }

    /// Copies `from` to a new file at `to` and records it.
    pub fn copy(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
        let bytes = std::fs::read(from)?;
        self.write(to, &bytes)
    }

    /// Creates the folder `dir` and records it; its parent must exist.
    pub fn create_dir(&mut self, dir: &Path) -> std::io::Result<()> {
        let dir = self.record(dir, Making::Dir)?;
        std::fs::create_dir(&dir)?;
        self.dirs.push(dir);
        Ok(())
    }

    /// Leaves what this guard staged in place for the dead-run sweep, which takes it once `owner`
    /// (this process when `None`) has ended.
    pub fn keep_for(mut self, owner: Option<u32>) {
        self.files.clear();
        self.dirs.clear();
        let Some(journal) = self.journal.take() else {
            return;
        };
        let Some(owner) = owner else { return };
        let Some(name) = journal
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            return;
        };
        drop(journal.file);
        // The owner's pid leads the name, so no journal of the owner's own can clash with it.
        let _ = std::fs::rename(
            &journal.path,
            journal.dir.join(format!("{owner}-kept-{name}")),
        );
    }

    /// Creates the missing parent folders of `rel` under `base`, shallowest first, and records them.
    pub fn create_parents(&mut self, base: &Path, rel: &Path) -> std::io::Result<()> {
        let mut dir = base.to_path_buf();
        for part in rel.parent().into_iter().flat_map(Path::components) {
            dir.push(part.as_os_str());
            if dir.is_dir() {
                continue;
            }
            let made = self.record(&dir, Making::Dir)?;
            std::fs::create_dir(&made).map_err(|e| {
                std::io::Error::new(e.kind(), format!("creating {}: {e}", dir.display()))
            })?;
            self.dirs.push(made);
        }
        Ok(())
    }
}

/// The form a journal records `path` in: absolute, with its folder resolved so no `..` reaches a line.
pub fn journal_path(path: &Path) -> std::io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let Some(name) = path.file_name() else {
        return Ok(path);
    };
    match path.parent().map(Path::canonicalize) {
        Some(Ok(parent)) => Ok(parent.join(name)),
        _ => Ok(path),
    }
}

fn named(journal: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("writing {}: {error}", journal.display()),
    )
}

impl Drop for Staged {
    fn drop(&mut self) {
        let mut clean = true;
        for path in self.files.iter().rev() {
            if let Err(error) = std::fs::remove_file(path) {
                clean &= error.kind() == std::io::ErrorKind::NotFound;
            }
        }
        for dir in self.dirs.iter().rev() {
            if let Err(error) = std::fs::remove_dir(dir) {
                clean &= matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                );
            }
        }
        let Some(journal) = &mut self.journal else {
            return;
        };
        if clean {
            // Close it first: Windows cannot delete a file that is still open.
            journal.file = None;
            if !journal.path.as_os_str().is_empty() {
                let _ = std::fs::remove_file(&journal.path);
            }
            let _ = std::fs::remove_dir(&journal.dir);
        }
    }
}

/// What a clear stopped on: the paths it removed before, and the entry it could not.
#[derive(Debug)]
pub struct ClearFailure {
    pub removed: Vec<PathBuf>,
    pub path: PathBuf,
    pub source: std::io::Error,
}

impl std::fmt::Display for ClearFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "removing {}: {}", self.path.display(), self.source)
    }
}

impl std::error::Error for ClearFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// What happened to one journal entry's path.
enum Removal {
    Removed,
    Skip,
    Failed(std::io::Error),
}

/// Removes what the runs that died left in `install`, newest entry first, and returns those paths.
pub fn clear_dead_runs(install: &Path) -> Result<Vec<PathBuf>, ClearFailure> {
    let dir = install.join(JOURNAL_NAME);
    let journals = match dead_journals(&dir) {
        Ok(journals) => journals,
        Err(source) => {
            return Err(ClearFailure {
                removed: Vec::new(),
                path: dir,
                source,
            })
        }
    };
    if journals.is_empty() {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    let install = match install.canonicalize() {
        Ok(install) => install,
        Err(source) => {
            return Err(ClearFailure {
                removed,
                path: dir,
                source,
            })
        }
    };
    for journal in journals {
        // Claim it first: another clear that loses this rename leaves it to us.
        let claimed = claim_path(&journal);
        match std::fs::rename(&journal, &claimed) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ClearFailure {
                    removed: std::mem::take(&mut removed),
                    path: journal,
                    source,
                })
            }
        }
        clear_journal(&claimed, &install, &mut removed)?;
    }
    let _ = std::fs::remove_dir(&dir);
    Ok(removed)
}

/// The journal files of runs that are no longer alive.
fn dead_journals(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|journal| {
            journal
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(owner_in_name)
                .is_some_and(|pid| !crate::process::alive(pid))
        })
        .collect())
}

/// The paths a dead run left that a clear would remove now, in the order a clear takes them.
pub fn dead_run_leftovers(install: &Path) -> Vec<PathBuf> {
    let Ok(journals) = dead_journals(&install.join(JOURNAL_NAME)) else {
        return Vec::new();
    };
    let Ok(install) = install.canonicalize() else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    for entry in journals.iter().flat_map(|journal| entries(journal)) {
        if contained(&install, entry.path()).ok() == Some(true)
            && ours_once_cleared(&entry, &listed)
        {
            listed.push(entry.path().to_path_buf());
        }
    }
    listed
}

/// `ours`, judged as if the paths in `cleared` were already gone, as they are by the time a
/// clear reaches a folder.
fn ours_once_cleared(entry: &Entry, cleared: &[PathBuf]) -> bool {
    let Entry::Dir { path } = entry else {
        return matches!(ours(entry), Ours::Yes);
    };
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
        && std::fs::read_dir(path).is_ok_and(|mut children| {
            children.all(|child| child.is_ok_and(|child| cleared.contains(&child.path())))
        })
}

/// The entries of one journal, newest first; a line that is not one is dropped.
fn entries(journal: &Path) -> Vec<Entry> {
    let Ok(bytes) = std::fs::read(journal) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    entries.reverse();
    entries
}

/// Whether a journaled path is one this install may remove: no `..`, and its folder inside the install.
fn contained(install: &Path, path: &Path) -> std::io::Result<bool> {
    if path.components().any(|part| part == Component::ParentDir) {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    match parent.canonicalize() {
        Ok(parent) => Ok(parent.starts_with(install)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// The pid a journal file's name holds: the process that claimed it last, else the one that wrote it.
fn owner_in_name(name: &str) -> Option<u32> {
    let tail = name.rsplit_once(".claim-").map_or(name, |(_, tail)| tail);
    tail.split('-').next()?.parse().ok()
}

/// The name this clear takes a journal under, so no other clear will touch it.
fn claim_path(journal: &Path) -> PathBuf {
    let name = journal.file_name().unwrap_or_default().to_string_lossy();
    let base = name.split(".claim-").next().unwrap_or_default();
    let mut path = PathBuf::new();
    // A claim a process with the same pid left behind must not be renamed over.
    for _ in 0..16 {
        let n = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
        path = journal.with_file_name(format!("{base}.claim-{}-{n}", std::process::id()));
        if !path.exists() {
            break;
        }
    }
    path
}

/// Removes the dead entries of one journal, newest first, and rewrites what is left.
fn clear_journal(
    journal: &Path,
    install: &Path,
    removed: &mut Vec<PathBuf>,
) -> Result<(), ClearFailure> {
    let bytes = match std::fs::read(journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ClearFailure {
                removed: std::mem::take(removed),
                path: journal.to_path_buf(),
                source,
            })
        }
    };
    // A blank line carries nothing, and keeping it would keep the file for ever.
    let lines: Vec<&[u8]> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    let mut keep = vec![true; lines.len()];
    let mut failed = None;
    for index in (0..lines.len()).rev() {
        let Ok(entry) = serde_json::from_slice::<Entry>(lines[index]) else {
            continue;
        };
        let outcome = match contained(install, entry.path()) {
            Ok(true) => match ours(&entry) {
                Ours::Yes => remove_ours(&entry),
                Ours::No => Removal::Skip,
                Ours::Unreadable(source) => Removal::Failed(source),
            },
            Ok(false) => Removal::Skip,
            Err(error) => self::failed(error),
        };
        match outcome {
            Removal::Removed => {
                removed.push(entry.path().to_path_buf());
                keep[index] = false;
            }
            Removal::Skip => keep[index] = false,
            Removal::Failed(source) => {
                failed = Some((entry.path().to_path_buf(), source));
                break;
            }
        }
    }
    let left = kept(&lines, &keep);
    if let Some((path, source)) = failed {
        // Back under its dead writer's name, so a later clear from this process retries it.
        if rewrite(journal, &left).is_ok() {
            let _ = std::fs::rename(journal, unclaimed(journal));
        }
        return Err(ClearFailure {
            removed: std::mem::take(removed),
            path,
            source,
        });
    }
    if left.is_empty() {
        let _ = std::fs::remove_file(journal);
        return Ok(());
    }
    rewrite(journal, &left).map_err(|source| ClearFailure {
        removed: std::mem::take(removed),
        path: journal.to_path_buf(),
        source,
    })
}

fn unclaimed(journal: &Path) -> PathBuf {
    let name = journal.file_name().unwrap_or_default().to_string_lossy();
    journal.with_file_name(name.split(".claim-").next().unwrap_or_default())
}

fn kept<'a>(lines: &[&'a [u8]], keep: &[bool]) -> Vec<&'a [u8]> {
    lines
        .iter()
        .zip(keep)
        .filter(|(_, keep)| **keep)
        .map(|(line, _)| *line)
        .collect()
}

fn rewrite(journal: &Path, lines: &[&[u8]]) -> std::io::Result<()> {
    let mut body = Vec::new();
    for line in lines {
        body.extend_from_slice(line);
        body.push(b'\n');
    }
    let temp = claim_path(journal);
    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, journal)
}

/// Whether a journaled path still holds exactly what the dead run placed there.
enum Ours {
    Yes,
    No,
    Unreadable(std::io::Error),
}

fn ours(entry: &Entry) -> Ours {
    let meta = match std::fs::symlink_metadata(entry.path()) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ours::No,
        Err(error) => return unreadable(error),
    };
    match entry {
        Entry::Link { path, target } => {
            let same = meta.file_type().is_symlink()
                && std::fs::read_link(path).is_ok_and(|found| found == *target);
            verdict(same)
        }
        Entry::File { path, len, hash } => {
            if !meta.file_type().is_file() || meta.len() != *len {
                return Ours::No;
            }
            match std::fs::read(path) {
                Ok(bytes) => verdict(self::hash(&bytes) == *hash),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ours::No,
                Err(error) => unreadable(error),
            }
        }
        Entry::Dir { path } => {
            if !meta.file_type().is_dir() {
                return Ours::No;
            }
            match std::fs::read_dir(path) {
                Ok(mut entries) => verdict(entries.next().is_none()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ours::No,
                Err(error) => unreadable(error),
            }
        }
    }
}

fn verdict(ours: bool) -> Ours {
    if ours {
        Ours::Yes
    } else {
        Ours::No
    }
}

fn unreadable(error: std::io::Error) -> Ours {
    if error.kind() == std::io::ErrorKind::NotFound {
        Ours::No
    } else {
        Ours::Unreadable(error)
    }
}

/// Removes a path judged ours: a folder by `remove_dir`, everything else as a file.
fn remove_ours(entry: &Entry) -> Removal {
    let outcome = match entry {
        Entry::Dir { path } => std::fs::remove_dir(path),
        Entry::Link { path, .. } | Entry::File { path, .. } => std::fs::remove_file(path),
    };
    match outcome {
        Ok(()) => Removal::Removed,
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Removal::Skip,
        Err(error) => failed(error),
    }
}

fn failed(error: std::io::Error) -> Removal {
    if error.kind() == std::io::ErrorKind::NotFound {
        Removal::Skip
    } else {
        Removal::Failed(error)
    }
}

/// Removes a file or link an earlier run left behind because its guard never dropped.
pub fn remove_leftover(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Creates `dir` and any missing parents for a run's logs or profiles; the folder outlives the run.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mf-stage-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn dead_pid() -> u32 {
        let _spawning = crate::spawn_lock();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn digest(bytes: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn a_journaled_guard_names_this_process_and_drops_its_file() {
        let root = tmp("journal");
        std::fs::write(root.join("keep.txt"), "keep").unwrap();
        let mut staged = Staged::in_install(&root);
        staged.link(&root.join("link"), &root).unwrap();
        staged
            .copy(&root.join("keep.txt"), &root.join("copy.txt"))
            .unwrap();
        staged.create_dir(&root.join("folder")).unwrap();

        let dir = root.join(JOURNAL_NAME);
        let names = files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(
            names[0].starts_with(&format!("{}-", std::process::id()))
                && names[0].ends_with(".jsonl"),
            "{names:?}"
        );
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(dir.join(&names[0]))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0]["kind"], "link");
        assert_eq!(
            lines[0]["path"].as_str().map(Path::new),
            Some(root.join("link").as_path())
        );
        assert_eq!(
            lines[0]["target"].as_str().map(Path::new),
            Some(root.as_path())
        );
        assert_eq!(lines[1]["kind"], "file");
        assert_eq!(
            lines[1]["path"].as_str().map(Path::new),
            Some(root.join("copy.txt").as_path())
        );
        assert_eq!(lines[1]["len"], 4);
        assert_eq!(lines[1]["hash"], digest(b"keep"));
        assert_eq!(lines[2]["kind"], "dir");
        assert_eq!(
            lines[2]["path"].as_str().map(Path::new),
            Some(root.join("folder").as_path())
        );

        drop(staged);
        assert_eq!(entries(&root), vec!["keep.txt".to_string()]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_guards_in_one_process_keep_separate_journal_files() {
        let root = tmp("journal-two");
        let mut first = Staged::in_install(&root);
        let mut second = Staged::in_install(&root);
        first.write(&root.join("one"), b"one").unwrap();
        second.write(&root.join("two"), b"two").unwrap();

        let dir = root.join(JOURNAL_NAME);
        assert_eq!(files(&dir).len(), 2, "one file per guard");

        drop(first);
        assert!(root.join("two").is_file());
        assert!(!root.join("one").exists());
        assert_eq!(files(&dir).len(), 1);

        drop(second);
        assert!(!dir.exists(), "the folder goes when nothing is left");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_staging_call_that_fails_leaves_nothing_behind() {
        let root = tmp("journal-fail");
        std::fs::write(root.join("JM"), b"user").unwrap();
        let mut staged = Staged::in_install(&root);

        assert!(staged
            .create_parents(&root, Path::new("JM/CF/leaf"))
            .is_err());
        drop(staged);

        assert_eq!(entries(&root), vec!["JM".to_string()]);
        assert_eq!(std::fs::read(root.join("JM")).unwrap(), b"user");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_json_cannot_hold_is_staged_without_a_journal_line() {
        use std::os::unix::ffi::OsStrExt;
        let root = tmp("journal-odd");
        let odd = root.join(std::ffi::OsStr::from_bytes(b"odd\xff.txt"));
        let mut staged = Staged::in_install(&root);

        staged.write(&odd, b"x").unwrap();
        assert!(odd.is_file(), "a non-UTF-8 path must still be staged");
        assert!(
            files(&root.join(JOURNAL_NAME)).is_empty(),
            "no line can name that path"
        );

        drop(staged);
        assert!(odd.symlink_metadata().is_err(), "the guard must remove it");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn file_line(path: &Path, contents: &[u8]) -> String {
        serde_json::json!({
            "kind": "file",
            "path": path,
            "len": contents.len(),
            "hash": digest(contents),
        })
        .to_string()
    }

    fn link_line(path: &Path, target: &Path) -> String {
        serde_json::json!({"kind": "link", "path": path, "target": target}).to_string()
    }

    fn dir_line(path: &Path) -> String {
        serde_json::json!({"kind": "dir", "path": path}).to_string()
    }

    fn journal_bytes(root: &Path, name: &str, body: &[u8]) {
        let dir = root.join(JOURNAL_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn journal(root: &Path, name: &str, lines: &[String]) {
        let mut body = lines.join("\n");
        body.push('\n');
        journal_bytes(root, name, body.as_bytes());
    }

    fn dead_journal(root: &Path, lines: &[String]) {
        journal(root, &format!("{}-0.jsonl", dead_pid()), lines);
    }

    #[test]
    fn clear_dead_runs_removes_a_dead_runs_file_link_and_empty_folder() {
        type Case = (&'static str, fn(&Path) -> String);
        let cases: [Case; 3] = [
            ("a file", |path| {
                std::fs::write(path, b"x").unwrap();
                file_line(path, b"x")
            }),
            ("a link", |path| {
                std::os::unix::fs::symlink("/nowhere", path).unwrap();
                link_line(path, Path::new("/nowhere"))
            }),
            ("an empty folder", |path| {
                std::fs::create_dir(path).unwrap();
                dir_line(path)
            }),
        ];
        for (label, make) in cases {
            let root = tmp("clear");
            let gone = root.join("gone");
            let line = make(&gone);
            let live = root.join("live");
            std::fs::write(&live, b"live").unwrap();
            dead_journal(&root, &[line]);
            let live_name = format!("{}-0.jsonl", std::process::id());
            journal(&root, &live_name, &[file_line(&live, b"live")]);

            assert_eq!(
                clear_dead_runs(&root).unwrap(),
                vec![gone.clone()],
                "{label}"
            );
            assert!(
                gone.symlink_metadata().is_err(),
                "{label}: it is still there"
            );
            assert!(live.is_file(), "{label}: the live path was removed");
            assert_eq!(
                files(&root.join(JOURNAL_NAME)),
                vec![live_name],
                "{label}: a live run's file must stay"
            );

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn clear_dead_runs_removes_the_newest_entry_first() {
        let root = tmp("clear-order");
        let older = root.join("older");
        let newer = root.join("newer");
        std::fs::write(&older, b"older").unwrap();
        std::fs::write(&newer, b"newer").unwrap();
        dead_journal(
            &root,
            &[file_line(&older, b"older"), file_line(&newer, b"newer")],
        );

        assert_eq!(clear_dead_runs(&root).unwrap(), vec![newer, older]);
        assert!(
            !root.join(JOURNAL_NAME).exists(),
            "the folder goes when nothing is left"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_dead_runs_keeps_a_missing_path_a_malformed_line_and_bad_bytes() {
        let root = tmp("clear-keeps");
        let name = format!("{}-0.jsonl", dead_pid());
        let mut body = Vec::new();
        body.extend(file_line(&root.join("missing"), b"x").as_bytes());
        body.extend_from_slice(b"\n\nnot a journal line\n\xff bad bytes\n\n");
        journal_bytes(&root, &name, &body);

        assert!(clear_dead_runs(&root).unwrap().is_empty());
        let dir = root.join(JOURNAL_NAME);
        let names = files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        let left = std::fs::read(dir.join(&names[0])).unwrap();
        let text = String::from_utf8_lossy(&left);
        assert!(text.contains("not a journal line"), "{text}");
        assert!(left.contains(&0xff), "bad bytes are kept");
        assert!(!text.contains("missing"), "a gone path's entry is dropped");
        assert!(
            !text.contains("\n\n") && !text.starts_with('\n'),
            "a blank line would keep the file for ever: {text:?}"
        );

        // A later run claims it again: the name must not grow with every clear.
        let later = dir.join(format!("{name}.claim-{}-9", dead_pid()));
        std::fs::rename(dir.join(&names[0]), &later).unwrap();
        assert!(clear_dead_runs(&root).unwrap().is_empty());
        let names = files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(
            names[0].matches(".claim-").count(),
            1,
            "a repeated claim must not lengthen the name: {names:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_guard_keeps_its_journal_when_it_cannot_remove_what_it_made() {
        let root = tmp("drop-locked");
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap();
        let mut staged = Staged::in_install(&root);
        staged.write(&locked.join("cfg"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::remove_file(locked.join("cfg")).is_ok() {
            // Root removes anything a read-only folder holds, so there is nothing to test.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _ = std::fs::remove_dir_all(&root);
            return;
        }

        drop(staged);

        assert!(locked.join("cfg").is_file());
        assert_eq!(
            files(&root.join(JOURNAL_NAME)).len(),
            1,
            "the entry that could not be removed keeps its journal"
        );

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_dead_runs_leaves_what_the_user_put_in_place_of_its_own() {
        type Case = (&'static str, fn(&Path) -> String);
        let cases: [Case; 5] = [
            ("a file", |path| {
                std::fs::write(path, b"replaced").unwrap();
                file_line(path, b"ours")
            }),
            ("a file of the same length", |path| {
                std::fs::write(path, b"mine").unwrap();
                file_line(path, b"ours")
            }),
            ("a link", |path| {
                std::os::unix::fs::symlink("/elsewhere", path).unwrap();
                link_line(path, Path::new("/somewhere"))
            }),
            ("a folder with something in it", |path| {
                std::fs::create_dir(path).unwrap();
                std::fs::write(path.join("user.txt"), b"user").unwrap();
                dir_line(path)
            }),
            ("a link where a folder was", |path| {
                let empty = path.with_file_name("empty");
                std::fs::create_dir(&empty).unwrap();
                std::os::unix::fs::symlink(&empty, path).unwrap();
                dir_line(path)
            }),
        ];
        for (label, user_made_it) in cases {
            let root = tmp("clear-replaced");
            let path = root.join("ours");
            let line = user_made_it(&path);
            dead_journal(&root, &[line]);

            assert!(clear_dead_runs(&root).unwrap().is_empty(), "{label}");
            assert!(path.symlink_metadata().is_ok(), "{label}: it must survive");
            assert!(
                files(&root.join(JOURNAL_NAME)).is_empty(),
                "{label}: the entry is dropped"
            );

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn clear_dead_runs_ignores_a_parent_path_and_a_path_outside_the_install() {
        let root = tmp("clear-outside");
        let foreign = tmp("clear-foreign");
        let outside = foreign.join("user.txt");
        std::fs::write(&outside, b"user").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let dodgy = root.join("sub").join("..").join("user.txt");
        std::fs::write(root.join("user.txt"), b"user").unwrap();
        dead_journal(
            &root,
            &[file_line(&dodgy, b"user"), file_line(&outside, b"user")],
        );

        assert!(clear_dead_runs(&root).unwrap().is_empty());
        assert!(root.join("user.txt").is_file(), "a `..` path is left alone");
        assert!(
            outside.is_file(),
            "a path outside the install is left alone"
        );
        assert!(
            files(&root.join(JOURNAL_NAME)).is_empty(),
            "both entries are dropped"
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&foreign);
    }

    #[test]
    fn clear_dead_runs_treats_a_claimed_journal_by_whoever_claimed_it() {
        for (label, claimer, stays) in [
            ("a live clear", std::process::id(), true),
            ("a dead one", dead_pid(), false),
        ] {
            let root = tmp("clear-claim");
            let path = root.join("ours");
            std::fs::write(&path, b"ours").unwrap();
            let name = format!("{}-0.jsonl.claim-{claimer}-1", dead_pid());
            journal(&root, &name, &[file_line(&path, b"ours")]);

            let removed = clear_dead_runs(&root).unwrap();
            assert_eq!(path.is_file(), stays, "{label}");
            assert_eq!(removed.is_empty(), stays, "{label}");
            if stays {
                assert_eq!(files(&root.join(JOURNAL_NAME)), vec![name], "{label}");
            }

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn clear_dead_runs_fails_on_a_removal_it_cannot_make_and_keeps_the_entry() {
        let root = tmp("clear-locked");
        let locked = root.join("keys");
        std::fs::create_dir(&locked).unwrap();
        let key = locked.join("Stale.bikey");
        std::fs::write(&key, b"key").unwrap();
        std::fs::write(locked.join("probe"), b"probe").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::remove_file(locked.join("probe")).is_ok() {
            // Root removes anything a read-only folder holds, so there is nothing to test.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let gone = root.join("gone");
        std::fs::write(&gone, b"gone").unwrap();
        journal(
            &root,
            &format!("{}-0.jsonl", dead_pid()),
            &[file_line(&key, b"key"), file_line(&gone, b"gone")],
        );

        let failure = clear_dead_runs(&root).unwrap_err();
        assert_eq!(
            failure.removed,
            vec![gone.clone()],
            "what was removed comes first"
        );
        assert_eq!(failure.path, key);
        assert!(!gone.exists());
        assert!(key.is_file(), "the entry that failed is kept for next time");
        let dir = root.join(JOURNAL_NAME);
        let names = files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        let left = std::fs::read_to_string(dir.join(&names[0])).unwrap();
        assert!(left.contains("Stale.bikey"), "{left}");
        assert!(!left.contains("gone"), "a done entry is not kept: {left}");

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_append_names_the_journal() {
        let root = tmp("journal-blocked");
        std::fs::write(root.join(JOURNAL_NAME), b"a file, not a folder").unwrap();
        let mut staged = Staged::in_install(&root);

        let error = staged
            .write(&root.join("cfg"), b"x")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(JOURNAL_NAME),
            "the error must name the journal: {error}"
        );
        assert!(!root.join("cfg").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_journal_line_never_holds_a_parent_step() {
        let root = tmp("journal-dots");
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let install = root.join("elsewhere").join("..").join("install");
        std::fs::create_dir_all(&install).unwrap();
        let cfg = install.join("cfg");
        let mut staged = Staged::in_install(&install);
        staged.write(&cfg, b"x").unwrap();

        let dir = root.join("install").join(JOURNAL_NAME);
        let names = files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        let line: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(dir.join(&names[0])).unwrap().trim())
                .unwrap();
        let path = Path::new(line["path"].as_str().unwrap());
        assert!(
            !path.components().any(|part| part == Component::ParentDir),
            "a clear would refuse this line: {path:?}"
        );

        std::mem::forget(staged);
        std::fs::rename(
            dir.join(&names[0]),
            dir.join(format!("{}-0.jsonl", dead_pid())),
        )
        .unwrap();
        assert_eq!(
            clear_dead_runs(&root.join("install")).unwrap(),
            vec![path.to_path_buf()],
            "the file a run left must still be found"
        );
        assert!(!cfg.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_dead_runs_keeps_an_entry_it_cannot_judge_and_retries_after_the_fix() {
        let root = tmp("clear-unreadable");
        let file = root.join("cfg");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&file).is_ok() {
            // Root reads anything, so there is nothing to test.
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        dead_journal(&root, &[file_line(&file, b"x")]);

        assert!(clear_dead_runs(&root).is_err());
        assert!(file.is_file(), "a path it cannot judge is left alone");

        // The same process retries: a failed clear must not stay claimed by it.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(clear_dead_runs(&root).unwrap().len(), 1);
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dropping_a_guard_keeps_no_journal_when_its_folder_still_holds_another_runs_file() {
        let root = tmp("drop-shared-dir");
        let dir = root.join("keys");
        let mut first = Staged::in_install(&root);
        first.create_dir(&dir).unwrap();
        std::fs::write(dir.join("other"), b"x").unwrap();
        drop(first);
        assert!(!root.join(JOURNAL_NAME).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dead_run_leftovers_lists_what_a_clear_then_removes_and_removes_nothing() {
        let root = tmp("leftovers");
        let ours = root.join("ours");
        std::fs::write(&ours, b"ours").unwrap();
        let replaced = root.join("replaced");
        std::fs::write(&replaced, b"mine").unwrap();
        let live = root.join("live");
        std::fs::write(&live, b"live").unwrap();
        dead_journal(
            &root,
            &[file_line(&replaced, b"ours"), file_line(&ours, b"ours")],
        );
        journal(
            &root,
            &format!("{}-0.jsonl", std::process::id()),
            &[file_line(&live, b"live")],
        );

        let leftovers = dead_run_leftovers(&root);

        assert_eq!(leftovers, vec![ours.clone()]);
        assert!(ours.is_file(), "listing must remove nothing");
        assert!(replaced.is_file());
        assert!(live.is_file());
        assert_eq!(clear_dead_runs(&root).unwrap(), leftovers);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dead_run_leftovers_lists_a_folder_the_clear_empties_first() {
        let root = tmp("leftovers-dir");
        let keys = root.join("keys");
        std::fs::create_dir(&keys).unwrap();
        let key = keys.join("a.bikey");
        std::fs::write(&key, b"key").unwrap();
        dead_journal(&root, &[dir_line(&keys), file_line(&key, b"key")]);

        let leftovers = dead_run_leftovers(&root);

        assert_eq!(leftovers, vec![key.clone(), keys.clone()]);
        assert!(key.is_file(), "listing must remove nothing");
        assert_eq!(clear_dead_runs(&root).unwrap(), leftovers);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keeping_a_guard_leaves_its_staging_for_the_dead_run_sweep() {
        let root = tmp("keep");
        let link = root.join("link");
        let mut staged = Staged::in_install(&root);
        staged.link(&link, &root).unwrap();

        staged.keep_for(None);
        assert!(
            link.symlink_metadata().is_ok(),
            "the link the game loads from must survive the guard"
        );
        let journal = root.join(JOURNAL_NAME);
        assert_eq!(
            files(&journal).len(),
            1,
            "the journal must stay for the sweep"
        );

        // Once the run that wrote the journal is gone, the next clear removes what it kept.
        let names = files(&journal);
        std::fs::rename(
            journal.join(&names[0]),
            journal.join(format!("{}-0.jsonl", dead_pid())),
        )
        .unwrap();
        assert_eq!(clear_dead_runs(&root).unwrap(), vec![link.clone()]);
        assert!(link.symlink_metadata().is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_kept_guard_handed_to_another_process_is_swept_only_once_that_process_ends() {
        let _spawning = crate::spawn_lock();
        let root = tmp("keep-owner");
        let link = root.join("link");
        let mut game = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut staged = Staged::in_install(&root);
        staged.link(&link, &root).unwrap();

        staged.keep_for(Some(game.id()));
        assert_eq!(clear_dead_runs(&root).unwrap(), Vec::<PathBuf>::new());
        assert!(link.symlink_metadata().is_ok(), "the game still runs");

        game.kill().unwrap();
        game.wait().unwrap();
        assert_eq!(clear_dead_runs(&root).unwrap(), vec![link.clone()]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_dead_runs_without_a_journal_removes_nothing() {
        let root = tmp("clear-none");
        assert!(clear_dead_runs(&root).unwrap().is_empty());
        assert!(!root.join(JOURNAL_NAME).exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dropping_removes_only_what_it_made() {
        let root = tmp("drop");
        std::fs::write(root.join("keep.txt"), "keep").unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        let mut staged = Staged::default();
        staged.link(&root.join("link"), &root).unwrap();
        staged.write(&root.join("staged.cfg"), b"x").unwrap();
        staged
            .create_parents(&root, Path::new("a/b/c/leaf"))
            .unwrap();

        for name in ["link", "staged.cfg", "a/b/c"] {
            assert!(root.join(name).exists(), "{name} missing");
        }

        drop(staged);
        assert_eq!(
            entries(&root),
            vec!["a".to_string(), "keep.txt".to_string()]
        );
        assert!(entries(&root.join("a")).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
