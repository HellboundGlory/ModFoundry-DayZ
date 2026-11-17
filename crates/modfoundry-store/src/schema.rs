//! Schema migrations.
//!
//! Each migration is one version step, applied in its own transaction and
//! recorded in SQLite's `user_version`. Migrations are append-only: a released
//! version is never edited, only superseded.

/// One versioned step from `version - 1` to `version`.
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "audit_policy_undo_settings",
        sql: V1,
    },
    Migration {
        version: 2,
        name: "confirmation_request",
        sql: V2,
    },
    Migration {
        version: 3,
        name: "undo_log_host_root",
        sql: V3,
    },
    Migration {
        version: 4,
        name: "run_record",
        sql: V4,
    },
    Migration {
        version: 5,
        name: "run_start_millis",
        sql: V5,
    },
    Migration {
        version: 6,
        name: "run_result",
        sql: V6,
    },
];

const V1: &str = r#"
CREATE TABLE settings (
    scope TEXT NOT NULL,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE policy_rule (
    id         INTEGER PRIMARY KEY,
    actor      TEXT NOT NULL,
    command    TEXT NOT NULL,
    effect     TEXT NOT NULL,
    decision   TEXT NOT NULL CHECK (decision IN ('allow', 'deny', 'ask')),
    created_at INTEGER NOT NULL,
    UNIQUE (actor, command, effect)
);

CREATE TABLE audit_entry (
    id         INTEGER PRIMARY KEY,
    at_unix    INTEGER NOT NULL,
    actor      TEXT NOT NULL,
    command    TEXT NOT NULL,
    effect     TEXT NOT NULL,
    args_hash  TEXT NOT NULL,
    outcome    TEXT NOT NULL,
    detail     TEXT,
    change_set INTEGER
);

CREATE INDEX audit_entry_by_actor ON audit_entry (actor, at_unix DESC, id DESC);

CREATE TABLE undo_log (
    id        INTEGER PRIMARY KEY,
    at_unix   INTEGER NOT NULL,
    actor     TEXT NOT NULL,
    command   TEXT NOT NULL,
    changes   TEXT NOT NULL,
    undone    INTEGER NOT NULL DEFAULT 0,
    undone_at INTEGER
);

CREATE INDEX undo_log_undoable ON undo_log (undone, id DESC);
"#;

const V2: &str = r#"
CREATE TABLE confirmation_request (
    id          INTEGER PRIMARY KEY,
    at_unix     INTEGER NOT NULL,
    actor       TEXT NOT NULL,
    command     TEXT NOT NULL,
    effect      TEXT NOT NULL,
    args_hash   TEXT NOT NULL,
    summary     TEXT NOT NULL,
    answer      TEXT CHECK (answer IN ('once', 'session', 'deny')),
    answered_at INTEGER
);

CREATE INDEX confirmation_request_pending ON confirmation_request (answer);
"#;

const V3: &str = r#"
ALTER TABLE undo_log ADD COLUMN host_root TEXT;
UPDATE undo_log SET host_root = json_extract(changes, '$.host_root');
"#;

const V4: &str = r#"
CREATE TABLE run (
    id             TEXT PRIMARY KEY,
    actor          TEXT NOT NULL,
    command        TEXT NOT NULL,
    source_root    TEXT,
    profiles_dir   TEXT,
    log_dirs       TEXT NOT NULL DEFAULT '[]',
    pids           TEXT NOT NULL DEFAULT '[]',
    started_unix   INTEGER NOT NULL,
    seen_unix      INTEGER NOT NULL,
    state          TEXT NOT NULL CHECK (state IN ('running', 'ended')),
    stop_requested INTEGER NOT NULL DEFAULT 0,
    outcome        TEXT
);

CREATE INDEX run_state ON run (state);
"#;

const V5: &str = r#"
ALTER TABLE run ADD COLUMN started_millis INTEGER NOT NULL DEFAULT 0;
UPDATE run SET started_millis = started_unix * 1000;
"#;

const V6: &str = r#"ALTER TABLE run ADD COLUMN result TEXT;"#;
