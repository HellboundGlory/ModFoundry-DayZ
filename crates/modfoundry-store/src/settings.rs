//! Key/value settings, global or scoped to one workspace.
//!
//! Values are JSON so a setting can grow structure without a migration. Scopes
//! are disjoint: a workspace inherits nothing implicitly from the global scope,
//! which keeps "what does this workspace override" answerable.

use crate::{Store, StoreError};
use rusqlite::OptionalExtension;
use serde_json::Value;

/// Which settings namespace a key lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Host-wide settings, independent of any open workspace.
    Global,
    /// Settings belonging to the workspace with this id.
    Workspace(String),
}

impl Scope {
    /// The persisted scope key.
    pub fn key(&self) -> String {
        match self {
            Scope::Global => "global".to_owned(),
            Scope::Workspace(id) => format!("workspace:{id}"),
        }
    }
}

impl Store {
    /// Write a setting, replacing any previous value for the key.
    pub fn set_setting(&self, scope: &Scope, key: &str, value: &Value) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO settings (scope, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![scope.key(), key, value.to_string()],
        )?;
        Ok(())
    }

    /// Read one setting.
    pub fn get_setting(&self, scope: &Scope, key: &str) -> Result<Option<Value>, StoreError> {
        let stored: Option<String> = self
            .conn()
            .query_row(
                "SELECT value FROM settings WHERE scope = ?1 AND key = ?2",
                rusqlite::params![scope.key(), key],
                |row| row.get(0),
            )
            .optional()?;
        stored
            .map(|text| serde_json::from_str(&text).map_err(StoreError::from))
            .transpose()
    }
}
