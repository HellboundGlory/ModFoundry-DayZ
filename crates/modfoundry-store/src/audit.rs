//! The append-only audit trail.
//!
//! Every dispatch through the Core Domain API records one row here: who acted,
//! what they invoked, its effect class, a hash of the arguments, and how it
//! ended. Rows are never updated or deleted — the audit trail is the evidence
//! that an agent's actions were authorized.

use crate::{effect_from_str, effect_str, Store, StoreError};
use modfoundry_core::EffectKind;
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// How a dispatched operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Ran to completion.
    Ok,
    /// Refused by the permission policy before the body ran.
    Denied,
    /// Ran and failed.
    Error,
    /// Returned a change-set without applying it (`preview`).
    Preview,
}

impl AuditOutcome {
    text_forms!(impl "audit outcome" {
        AuditOutcome::Ok => "ok",
        AuditOutcome::Denied => "denied",
        AuditOutcome::Error => "error",
        AuditOutcome::Preview => "preview",
    });
}

/// An audit row to append. `at_unix` is supplied by the caller so the recorded
/// time is the one the operation was dispatched at, not the one it persisted at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditEntry {
    pub at_unix: i64,
    pub actor: String,
    pub command: String,
    pub effect: EffectKind,
    pub args_hash: String,
    pub outcome: AuditOutcome,
    pub detail: Option<String>,
    /// `undo_log.id` this operation produced, when it recorded a change-set.
    pub change_set: Option<i64>,
}

/// A persisted audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at_unix: i64,
    pub actor: String,
    pub command: String,
    pub effect: EffectKind,
    pub args_hash: String,
    pub outcome: AuditOutcome,
    pub detail: Option<String>,
    pub change_set: Option<i64>,
}

impl Store {
    /// Append one audit row, returning its id.
    pub fn record_audit(&self, entry: &NewAuditEntry) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO audit_entry
                 (at_unix, actor, command, effect, args_hash, outcome, detail, change_set)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                entry.at_unix,
                entry.actor,
                entry.command,
                effect_str(entry.effect),
                entry.args_hash,
                entry.outcome.as_str(),
                entry.detail,
                entry.change_set,
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// The `limit` most recent audit rows, newest first.
    pub fn audit_entries(&self, limit: usize) -> Result<Vec<AuditEntry>, StoreError> {
        self.query_audit(limit, None)
    }

    /// The `limit` most recent audit rows for one actor, newest first.
    pub fn audit_entries_for(
        &self,
        actor: &str,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        self.query_audit(limit, Some(actor))
    }

    fn query_audit(
        &self,
        limit: usize,
        actor: Option<&str>,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        const COLUMNS: &str =
            "id, at_unix, actor, command, effect, args_hash, outcome, detail, change_set";
        let filter = if actor.is_some() {
            "WHERE actor = ?2"
        } else {
            ""
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM audit_entry {filter} ORDER BY at_unix DESC, id DESC LIMIT ?1"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = match actor {
            Some(actor) => stmt
                .query_map(rusqlite::params![limit as i64, actor], map_audit)?
                .collect::<Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(rusqlite::params![limit as i64], map_audit)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }
}

fn map_audit(row: &Row<'_>) -> rusqlite::Result<AuditEntry> {
    let effect: String = row.get(4)?;
    let outcome: String = row.get(6)?;
    Ok(AuditEntry {
        id: row.get(0)?,
        at_unix: row.get(1)?,
        actor: row.get(2)?,
        command: row.get(3)?,
        effect: effect_from_str(&effect).map_err(|e| crate::bad_text(4, e))?,
        args_hash: row.get(5)?,
        outcome: AuditOutcome::parse(&outcome).map_err(|e| crate::bad_text(6, e))?,
        detail: row.get(7)?,
        change_set: row.get(8)?,
    })
}
