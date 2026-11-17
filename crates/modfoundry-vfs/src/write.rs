//! The write side of the VFS: change-sets and the transaction that produces
//! them.
//!
//! Everything ModFoundry writes into a mod tree goes through [`WriteTxn`]: it
//! addresses files by virtual path under one writable root, refuses a set that
//! would collide case-insensitively with what is already there (DayZ resolves
//! case-insensitively, a Linux host does not), writes each file atomically
//! (temp + rename in the same directory), and reports the [`ChangeSet`] it
//! produced so the operation can be undone.
//!
//! A change-set is the *inverse payload*, not a diff: for every file it records
//! what was replaced, so [`ChangeSet::undo`] restores it. It is the schema
//! `modfoundry-store` keeps opaque in `undo_log.changes` and `workspace.undo`
//! inverts, so the same set is both the preview a caller inspects and the record
//! the host replays.
//!
//! Two rules keep that record safe to keep around. A change-set lives in the
//! database for as long as the user keeps it, so [`ChangeSet::undo`] checks that
//! every file still holds what the change wrote (its SHA-256) and refuses the
//! whole undo when someone has edited one since — it never clobbers a newer
//! edit. And content that must not be persisted *anywhere* else (a signing key)
//! is written with [`WriteTxn::write_unretained`], which records the path and
//! the operation but keeps neither what it replaced nor what it restored. On
//! Unix the file it creates is readable by its owner only.

use crate::path::VirtualPath;
use crate::VfsError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// What a change did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    /// The file did not exist and was written.
    Create,
    /// The file existed and was replaced.
    Update,
    /// The file existed and was removed.
    Delete,
}

/// One file in a change-set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Virtual path inside the change-set's root.
    pub path: VirtualPath,
    pub op: ChangeOp,
    /// The content this change replaced or removed, kept so it can be inverted.
    /// `None` for a [`ChangeOp::Create`], a removed symlink, a removed private
    /// key, and a transaction that retained nothing (a regenerable output, a
    /// dry run, or [`WriteTxn::write_unretained`]). Base64 in JSON, since mod
    /// content is not always text.
    #[serde(default, with = "base64_blob")]
    pub before: Option<Vec<u8>>,
    /// SHA-256 (hex) of the content this change wrote (`None` for a
    /// [`ChangeOp::Delete`], which wrote nothing), so an undo can tell a
    /// file it still owns from one somebody edited afterwards. A digest, never
    /// the content: cheap to keep, and safe to keep even for material that is
    /// not.
    #[serde(default)]
    pub after_sha256: Option<String>,
    /// Set when the change removed something an undo deliberately does not bring
    /// back (a symlink): the undo leaves it removed and still undoes the rest of
    /// the set.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip_on_undo: bool,
}

/// A reversible record of what one operation changed under one writable root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// The writable root every entry belongs to (`mod:MyMod`, `stage:MyMod`).
    pub root: String,
    /// Where that root sat on the host when the change was made: an undo replays
    /// a change where it happened, never wherever the caller happens to be.
    pub host_root: PathBuf,
    pub changes: Vec<FileChange>,
}

impl ChangeSet {
    pub fn new(root: impl Into<String>, host_root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            host_root: host_root.into(),
            changes: Vec::new(),
        }
    }

    pub fn files(&self) -> &[FileChange] {
        &self.changes
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Invert the change-set: restore what each change replaced or removed and
    /// delete what it created. Nothing is touched unless the whole set passes —
    /// every path must be one this root can address, every file must still hold
    /// what the change wrote (a removed one must still be absent), and every
    /// replacement must still have the content it replaced. A file edited since the change-set was recorded makes the undo
    /// [`VfsError::Stale`] rather than destroying that edit. A change marked
    /// [`FileChange::skip_on_undo`] is left as it is.
    pub fn undo(&self) -> Result<usize, VfsError> {
        for change in &self.changes {
            host_path(&self.host_root, &self.root, &change.path)?;
        }

        let mut ops = Vec::with_capacity(self.changes.len());
        let mut checked = HashSet::new();
        // Newest first, so undoing a set that wrote a path twice ends on the
        // content that preceded the set. Only the newest change to a path holds
        // the digest of what is on disk now.
        for change in self.changes.iter().rev() {
            let newest = checked.insert(change.path.as_str());
            if change.skip_on_undo {
                continue;
            }
            if newest {
                self.check_unchanged(change)?;
            }
            let restore = match (change.op, &change.before) {
                (ChangeOp::Create, _) => None,
                (ChangeOp::Update | ChangeOp::Delete, Some(before)) => Some(before.clone()),
                (ChangeOp::Update | ChangeOp::Delete, None) => {
                    return Err(VfsError::Incomplete(format!(
                        "{} was changed without keeping its previous content",
                        change.path
                    )))
                }
            };
            ops.push(Op {
                path: change.path.clone(),
                content: restore,
                owner_only: false,
            });
        }
        let targets = preflight_all(&self.root, &self.host_root, &ops)?;
        apply(&self.host_root, &ops, &targets)
    }

    /// Whether the file still holds what this change left. A `Create` whose file
    /// is already gone is fine, and so is a `Delete` whose file is still absent:
    /// an undo has nothing to delete, or it restores what was removed. Anything
    /// else that does not match the recorded digest is somebody else's edit,
    /// which an undo must not overwrite.
    fn check_unchanged(&self, change: &FileChange) -> Result<(), VfsError> {
        let host = host_path(&self.host_root, &self.root, &change.path)?;
        let unchanged = if change.op == ChangeOp::Delete {
            matches!(std::fs::symlink_metadata(&host), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        } else {
            let expected = change.after_sha256.as_deref().ok_or_else(|| {
                VfsError::Incomplete(format!(
                    "{} does not record what this change wrote",
                    change.path
                ))
            })?;
            match std::fs::read(&host) {
                Ok(current) => sha256_hex(&current) == expected,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound && change.op == ChangeOp::Create,
            }
        };
        if unchanged {
            Ok(())
        } else {
            Err(VfsError::Stale(format!(
                "{} changed after this change-set was recorded",
                change.path
            )))
        }
    }
}

/// A transaction over one writable root. Every `write` is recorded; [`apply_pending`]
/// and [`finish`] preflight and apply the writes not yet applied.
///
/// [`apply_pending`]: WriteTxn::apply_pending
///
/// [`finish`]: WriteTxn::finish
pub struct WriteTxn {
    root: String,
    host_root: PathBuf,
    retain: bool,
    apply: bool,
    pending: Vec<Pending>,
    recorded: Vec<FileChange>,
}

/// One write (`Some`) or delete (`None`) that [`apply`] carries out.
struct Op {
    path: VirtualPath,
    content: Option<Vec<u8>>,
    owner_only: bool,
}

struct Pending {
    change: FileChange,
    /// `None` deletes the file; a dry run never reaches the apply step.
    content: Option<Vec<u8>>,
    /// When set, the write only happens if the file still holds this digest:
    /// the base revision the caller planned against.
    expected_sha256: Option<String>,
    owner_only: bool,
}

/// What an earlier queued change leaves at a path when the set finally runs.
enum Queued<'a> {
    Written { content: &'a [u8], owner_only: bool },
    Removed,
}

impl WriteTxn {
    /// A transaction that writes and keeps what it replaced, so its change-set
    /// can be undone. What a source-content mutation wants.
    pub fn new(root: impl Into<String>, host_root: impl Into<PathBuf>) -> Self {
        Self::with(root, host_root, true, true)
    }

    /// A transaction that writes without keeping content: for regenerable
    /// outputs (build staging), where the undo payload would be the largest
    /// thing the host stores and the output can simply be rebuilt.
    pub fn unrecorded(root: impl Into<String>, host_root: impl Into<PathBuf>) -> Self {
        Self::with(root, host_root, false, true)
    }

    /// A transaction that records what a set *would* change and writes nothing.
    /// The shape of a change-set (`preview`) without its content.
    pub fn dry_run(root: impl Into<String>, host_root: impl Into<PathBuf>) -> Self {
        Self::with(root, host_root, false, false)
    }

    fn with(
        root: impl Into<String>,
        host_root: impl Into<PathBuf>,
        retain: bool,
        apply: bool,
    ) -> Self {
        Self {
            root: root.into(),
            host_root: host_root.into(),
            retain,
            apply,
            pending: Vec::new(),
            recorded: Vec::new(),
        }
    }

    /// Record a write of `rel` (relative to the root). The content it replaces
    /// is read now — before the set is applied — so the change can be inverted.
    pub fn write(
        &mut self,
        rel: impl AsRef<str>,
        content: impl Into<Vec<u8>>,
    ) -> Result<(), VfsError> {
        self.record(rel, content, self.retain, None, false)
    }

    /// Record the removal of `rel` (relative to the root) if the file exists on
    /// disk or an earlier write in this set creates it.
    pub fn remove(&mut self, rel: impl AsRef<str>) -> Result<(), VfsError> {
        let path = self.path(rel.as_ref())?;
        let host = host_path(&self.host_root, &self.root, &path)?;
        let (before, skip_on_undo) = match self.queued_state(&path) {
            // Nothing to do: the set already takes this path out.
            Some(Queued::Removed) => return Ok(()),
            // The file is not on disk yet, so its content is what the queue holds.
            Some(Queued::Written {
                content,
                owner_only,
            }) => {
                let keep = self.retain && !owner_only && !is_private_key(&path);
                (keep.then(|| content.to_vec()), false)
            }
            None => {
                let meta = match std::fs::symlink_metadata(&host) {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(e.into()),
                };
                let symlink = meta.file_type().is_symlink();
                // A symlink's target may lie outside the root; its bytes, and a
                // private key's, are never kept.
                let keep = self.retain && !symlink && !is_private_key(&path);
                (keep.then(|| std::fs::read(&host)).transpose()?, symlink)
            }
        };
        self.pending.push(Pending {
            change: FileChange {
                path,
                op: ChangeOp::Delete,
                before,
                after_sha256: None,
                skip_on_undo,
            },
            content: None,
            expected_sha256: None,
            owner_only: false,
        });
        Ok(())
    }

    /// What the queue leaves at `path`, newest first. Only the writes not yet
    /// applied count: what an earlier `apply_pending` applied is on the disk.
    fn queued_state(&self, path: &VirtualPath) -> Option<Queued<'_>> {
        self.pending.iter().rev().find_map(|pending| {
            if pending.change.path != *path {
                return None;
            }
            Some(match &pending.content {
                Some(content) => Queued::Written {
                    content,
                    owner_only: pending.owner_only,
                },
                None => Queued::Removed,
            })
        })
    }

    /// Record a write that only applies if `rel` still holds `expected` — the
    /// digest the caller read (a [`FileChange::after_sha256`]). A file somebody
    /// edited in between, or one that is not there at all, makes the whole batch
    /// [`VfsError::Stale`] with nothing written, so a caller that was shown old
    /// content cannot land on top of newer content.
    ///
    /// The check runs with the rest of the preflight, so a [`dry_run`] reports
    /// the same staleness an apply would refuse on.
    ///
    /// [`dry_run`]: WriteTxn::dry_run
    pub fn write_checked(
        &mut self,
        rel: impl AsRef<str>,
        content: impl Into<Vec<u8>>,
        expected: &str,
    ) -> Result<(), VfsError> {
        self.record(rel, content, self.retain, Some(expected.to_owned()), false)
    }

    /// Record a write whose replaced content is deliberately not kept: the
    /// change-set still names the file — and an undo still deletes one this
    /// change created, once it has checked the file is unchanged — but nothing
    /// of what it replaced can be read back out of the undo log. For material
    /// that must not be persisted anywhere else, like a signing key; on Unix the
    /// file is created readable by its owner only.
    pub fn write_unretained(
        &mut self,
        rel: impl AsRef<str>,
        content: impl Into<Vec<u8>>,
    ) -> Result<(), VfsError> {
        self.record(rel, content, false, None, true)
    }

    fn record(
        &mut self,
        rel: impl AsRef<str>,
        content: impl Into<Vec<u8>>,
        retain: bool,
        expected_sha256: Option<String>,
        owner_only: bool,
    ) -> Result<(), VfsError> {
        let path = self.path(rel.as_ref())?;
        let host = host_path(&self.host_root, &self.root, &path)?;
        // Existence decides create-vs-update, and only that: a transaction that
        // is not keeping the previous content must not read it (build staging
        // would otherwise pull a whole PBO through memory for nothing).
        let op = match std::fs::symlink_metadata(&host) {
            Ok(_) => ChangeOp::Update,
            Err(_) => ChangeOp::Create,
        };
        let before = match (retain, op) {
            // Content that cannot be read is not retained, and does not turn the
            // change into a `Create` either: the preflight in `finish` reports
            // what is actually wrong with the target.
            (true, ChangeOp::Update) if !is_private_key(&path) => std::fs::read(&host).ok(),
            _ => None,
        };
        let content = content.into();
        self.pending.push(Pending {
            change: FileChange {
                path,
                op,
                before,
                after_sha256: Some(sha256_hex(&content)),
                skip_on_undo: false,
            },
            content: Some(content),
            expected_sha256,
            owner_only,
        });
        Ok(())
    }

    /// Preflight and apply the writes recorded so far, then drop their content
    /// from memory and keep their records. A later failure does not roll these back.
    /// A dry run only preflights. An error leaves the writes queued, so `finish`
    /// tries them again or fails, and never succeeds without them.
    pub fn apply_pending(&mut self) -> Result<(), VfsError> {
        let mut changes = Vec::with_capacity(self.pending.len());
        let mut ops = Vec::with_capacity(self.pending.len());
        let mut expected = Vec::with_capacity(self.pending.len());
        for Pending {
            change,
            content,
            expected_sha256,
            owner_only,
        } in std::mem::take(&mut self.pending)
        {
            ops.push(Op {
                path: change.path.clone(),
                content,
                owner_only,
            });
            expected.push(expected_sha256);
            changes.push(change);
        }
        if let Err(e) = self.check_and_apply(&ops, &expected) {
            self.pending = queued(changes, ops, expected);
            return Err(e);
        }
        self.recorded.extend(changes);
        Ok(())
    }

    fn check_and_apply(&self, ops: &[Op], expected: &[Option<String>]) -> Result<(), VfsError> {
        // The whole batch is verified before the first byte is written, checked
        // writes included, so a stale base revision refuses the entire batch.
        for (op, wanted) in ops.iter().zip(expected) {
            let Some(wanted) = wanted else {
                continue;
            };
            let path = &op.path;
            let host = host_path(&self.host_root, &self.root, path)?;
            let actual = std::fs::read(&host).map_err(|_| {
                VfsError::Stale(format!(
                    "{path} is not the content this change was planned against"
                ))
            })?;
            if sha256_hex(&actual) != *wanted {
                return Err(VfsError::Stale(format!(
                    "{path} changed since this change was planned"
                )));
            }
        }
        let targets = preflight_all(&self.root, &self.host_root, ops)?;
        if self.apply {
            apply(&self.host_root, ops, &targets)?;
        }
        Ok(())
    }

    /// Apply the writes not yet applied, returning the change-set of every recorded
    /// write that inverts it. A dry run returns the same shape without writing.
    pub fn finish(mut self) -> Result<ChangeSet, VfsError> {
        self.apply_pending()?;
        Ok(ChangeSet {
            root: self.root,
            host_root: self.host_root,
            changes: self.recorded,
        })
    }

    fn path(&self, rel: &str) -> Result<VirtualPath, VfsError> {
        let cleaned = VirtualPath::clean(rel).map_err(VfsError::InvalidPath)?;
        if cleaned.is_empty() {
            return Err(VfsError::InvalidPath(format!(
                "empty path is not writable: {rel:?}"
            )));
        }
        VirtualPath::parse(&format!("{}:{cleaned}", self.root)).map_err(VfsError::InvalidPath)
    }
}

/// Put a batch that did not go through back in the queue, keeping the content
/// the ops already own.
fn queued(changes: Vec<FileChange>, ops: Vec<Op>, expected: Vec<Option<String>>) -> Vec<Pending> {
    changes
        .into_iter()
        .zip(ops)
        .zip(expected)
        .map(|((change, op), expected_sha256)| Pending {
            change,
            content: op.content,
            expected_sha256,
            owner_only: op.owner_only,
        })
        .collect()
}

/// The lowercase hex SHA-256 of `bytes`, the digest a [`FileChange`] records.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The root name for a bare host directory: `dir`'s own name under `namespace`
/// (`mod:MyMod` for a source tree, `stage:MyMod` for build output). Callers that
/// know the root's identity (`mod:<name>` for a scaffold) name it themselves.
pub fn root_name(namespace: &str, dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "root".into());
    format!("{namespace}:{name}")
}

/// Whether `path` names a private key, whose bytes never leave the machine they
/// were made on and so never enter a change-set.
fn is_private_key(path: &VirtualPath) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".biprivatekey"))
}

/// The host path of each op, every one preflighted, so a refused set writes nothing.
fn preflight_all(root: &str, host_root: &Path, ops: &[Op]) -> Result<Vec<PathBuf>, VfsError> {
    preflight_batch(root, host_root, ops, &mut Listings::default())
}

/// Each folder's entries by lowercase name, read once per batch, so a set of
/// many files in one folder lists it once instead of once per file.
#[derive(Default)]
struct Listings {
    dirs: HashMap<PathBuf, HashMap<String, Vec<String>>>,
    reads: usize,
}

impl Listings {
    /// The spellings of `name` in `dir`, in listing order.
    fn spellings(&mut self, dir: &Path, name: &str) -> Result<&[String], VfsError> {
        if !self.dirs.contains_key(dir) {
            let mut by_lower: HashMap<String, Vec<String>> = HashMap::new();
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries {
                    let entry = entry?;
                    let spelling = entry.file_name().to_string_lossy().into_owned();
                    by_lower
                        .entry(spelling.to_ascii_lowercase())
                        .or_default()
                        .push(spelling);
                }
            }
            self.reads += 1;
            self.dirs.insert(dir.to_path_buf(), by_lower);
        }
        Ok(self.dirs[dir]
            .get(&name.to_ascii_lowercase())
            .map(|spellings| spellings.as_slice())
            .unwrap_or_default())
    }
}

/// Preflight a batch in order, remembering which paths its earlier ops remove:
/// only a removal recorded before a write clears that write's case conflict.
fn preflight_batch(
    root: &str,
    host_root: &Path,
    ops: &[Op],
    listings: &mut Listings,
) -> Result<Vec<PathBuf>, VfsError> {
    let mut targets = Vec::with_capacity(ops.len());
    let mut removed: HashSet<PathBuf> = HashSet::new();
    for op in ops {
        let host = host_path(host_root, root, &op.path)?;
        preflight(host_root, &op.path, &host, listings, &removed)?;
        if op.content.is_none() {
            removed.insert(host.clone());
        } else {
            removed.remove(&host);
        }
        targets.push(host);
    }
    Ok(targets)
}

/// Apply `ops` (`None` = delete) to the `targets` that [`preflight_all`] returned.
fn apply(host_root: &Path, ops: &[Op], targets: &[PathBuf]) -> Result<usize, VfsError> {
    let mut changed = 0;
    for (op, host) in ops.iter().zip(targets) {
        match &op.content {
            Some(bytes) => {
                write_atomic(host, bytes, op.owner_only)?;
                changed += 1;
            }
            None => {
                match std::fs::remove_file(host) {
                    Ok(()) => changed += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                prune_empty_dirs(host.parent(), host_root);
            }
        }
    }
    Ok(changed)
}

/// The host path of `path`, refusing anything that is not a plain relative path
/// under `root`. A change-set can arrive from the database or an agent, so its
/// paths are input, not truth.
fn host_path(host_root: &Path, root: &str, path: &VirtualPath) -> Result<PathBuf, VfsError> {
    let rel = path
        .as_str()
        .strip_prefix(&format!("{root}:"))
        .ok_or_else(|| VfsError::OutsideRoot(format!("{path} is not under {root}")))?;
    let mut host = host_root.to_path_buf();
    for component in rel.split('/') {
        let portable = !component.is_empty()
            && component != "."
            && component != ".."
            && !component.contains([':', '\\', '\0'])
            && !component.chars().any(char::is_control);
        if !portable {
            return Err(VfsError::InvalidPath(format!(
                "{path}: `{component}` is not a plain path component"
            )));
        }
        host.push(component);
    }
    Ok(host)
}

/// Whether replacing `host` is allowed: each existing component must be a
/// directory (except the file being replaced), and no sibling may differ only in
/// case — that is a distinct file on Linux and the same file to the engine. A
/// sibling an earlier op of the same batch removes no longer counts.
fn preflight(
    host_root: &Path,
    path: &VirtualPath,
    host: &Path,
    listings: &mut Listings,
    removed: &HashSet<PathBuf>,
) -> Result<(), VfsError> {
    let rel = host
        .strip_prefix(host_root)
        .map_err(|_| VfsError::OutsideRoot(host.display().to_string()))?;
    let components: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if components.is_empty() {
        return Err(VfsError::InvalidPath(format!("{path} names the root")));
    }

    let mut dir = host_root.to_path_buf();
    for (i, component) in components.iter().enumerate() {
        let last = i + 1 == components.len();
        let target = dir.join(component);
        match std::fs::symlink_metadata(&target) {
            Ok(md) if md.file_type().is_dir() && last => {
                return Err(VfsError::InvalidPath(format!(
                    "{path} is an existing directory"
                )))
            }
            Ok(md) if !md.file_type().is_dir() && !last => {
                return Err(VfsError::InvalidPath(format!(
                    "{path}: {} is not a directory",
                    target.display()
                )))
            }
            _ => {}
        }
        for sibling in listings.spellings(&dir, component)? {
            if sibling != component && !removed.contains(&dir.join(sibling)) {
                return Err(VfsError::CaseConflict(format!(
                    "{path} collides with {} (DayZ resolves case-insensitively)",
                    dir.join(sibling).display()
                )));
            }
        }
        dir = target;
    }
    Ok(())
}

/// Write `bytes` to `host` atomically: a staging file in the same directory,
/// renamed over the target, so a reader never sees a half-written file.
fn write_atomic(host: &Path, bytes: &[u8], owner_only: bool) -> Result<(), VfsError> {
    if let Some(parent) = host.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = staging_path(host);
    let written = if owner_only {
        modfoundry_platform::write_owner_only(&staging, bytes)
    } else {
        std::fs::write(&staging, bytes)
    };
    // A half-written staging file can hold part of a private key.
    if let Err(e) = written.and_then(|()| std::fs::rename(&staging, host)) {
        let _ = std::fs::remove_file(&staging);
        return Err(e.into());
    }
    Ok(())
}

fn staging_path(host: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = host
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    host.with_file_name(format!(
        ".{name}.mf-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Remove directories an undo emptied, up to (but never including) the root.
fn prune_empty_dirs(mut dir: Option<&Path>, host_root: &Path) {
    while let Some(current) = dir {
        if current == host_root || !current.starts_with(host_root) {
            return;
        }
        if std::fs::remove_dir(current).is_err() {
            return;
        }
        dir = current.parent();
    }
}

/// `Option<Vec<u8>>` as base64 (or `null`), so binary content survives JSON.
mod base64_blob {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => serializer.serialize_some(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(deserializer)? {
            Some(encoded) => STANDARD
                .decode(encoded)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_pending_writes_what_is_recorded_and_finish_keeps_every_record() {
        let dir = std::env::temp_dir().join(format!("mf-apply-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut txn = WriteTxn::new("mod:Pending", &dir);
        txn.write("a.txt", "first").unwrap();
        txn.apply_pending().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "first");

        txn.write("b/c.txt", "second").unwrap();
        let change_set = txn.finish().unwrap();
        assert_eq!(change_set.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.join("b/c.txt")).unwrap(),
            "second"
        );

        change_set.undo().unwrap();
        assert!(!dir.join("a.txt").exists());
        assert!(!dir.join("b").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_apply_pending_writes_nothing_and_keeps_the_record() {
        let dir = std::env::temp_dir().join(format!("mf-dry-apply-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut txn = WriteTxn::dry_run("mod:Dry", &dir);
        txn.write("a.txt", "first").unwrap();
        txn.apply_pending().unwrap();
        assert!(!dir.join("a.txt").exists());
        assert_eq!(txn.finish().unwrap().len(), 1);
    }

    #[test]
    fn preflight_reads_each_folder_once() {
        let dir =
            std::env::temp_dir().join(format!("mf-preflight-listings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("Sub/g{i}.txt")), b"x").unwrap();
        }

        let ops: Vec<Op> = (0..50)
            .map(|i| Op {
                path: VirtualPath::parse(&format!("mod:T:Sub/f{i}.txt")).unwrap(),
                content: Some(b"x".to_vec()),
                owner_only: false,
            })
            .collect();
        let mut listings = Listings::default();
        let targets = preflight_batch("mod:T", &dir, &ops, &mut listings).unwrap();

        assert_eq!(targets.len(), 50);
        assert_eq!(listings.reads, 2, "the root and Sub, once each");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_hex_matches_the_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
