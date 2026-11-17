//! Confirmation requests for policy `ask` decisions, answered across processes
//! through the shared database. First writer wins.

use crate::{effect_from_str, effect_str, Store, StoreError};
use modfoundry_core::EffectKind;
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// What a frontend decided about a pending confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmAnswer {
    /// Run this one call.
    Once,
    /// Run it, and later matching calls for the rest of the session.
    Session,
    /// Refuse it.
    Deny,
}

impl ConfirmAnswer {
    text_forms!(impl "confirmation answer" {
        ConfirmAnswer::Once => "once",
        ConfirmAnswer::Session => "session",
        ConfirmAnswer::Deny => "deny",
    });
}

/// A confirmation request to raise. `at_unix` is supplied by the caller so the
/// recorded time is when the call was dispatched, not when it was persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConfirmation {
    pub at_unix: i64,
    pub actor: String,
    pub command: String,
    pub effect: EffectKind,
    pub args_hash: String,
    pub summary: String,
}

/// A persisted confirmation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub id: i64,
    pub at_unix: i64,
    pub actor: String,
    pub command: String,
    pub effect: EffectKind,
    pub args_hash: String,
    pub summary: String,
    /// `None` while the request is unanswered.
    pub answer: Option<ConfirmAnswer>,
    pub answered_at: Option<i64>,
}

const COLUMNS: &str =
    "id, at_unix, actor, command, effect, args_hash, summary, answer, answered_at";

impl Store {
    /// Raise a confirmation request, returning its id.
    pub fn request_confirmation(&self, request: &NewConfirmation) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO confirmation_request
                 (at_unix, actor, command, effect, args_hash, summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                request.at_unix,
                request.actor,
                request.command,
                effect_str(request.effect),
                request.args_hash,
                request.summary,
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// One request by id.
    pub fn confirmation(&self, id: i64) -> Result<Option<Confirmation>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM confirmation_request WHERE id = ?1");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query_map(rusqlite::params![id], map_confirmation)?;
        rows.next().transpose().map_err(StoreError::from)
    }

    /// Every unanswered request, oldest first.
    pub fn pending_confirmations(&self) -> Result<Vec<Confirmation>, StoreError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM confirmation_request WHERE answer IS NULL ORDER BY at_unix, id"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let pending = stmt
            .query_map([], map_confirmation)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(pending)
    }

    /// Answer a request, reporting whether this call is the one that answered it.
    /// A second answer returns `false` and leaves the stored one alone.
    pub fn answer_confirmation(
        &self,
        id: i64,
        answer: ConfirmAnswer,
        at_unix: i64,
    ) -> Result<bool, StoreError> {
        let updated = self.conn().execute(
            "UPDATE confirmation_request SET answer = ?1, answered_at = ?2
             WHERE id = ?3 AND answer IS NULL",
            rusqlite::params![answer.as_str(), at_unix, id],
        )?;
        Ok(updated > 0)
    }
}

fn map_confirmation(row: &Row<'_>) -> rusqlite::Result<Confirmation> {
    let effect: String = row.get(4)?;
    let answer: Option<String> = row.get(7)?;
    Ok(Confirmation {
        id: row.get(0)?,
        at_unix: row.get(1)?,
        actor: row.get(2)?,
        command: row.get(3)?,
        effect: effect_from_str(&effect).map_err(|e| crate::bad_text(4, e))?,
        args_hash: row.get(5)?,
        summary: row.get(6)?,
        answer: answer
            .map(|text| ConfirmAnswer::parse(&text))
            .transpose()
            .map_err(|e| crate::bad_text(7, e))?,
        answered_at: row.get(8)?,
    })
}
