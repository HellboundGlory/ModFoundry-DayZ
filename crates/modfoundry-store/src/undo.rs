//! The change-set stack behind `workspace.undo`.
//!
//! A mutating operation that changed user content records one row here, holding
//! the change-set needed to invert it. The row is opaque to the store: what a
//! change-set contains belongs to the VFS write API. `workspace.undo` walks the
//! stack newest-first and marks what it reversed, so a change-set is reversed at
//! most once.

use crate::{Store, StoreError};
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// A recorded, still-invertible change-set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoRecord {
    pub id: i64,
    pub at_unix: i64,
    pub actor: String,
    pub command: String,
    /// The inverting payload, as the VFS write API serialized it.
    pub changes: Value,
    /// Where the change-set's source root sat on the host, when the payload
    /// carried one; `None` for a payload that named no (or only a relative) root.
    pub host_root: Option<PathBuf>,
    pub undone: bool,
    pub undone_at: Option<i64>,
}

const COLUMNS: &str = "id, at_unix, actor, command, changes, host_root, undone, undone_at";

impl Store {
    /// Record a change-set, returning its id (the value audit rows reference).
    pub fn record_change_set(
        &self,
        actor: &str,
        command: &str,
        changes: &Value,
        at_unix: i64,
    ) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO undo_log (at_unix, actor, command, changes, host_root)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                at_unix,
                actor,
                command,
                changes.to_string(),
                changes.get("host_root").and_then(Value::as_str),
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// The newest change-set that has not been reversed — what `workspace.undo`
    /// would invert next.
    pub fn last_undoable(&self) -> Result<Option<UndoRecord>, StoreError> {
        let sql =
            format!("SELECT {COLUMNS} FROM undo_log WHERE undone = 0 ORDER BY id DESC LIMIT 1");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query_map([], map_record)?;
        rows.next().transpose().map_err(StoreError::from)
    }

    /// The newest not-undone change-set whose `host_root` lies under `root`, both
    /// resolved through links (a missing tail kept under its nearest existing
    /// folder). A relative, `..`-holding or unresolvable `host_root` never matches,
    /// and a relative or unresolvable `root` finds nothing.
    pub fn last_undoable_within(&self, root: &Path) -> Result<Option<UndoRecord>, StoreError> {
        if !root.is_absolute() {
            return Ok(None);
        }
        let Some(root) = resolve(root) else {
            return Ok(None);
        };
        let matched_id = {
            let mut stmt = self
                .conn()
                .prepare("SELECT id, host_root FROM undo_log WHERE undone = 0 ORDER BY id DESC")?;
            let mut rows = stmt.query([])?;
            let mut matched_id = None;
            while let Some(row) = rows.next()? {
                if let Some(host_root) = row.get::<_, Option<String>>(1)? {
                    let host_root = PathBuf::from(host_root);
                    let climbs_out = host_root.components().any(|c| c == Component::ParentDir);
                    if !host_root.is_absolute() || climbs_out {
                        continue;
                    }
                    let matched =
                        resolve(&host_root).is_some_and(|host_root| host_root.starts_with(&root));
                    if matched {
                        matched_id = Some(row.get::<_, i64>(0)?);
                        break;
                    }
                }
            }
            matched_id
        };
        let Some(id) = matched_id else {
            return Ok(None);
        };
        let sql = format!("SELECT {COLUMNS} FROM undo_log WHERE id = ?1");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query_map([id], map_record)?;
        rows.next().transpose().map_err(StoreError::from)
    }

    /// Mark a change-set as reversed, reporting whether this call is what
    /// reversed it (a second attempt returns `false`).
    pub fn mark_undone(&self, id: i64, at_unix: i64) -> Result<bool, StoreError> {
        let updated = self.conn().execute(
            "UPDATE undo_log SET undone = 1, undone_at = ?1 WHERE id = ?2 AND undone = 0",
            rusqlite::params![at_unix, id],
        )?;
        Ok(updated > 0)
    }
}

/// `path` with every link followed, keeping a tail that does not exist yet as
/// written under the nearest folder that does. `None` when it cannot be
/// resolved: a dangling link, or an error other than a missing path.
fn resolve(path: &Path) -> Option<PathBuf> {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        match std::fs::canonicalize(current) {
            Ok(mut resolved) => {
                for name in tail.iter().rev() {
                    resolved.push(name);
                }
                return Some(resolved);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                if std::fs::symlink_metadata(current).is_ok() {
                    return None;
                }
                tail.push(current.file_name()?.to_os_string());
                current = current.parent()?;
            }
            Err(_) => return None,
        }
    }
}

fn map_record(row: &Row<'_>) -> rusqlite::Result<UndoRecord> {
    let changes: String = row.get(4)?;
    Ok(UndoRecord {
        id: row.get(0)?,
        at_unix: row.get(1)?,
        actor: row.get(2)?,
        command: row.get(3)?,
        changes: serde_json::from_str(&changes).map_err(|e| crate::bad_text(4, e))?,
        host_root: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
        undone: row.get::<_, i64>(6)? != 0,
        undone_at: row.get(7)?,
    })
}
