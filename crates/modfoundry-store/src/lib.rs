//! ModFoundry local persistence.
//!
//! One SQLite database per project plus an application-level database hold the
//! state that must outlive a session: the policy rules that authorize an agent's
//! calls, the append-only audit trail every dispatch writes, the change-set stack
//! `workspace.undo` pops, and settings.
//!
//! The database is a *cache of derivable truth* — files stay the source of truth —
//! so schema changes are code ([`Migration`]) applied on open, and an unreadable
//! database is always safe to delete. Two rules are enforced here: WAL
//! journaling with a busy timeout (concurrent GUI + agent + CLI access), and a
//! refusal to open a database written by a newer build.
//!
//! Change-set payloads are stored as opaque JSON: what a change-set *contains* is
//! the VFS write API's business, not the store's.

/// Writes a type's persisted text form and its inverse from one table, so a variant cannot be added to one
/// direction and missed in the other. An unknown text is `StoreError::Invalid("unknown <noun>: <text>")`.
macro_rules! text_forms {
    (impl $noun:literal { $($variant:path => $text:literal),+ $(,)? }) => {
        /// The persisted text form.
        pub fn as_str(self) -> &'static str {
            match self {
                $($variant => $text,)+
            }
        }

        /// Parse the persisted text form.
        pub fn parse(s: &str) -> Result<Self, $crate::StoreError> {
            match s {
                $($text => Ok($variant),)+
                other => Err($crate::StoreError::Invalid(format!(
                    concat!("unknown ", $noun, ": {}"),
                    other
                ))),
            }
        }
    };
    ($noun:literal, $ty:ty, $to:ident, $from:ident { $($variant:path => $text:literal),+ $(,)? }) => {
        /// Stable text form as persisted in policy and audit rows.
        pub fn $to(value: $ty) -> &'static str {
            match value {
                $($variant => $text,)+
            }
        }

        /// Inverse of the text form.
        pub fn $from(s: &str) -> Result<$ty, $crate::StoreError> {
            match s {
                $($text => Ok($variant),)+
                other => Err($crate::StoreError::Invalid(format!(
                    concat!("unknown ", $noun, ": {}"),
                    other
                ))),
            }
        }
    };
}

pub mod audit;
pub mod confirm;
pub mod policy;
pub mod run;
pub mod schema;
pub mod settings;
pub mod undo;

use modfoundry_core::EffectKind;
use rusqlite::Connection;
use std::path::Path;

/// Schema version this build writes and can read.
pub const SCHEMA_VERSION: u32 = 6;

use schema::MIGRATIONS;

/// Errors surfaced by the store. Callers (the agent/command layers) map these
/// onto their own presentation and must not need to know `rusqlite`.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("malformed stored value: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    /// A vocabulary string this build does not know — an effect class, a
    /// decision, an actor key — whether it came from a row or from a caller
    /// that typed it.
    #[error("invalid value: {0}")]
    Invalid(String),
    /// A run id another run is still using.
    #[error("run `{0}` is already running")]
    RunLive(String),
}

/// A handle to one ModFoundry database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if absent) the database at `path`, then bring its schema up
    /// to [`SCHEMA_VERSION`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_conn(Connection::open(path)?)
    }

    /// An in-memory database with the same schema, for tests and for hosts that
    /// want no persistence.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(mut conn: Connection) -> Result<Self, StoreError> {
        // `journal_mode` returns the resulting mode, so it cannot go through
        // `execute_batch`; an in-memory database answers `memory` (WAL needs a file).
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        enable_wal(&conn)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The schema version currently persisted in the database.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    /// The active journal mode (`wal` for file databases).
    pub fn journal_mode(&self) -> Result<String, StoreError> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?)
    }

    /// Fold the write-ahead log back into the database and truncate it.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        let _: (i64, i64, i64) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        Ok(())
    }
}

/// Switch to WAL. On a fresh file another opener may hold the lock this needs,
/// and SQLite answers that with an immediate BUSY rather than the busy handler,
/// so the retry is ours.
fn enable_wal(conn: &Connection) -> Result<(), StoreError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other.map(|_| ()).map_err(StoreError::from),
        }
    }
}

/// Apply every migration newer than what is persisted, each in its own
/// transaction, recording the version it produced.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        // IMMEDIATE takes the write lock up front, and the version is re-read
        // under it, so two processes opening one fresh file apply each step once.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let applied: u32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if applied < migration.version {
            tx.execute_batch(migration.sql)?;
            tx.pragma_update(None, "user_version", migration.version)?;
        }
        tx.commit()?;
    }
    Ok(())
}

text_forms!("effect class", EffectKind, effect_str, effect_from_str {
    EffectKind::Read => "read",
    EffectKind::IndexWrite => "index_write",
    EffectKind::Mutate => "mutate",
    EffectKind::Destructive => "destructive",
    EffectKind::Launch => "launch",
    EffectKind::Network => "network",
    EffectKind::Publish => "publish",
});

/// Wrap a text-column conversion failure as a `rusqlite` conversion error.
pub(crate) fn bad_text(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_the_last_migration() {
        let last = MIGRATIONS.last().expect("at least one migration");
        assert_eq!(last.version, SCHEMA_VERSION);
    }

    #[test]
    fn migrations_are_strictly_ordered_from_one() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version as usize, index + 1);
        }
    }
}
