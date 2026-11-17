//! Launches shared through the database, so the GUI, `mf` and the agent server see the same runs. A row
//! its launcher has stopped touching is not live: that launcher died.

use crate::{Store, StoreError};
use rusqlite::{OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a run is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Its launcher is still working on it.
    Running,
    /// It has finished, with an outcome.
    Ended,
}

impl RunState {
    text_forms!(impl "run state" {
        RunState::Running => "running",
        RunState::Ended => "ended",
    });
}

/// A launch to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    pub id: String,
    pub actor: String,
    pub command: String,
    pub source_root: Option<PathBuf>,
    pub profiles_dir: Option<PathBuf>,
    pub log_dirs: Vec<PathBuf>,
    pub at_millis: i64,
}

/// A persisted run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub actor: String,
    pub command: String,
    pub source_root: Option<PathBuf>,
    pub profiles_dir: Option<PathBuf>,
    pub log_dirs: Vec<PathBuf>,
    pub pids: Vec<u32>,
    pub started_unix: i64,
    /// Start in milliseconds; a row from a build that did not write it reads back as `started_unix * 1000`.
    pub started_millis: i64,
    pub seen_unix: i64,
    pub state: RunState,
    pub stop_requested: bool,
    pub outcome: Option<String>,
}

const COLUMNS: &str = concat!(
    "id, actor, command, source_root, profiles_dir, log_dirs, pids, ",
    "started_unix, started_millis, seen_unix, state, stop_requested, outcome"
);

/// A running row not touched for this long has lost its launcher.
pub const STALE_AFTER_SECS: i64 = 10;

/// Rows untouched for this long are dropped when a run starts.
pub const RUN_KEEP_SECS: i64 = 7 * 24 * 60 * 60;

impl Store {
    /// Record a launch as running. Refused while a run with that id is live; an ended or
    /// abandoned row with it is replaced.
    pub fn start_run(&self, run: &NewRun) -> Result<(), StoreError> {
        let source_root = run.source_root.as_deref().map(utf8).transpose()?;
        let profiles_dir = run.profiles_dir.as_deref().map(utf8).transpose()?;
        let log_dirs: Vec<&str> = run
            .log_dirs
            .iter()
            .map(|p| utf8(p))
            .collect::<Result<_, _>>()?;
        let now = run.at_millis / 1000;
        self.conn().execute(
            "DELETE FROM run WHERE seen_unix < ?1",
            [now - RUN_KEEP_SECS],
        )?;
        let changed = self.conn().execute(
            "INSERT OR REPLACE INTO run
                 (id, actor, command, source_root, profiles_dir, log_dirs, started_unix, seen_unix, state,
                  started_millis)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9
             WHERE NOT EXISTS (
                 SELECT 1 FROM run WHERE id = ?1 AND state = ?8 AND seen_unix >= ?7 - ?10
             )",
            rusqlite::params![
                run.id,
                run.actor,
                run.command,
                source_root,
                profiles_dir,
                serde_json::to_string(&log_dirs)?,
                now,
                RunState::Running.as_str(),
                run.at_millis,
                STALE_AFTER_SECS,
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::RunLive(run.id.clone()));
        }
        Ok(())
    }

    /// Replace the process ids a running run is using.
    pub fn set_run_pids(&self, id: &str, pids: &[u32]) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE run SET pids = ?1 WHERE id = ?2 AND state = 'running'",
            rusqlite::params![serde_json::to_string(pids)?, id],
        )?;
        Ok(changed > 0)
    }

    /// Keep the result a run produced; only a running run takes one.
    pub fn set_run_result(&self, id: &str, result: &serde_json::Value) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE run SET result = ?1 WHERE id = ?2 AND state = 'running'",
            rusqlite::params![serde_json::to_string(result)?, id],
        )?;
        Ok(changed > 0)
    }

    /// The result an ended run left, when it left one.
    pub fn run_result(&self, id: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let result = self
            .conn()
            .query_row("SELECT result FROM run WHERE id = ?1", [id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        result
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(StoreError::from)
    }

    /// Record that a run's launcher is still alive.
    pub fn touch_run(&self, id: &str, now_unix: i64) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE run SET seen_unix = ?1 WHERE id = ?2 AND state = 'running'",
            rusqlite::params![now_unix, id],
        )?;
        Ok(changed > 0)
    }

    /// Mark a run ended with its outcome.
    pub fn end_run(&self, id: &str, now_unix: i64, outcome: &str) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "UPDATE run SET state = ?1, seen_unix = ?2, outcome = ?3 WHERE id = ?4 AND state = 'running'",
            rusqlite::params![RunState::Ended.as_str(), now_unix, outcome, id],
        )?;
        Ok(changed > 0)
    }

    /// Running runs seen within the last `stale_secs`, newest first.
    pub fn live_runs(&self, now_unix: i64, stale_secs: i64) -> Result<Vec<RunRecord>, StoreError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM run
             WHERE state = ?1 AND seen_unix >= ?2 - ?3
             ORDER BY CASE WHEN started_millis > 0 THEN started_millis ELSE started_unix * 1000 END DESC, id DESC"
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let runs = stmt
            .query_map(
                rusqlite::params![RunState::Running.as_str(), now_unix, stale_secs],
                map_run,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(runs)
    }

    /// Ask a running run to stop, reporting whether a running run with that id exists.
    pub fn request_run_stop(&self, id: &str) -> Result<bool, StoreError> {
        let updated = self.conn().execute(
            "UPDATE run SET stop_requested = 1 WHERE id = ?1 AND state = ?2",
            rusqlite::params![id, RunState::Running.as_str()],
        )?;
        Ok(updated > 0)
    }

    /// Whether a stop has been requested for a run.
    pub fn run_stop_requested(&self, id: &str) -> Result<bool, StoreError> {
        let flag = self
            .conn()
            .query_row(
                "SELECT stop_requested FROM run WHERE id = ?1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        Ok(flag.is_some_and(|flag| flag != 0))
    }

    /// One run by id.
    pub fn run(&self, id: &str) -> Result<Option<RunRecord>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM run WHERE id = ?1");
        let mut stmt = self.conn().prepare(&sql)?;
        let mut rows = stmt.query_map([id], map_run)?;
        rows.next().transpose().map_err(StoreError::from)
    }
}

fn utf8(path: &Path) -> Result<&str, StoreError> {
    path.to_str()
        .ok_or_else(|| StoreError::Invalid(format!("path is not valid UTF-8: {}", path.display())))
}

fn map_run(row: &Row<'_>) -> rusqlite::Result<RunRecord> {
    let log_dirs: String = row.get(5)?;
    let pids: String = row.get(6)?;
    let state: String = row.get(10)?;
    let started_unix: i64 = row.get(7)?;
    let started_millis: i64 = row.get(8)?;
    Ok(RunRecord {
        id: row.get(0)?,
        actor: row.get(1)?,
        command: row.get(2)?,
        source_root: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
        profiles_dir: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
        log_dirs: serde_json::from_str(&log_dirs).map_err(|e| crate::bad_text(5, e))?,
        pids: serde_json::from_str(&pids).map_err(|e| crate::bad_text(6, e))?,
        started_unix,
        started_millis: if started_millis == 0 {
            started_unix * 1000
        } else {
            started_millis
        },
        seen_unix: row.get(9)?,
        state: RunState::parse(&state).map_err(|e| crate::bad_text(10, e))?,
        stop_requested: row.get::<_, i64>(11)? != 0,
        outcome: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_written_without_its_start_in_milliseconds_reads_back_with_one() {
        let store = Store::open_in_memory().unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO run (id, actor, command, started_unix, seen_unix, state)
                 VALUES ('run-1', 'agent:1', 'diag.run', 100, 100, 'running')",
                [],
            )
            .unwrap();

        let run = store.run("run-1").unwrap().expect("the row");
        assert_eq!(run.started_millis, 100_000);
    }

    #[test]
    fn live_runs_order_a_row_without_its_start_in_milliseconds_by_its_seconds() {
        let store = Store::open_in_memory().unwrap();
        store
            .start_run(&NewRun {
                id: "z-early".into(),
                actor: "agent:1".into(),
                command: "diag.run".into(),
                source_root: None,
                profiles_dir: None,
                log_dirs: Vec::new(),
                at_millis: 99_500,
            })
            .unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO run (id, actor, command, started_unix, seen_unix, state)
                 VALUES ('a-old', 'agent:1', 'diag.run', 100, 100, 'running')",
                [],
            )
            .unwrap();

        let ids: Vec<String> = store
            .live_runs(100, 30)
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(ids, ["a-old", "z-early"]);
    }
}
