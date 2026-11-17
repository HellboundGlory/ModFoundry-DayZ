//! Store contract tests.
//!
//! `modfoundry-store` is the durable side of the agent pipeline: the policy rules
//! that authorize agent calls, the audit trail every dispatch appends to, the
//! change-set stack `workspace.undo` pops, and the settings table. These tests pin
//! the observable contract the agent and frontend layers depend on: schema
//! versioning, WAL durability, append-only audit ordering, policy precedence, and
//! change-set lifecycle. Nothing here asserts table names or SQL.

use modfoundry_core::EffectKind;
use modfoundry_store::audit::{AuditOutcome, NewAuditEntry};
use modfoundry_store::confirm::{ConfirmAnswer, NewConfirmation};
use modfoundry_store::policy::Decision;
use modfoundry_store::run::{NewRun, RunState};
use modfoundry_store::schema::MIGRATIONS;
use modfoundry_store::settings::Scope;
use modfoundry_store::{effect_from_str, effect_str, Store, StoreError, SCHEMA_VERSION};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Unique temp dir per call (label + pid + per-call counter) so parallel tests in
/// one binary never share a database file.
fn tmp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mf-store-{label}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("modfoundry.sqlite")
}

fn entry(actor: &str, command: &str, outcome: AuditOutcome) -> NewAuditEntry {
    NewAuditEntry {
        at_unix: 1_700_000_000,
        actor: actor.to_owned(),
        command: command.to_owned(),
        effect: EffectKind::Mutate,
        args_hash: "deadbeef".to_owned(),
        outcome,
        detail: None,
        change_set: None,
    }
}

fn confirmation() -> NewConfirmation {
    NewConfirmation {
        at_unix: 1_700_000_000,
        actor: "agent:1".to_owned(),
        command: "build.run".to_owned(),
        effect: EffectKind::Mutate,
        args_hash: "deadbeef".to_owned(),
        summary: "Run build.run".to_owned(),
    }
}

#[test]
fn open_creates_a_file_database_at_the_current_schema_version() {
    let dir = tmp_dir("open");
    let store = Store::open(db_path(&dir)).expect("open");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(db_path(&dir).is_file());
}

#[test]
fn a_v1_database_migrates_to_the_current_version_and_keeps_its_rows() {
    let dir = tmp_dir("migrate-v1");
    let path = db_path(&dir);

    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch(MIGRATIONS[0].sql).unwrap();
        raw.pragma_update(None, "user_version", 1).unwrap();
        raw.execute(
            "INSERT INTO settings (scope, key, value) VALUES ('global', 'theme', '\"dark\"')",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("a v1 database migrates on open");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        store.get_setting(&Scope::Global, "theme").unwrap(),
        Some(json!("dark")),
        "pre-migration rows survive"
    );
    let id = store.request_confirmation(&confirmation()).unwrap();
    assert_eq!(store.pending_confirmations().unwrap().len(), 1);
    assert!(store
        .answer_confirmation(id, ConfirmAnswer::Once, 30)
        .unwrap());
}

#[test]
fn a_v2_database_migrates_to_the_current_version_backfilling_host_root_from_the_change_set() {
    let dir = tmp_dir("migrate-v2");
    let path = db_path(&dir);

    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch(MIGRATIONS[0].sql).unwrap();
        raw.execute_batch(MIGRATIONS[1].sql).unwrap();
        raw.pragma_update(None, "user_version", 2).unwrap();
        raw.execute(
            "INSERT INTO undo_log (at_unix, actor, command, changes, undone, undone_at)
             VALUES (10, 'agent:1', 'text.edit',
                     '{\"root\":\"/w\",\"host_root\":\"/w/a\",\"changes\":[]}', 0, NULL)",
            [],
        )
        .unwrap();
        raw.execute(
            "INSERT INTO undo_log (at_unix, actor, command, changes, undone, undone_at)
             VALUES (20, 'agent:1', 'text.edit', '{\"root\":\"/w\",\"changes\":[]}', 0, NULL)",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("a v2 database migrates on open");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let newest = store.last_undoable().unwrap().expect("a pre-migration row");
    assert_eq!(newest.id, 2);
    assert_eq!(newest.host_root, None);
    assert!(store.mark_undone(2, 30).unwrap());
    let backfilled = store.last_undoable().unwrap().expect("the row below it");
    assert_eq!(backfilled.id, 1);
    assert_eq!(backfilled.host_root.as_deref(), Some(Path::new("/w/a")));
}

#[test]
fn file_databases_run_in_wal_mode() {
    let dir = tmp_dir("wal");
    let store = Store::open(db_path(&dir)).expect("open");

    assert_eq!(store.journal_mode().unwrap(), "wal");
}

#[test]
fn a_database_written_by_a_newer_build_is_refused_rather_than_used() {
    let dir = tmp_dir("future");
    let path = db_path(&dir);

    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
    }

    match Store::open(&path) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, SCHEMA_VERSION + 1);
            assert_eq!(supported, SCHEMA_VERSION);
        }
        Err(other) => panic!("expected SchemaTooNew, got {other}"),
        Ok(_) => panic!("opened a database written by a newer build"),
    }
}

#[test]
fn reopening_keeps_the_schema_version_and_the_rows() {
    let dir = tmp_dir("reopen");
    let path = db_path(&dir);

    {
        let store = Store::open(&path).expect("open");
        store
            .set_setting(&Scope::Global, "theme", &json!("dark"))
            .unwrap();
    }

    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(reopened.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        reopened.get_setting(&Scope::Global, "theme").unwrap(),
        Some(json!("dark"))
    );
}

#[test]
fn in_memory_stores_support_the_full_api() {
    let store = Store::open_in_memory().expect("open in memory");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    store
        .set_policy_rule("*", "text.edit", None, Decision::Ask, 5)
        .unwrap();
    assert_eq!(
        store
            .decision_for("agent:1", "text.edit", EffectKind::Mutate)
            .unwrap(),
        Some(Decision::Ask)
    );
}

#[test]
fn checkpoint_succeeds_and_truncates_the_write_ahead_log() {
    let dir = tmp_dir("checkpoint");
    let path = db_path(&dir);
    let store = Store::open(&path).expect("open");

    store
        .record_audit(&entry("agent:1", "build.run", AuditOutcome::Ok))
        .unwrap();
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert!(
        wal.metadata().unwrap().len() > 0,
        "a write must reach the WAL"
    );

    store.checkpoint().expect("checkpoint");

    assert_eq!(wal.metadata().unwrap().len(), 0, "checkpoint must truncate");
    assert_eq!(store.audit_entries(10).unwrap().len(), 1, "rows survive");
}

#[test]
fn audit_entries_are_listed_newest_first_and_carry_every_field() {
    let store = Store::open_in_memory().unwrap();

    let mut first = entry("agent:7", "config.binarize", AuditOutcome::Ok);
    first.at_unix = 100;
    first.detail = Some("wrote 2048 bytes".to_owned());
    first.change_set = Some(42);
    store.record_audit(&first).unwrap();

    let mut denied = entry("agent:7", "publish.prepare", AuditOutcome::Denied);
    denied.at_unix = 200;
    denied.effect = EffectKind::Publish;
    store.record_audit(&denied).unwrap();

    let rows = store.audit_entries(10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].command, "publish.prepare", "newest first");
    assert_eq!(rows[0].effect, EffectKind::Publish);
    assert_eq!(rows[0].outcome, AuditOutcome::Denied);
    assert_eq!(rows[1].at_unix, 100);
    assert_eq!(rows[1].detail.as_deref(), Some("wrote 2048 bytes"));
    assert_eq!(rows[1].change_set, Some(42));
    assert_eq!(rows[1].actor, "agent:7");
    assert!(rows[1].id < rows[0].id, "ids follow insertion order");
}

#[test]
fn audit_limit_applies_to_the_newest_entries() {
    let store = Store::open_in_memory().unwrap();
    for n in 0..5 {
        let mut e = entry("agent:1", &format!("command.{n}"), AuditOutcome::Ok);
        e.at_unix = 100 + n;
        store.record_audit(&e).unwrap();
    }

    let rows = store.audit_entries(2).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].command, "command.4");
    assert_eq!(rows[1].command, "command.3");
}

#[test]
fn audit_can_be_filtered_to_one_actor() {
    let store = Store::open_in_memory().unwrap();
    store
        .record_audit(&entry("agent:1", "build.run", AuditOutcome::Ok))
        .unwrap();
    store
        .record_audit(&entry("agent:2", "vfs.read", AuditOutcome::Ok))
        .unwrap();
    store
        .record_audit(&entry("agent:1", "vfs.read", AuditOutcome::Error))
        .unwrap();

    let rows = store.audit_entries_for("agent:1", 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.actor == "agent:1"));
    assert_eq!(store.audit_entries_for("nobody", 10).unwrap().len(), 0);
}

#[test]
fn policy_rule_upsert_replaces_the_decision_for_the_same_selector() {
    let store = Store::open_in_memory().unwrap();

    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Ask, 10)
        .unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 20)
        .unwrap();

    assert_eq!(store.policy_rules().unwrap().len(), 1, "upsert, not append");
    assert_eq!(
        store
            .decision_for("agent:1", "build.run", EffectKind::Mutate)
            .unwrap(),
        Some(Decision::Allow)
    );
}

#[test]
fn policy_without_an_effect_selector_covers_every_effect_class() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 10)
        .unwrap();

    for effect in [
        EffectKind::Read,
        EffectKind::IndexWrite,
        EffectKind::Mutate,
        EffectKind::Destructive,
        EffectKind::Launch,
        EffectKind::Network,
        EffectKind::Publish,
    ] {
        assert_eq!(
            store.decision_for("agent:1", "build.run", effect).unwrap(),
            Some(Decision::Allow),
            "wildcard effect must cover {effect:?}"
        );
    }
}

#[test]
fn policy_precedence_prefers_the_most_specific_matching_rule() {
    let store = Store::open_in_memory().unwrap();
    // Coarse to specific, deliberately inserted out of precedence order.
    store
        .set_policy_rule("*", "*", None, Decision::Deny, 1)
        .unwrap();
    store
        .set_policy_rule("agent:1", "*", None, Decision::Ask, 2)
        .unwrap();
    store
        .set_policy_rule("agent:1", "text.edit", None, Decision::Allow, 3)
        .unwrap();
    store
        .set_policy_rule(
            "agent:1",
            "text.edit",
            Some(EffectKind::Publish),
            Decision::Deny,
            4,
        )
        .unwrap();

    assert_eq!(
        store
            .decision_for("agent:2", "anything", EffectKind::Read)
            .unwrap(),
        Some(Decision::Deny),
        "actor wildcard is the fallback"
    );
    assert_eq!(
        store
            .decision_for("agent:1", "build.run", EffectKind::Mutate)
            .unwrap(),
        Some(Decision::Ask),
        "actor-exact beats actor-wildcard"
    );
    assert_eq!(
        store
            .decision_for("agent:1", "text.edit", EffectKind::Mutate)
            .unwrap(),
        Some(Decision::Allow),
        "command-exact beats command-wildcard"
    );
    assert_eq!(
        store
            .decision_for("agent:1", "text.edit", EffectKind::Publish)
            .unwrap(),
        Some(Decision::Deny),
        "effect-exact beats effect-wildcard at the same actor+command"
    );
}

#[test]
fn policy_lookup_for_an_unmatched_selector_is_none() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 10)
        .unwrap();

    assert_eq!(
        store
            .decision_for("agent:9", "build.run", EffectKind::Mutate)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .decision_for("agent:1", "vfs.read", EffectKind::Read)
            .unwrap(),
        None
    );
}

#[test]
fn policy_remove_reports_whether_it_matched() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 10)
        .unwrap();

    assert!(store
        .remove_policy_rule("agent:1", "build.run", None)
        .unwrap());
    assert!(!store
        .remove_policy_rule("agent:1", "build.run", None)
        .unwrap());
    assert_eq!(store.policy_rules().unwrap().len(), 0);
}

#[test]
fn removing_a_wildcard_effect_rule_leaves_effect_specific_rules_alone() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 10)
        .unwrap();
    store
        .set_policy_rule(
            "agent:1",
            "build.run",
            Some(EffectKind::Publish),
            Decision::Deny,
            11,
        )
        .unwrap();

    assert!(store
        .remove_policy_rule("agent:1", "build.run", None)
        .unwrap());

    let remaining = store.policy_rules().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].effect, Some(EffectKind::Publish));
}

#[test]
fn a_rewritten_rule_wins_a_tie() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:1", "*", Some(EffectKind::Mutate), Decision::Deny, 1)
        .unwrap();
    store
        .set_policy_rule(
            "*",
            "build.run",
            Some(EffectKind::Mutate),
            Decision::Allow,
            2,
        )
        .unwrap();
    store
        .set_policy_rule("agent:1", "*", Some(EffectKind::Mutate), Decision::Deny, 3)
        .unwrap();

    assert_eq!(
        store
            .decision_for("agent:1", "build.run", EffectKind::Mutate)
            .unwrap(),
        Some(Decision::Deny),
        "the rule written last wins between two equally specific matches"
    );
}

#[test]
fn settings_round_trip_json_values() {
    let store = Store::open_in_memory().unwrap();
    let value = json!({ "layers": ["game", "workshop"], "depth": 3 });

    store
        .set_setting(&Scope::Workspace("ws-1".into()), "vfs.mounts", &value)
        .unwrap();

    assert_eq!(
        store
            .get_setting(&Scope::Workspace("ws-1".into()), "vfs.mounts")
            .unwrap(),
        Some(value)
    );
}

#[test]
fn settings_scopes_are_isolated() {
    let store = Store::open_in_memory().unwrap();
    let ws = Scope::Workspace("ws-1".into());

    store
        .set_setting(&Scope::Global, "theme", &json!("dark"))
        .unwrap();
    store.set_setting(&ws, "theme", &json!("light")).unwrap();

    assert_eq!(
        store.get_setting(&Scope::Global, "theme").unwrap(),
        Some(json!("dark"))
    );
    assert_eq!(
        store.get_setting(&ws, "theme").unwrap(),
        Some(json!("light"))
    );
    assert_eq!(
        store
            .get_setting(&Scope::Workspace("ws-2".into()), "theme")
            .unwrap(),
        None
    );
}

#[test]
fn set_setting_overwrites_the_previous_value() {
    let store = Store::open_in_memory().unwrap();
    store.set_setting(&Scope::Global, "k", &json!(1)).unwrap();
    store.set_setting(&Scope::Global, "k", &json!(2)).unwrap();

    assert_eq!(
        store.get_setting(&Scope::Global, "k").unwrap(),
        Some(json!(2))
    );
}

#[test]
fn undo_stack_returns_the_most_recent_undoable_change_set() {
    let store = Store::open_in_memory().unwrap();
    let first = json!([{ "op": "create", "path": "a.c" }]);
    let second = json!([{ "op": "overwrite", "path": "b.c" }]);

    store
        .record_change_set("agent:1", "text.edit", &first, 10)
        .unwrap();
    let second_id = store
        .record_change_set("agent:1", "text.edit", &second, 20)
        .unwrap();

    let top = store.last_undoable().unwrap().expect("a change set");
    assert_eq!(top.id, second_id);
    assert_eq!(top.changes, second);
    assert_eq!(top.command, "text.edit");
    assert_eq!(top.actor, "agent:1");
    assert!(!top.undone);
    assert_eq!(top.undone_at, None);
}

#[test]
fn marking_done_removes_a_change_set_from_the_undo_stack_exactly_once() {
    let store = Store::open_in_memory().unwrap();
    let older = store
        .record_change_set("agent:1", "text.edit", &json!([]), 10)
        .unwrap();
    let newer = store
        .record_change_set("agent:1", "text.edit", &json!([]), 20)
        .unwrap();

    assert!(store.mark_undone(newer, 30).unwrap());
    assert!(
        !store.mark_undone(newer, 31).unwrap(),
        "double-undo is refused"
    );

    let top = store.last_undoable().unwrap().expect("older set remains");
    assert_eq!(top.id, older);
}

#[test]
fn the_undo_stack_is_empty_when_nothing_was_recorded() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.last_undoable().unwrap(), None);
}

#[test]
fn record_change_set_stores_the_host_root_it_was_given() {
    let store = Store::open_in_memory().unwrap();
    let payload = json!({ "root": "/w", "host_root": "/w/a", "changes": [] });

    let id = store
        .record_change_set("agent:1", "text.edit", &payload, 10)
        .unwrap();

    let record = store.last_undoable().unwrap().expect("a change set");
    assert_eq!(record.id, id);
    assert_eq!(record.host_root.as_deref(), Some(Path::new("/w/a")));
}

#[test]
fn a_change_set_without_a_host_root_records_none() {
    let store = Store::open_in_memory().unwrap();
    store
        .record_change_set("agent:1", "text.edit", &json!([{ "op": "create" }]), 10)
        .unwrap();

    let record = store.last_undoable().unwrap().expect("a change set");
    assert_eq!(record.host_root, None);
}

#[test]
fn last_undoable_within_selects_the_newest_change_set_in_that_tree() {
    let store = Store::open_in_memory().unwrap();
    let set = |root: &str| json!({ "host_root": root, "changes": [] });
    let a = store
        .record_change_set("agent:1", "text.edit", &set("/w/a"), 10)
        .unwrap();
    let a_sub = store
        .record_change_set("agent:1", "text.edit", &set("/w/a/sub"), 20)
        .unwrap();
    store
        .record_change_set("agent:1", "text.edit", &set("/w/ab"), 30)
        .unwrap();
    let b = store
        .record_change_set("agent:1", "text.edit", &set("/w/b"), 40)
        .unwrap();

    assert_eq!(
        store
            .last_undoable_within(Path::new("/w/a"))
            .unwrap()
            .expect("a nested change set")
            .id,
        a_sub,
        "a child of the root matches; the newer sibling /w/ab does not"
    );
    assert_eq!(
        store
            .last_undoable_within(Path::new("/w"))
            .unwrap()
            .unwrap()
            .id,
        b,
        "the root contains every tree"
    );

    assert!(store.mark_undone(a_sub, 50).unwrap());
    assert_eq!(
        store
            .last_undoable_within(Path::new("/w/a"))
            .unwrap()
            .unwrap()
            .id,
        a
    );
    assert!(store.mark_undone(a, 51).unwrap());
    assert_eq!(store.last_undoable_within(Path::new("/w/a")).unwrap(), None);
}

#[test]
fn last_undoable_within_ignores_relative_and_missing_host_roots() {
    let store = Store::open_in_memory().unwrap();
    store
        .record_change_set(
            "agent:1",
            "text.edit",
            &json!({ "host_root": ".", "changes": [] }),
            10,
        )
        .unwrap();
    store
        .record_change_set("agent:1", "text.edit", &json!([{ "op": "create" }]), 20)
        .unwrap();

    assert_eq!(store.last_undoable_within(Path::new(".")).unwrap(), None);
    assert_eq!(store.last_undoable_within(Path::new("/w")).unwrap(), None);
    assert!(
        store.last_undoable().unwrap().is_some(),
        "last_undoable still sees them"
    );
}

#[test]
fn undo_within_ignores_a_root_that_climbs_out() {
    let store = Store::open_in_memory().unwrap();
    store
        .record_change_set(
            "agent:1",
            "text.edit",
            &json!({ "host_root": "/w/mod/../../home/u", "changes": [] }),
            10,
        )
        .unwrap();

    assert_eq!(
        store.last_undoable_within(Path::new("/w/mod")).unwrap(),
        None
    );

    let sub = store
        .record_change_set(
            "agent:1",
            "text.edit",
            &json!({ "host_root": "/w/mod/sub", "changes": [] }),
            20,
        )
        .unwrap();
    assert_eq!(
        store
            .last_undoable_within(Path::new("/w/mod"))
            .unwrap()
            .expect("a change set under the root")
            .id,
        sub
    );
}

#[cfg(unix)]
#[test]
fn undo_within_does_not_follow_a_link_out_of_the_root() {
    use std::os::unix::fs::symlink;

    let store = Store::open_in_memory().unwrap();
    let base = tmp_dir("undo-escape");
    let work = base.join("work");
    let outside = base.join("outside");
    std::fs::create_dir_all(work.join("proj")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, work.join("escape")).unwrap();
    symlink(outside.join("not-there"), work.join("dangling")).unwrap();

    let set = |root: PathBuf| json!({ "host_root": root, "changes": [] });
    let inside = store
        .record_change_set("agent:1", "text.edit", &set(work.join("proj")), 10)
        .unwrap();
    store
        .record_change_set("agent:1", "text.edit", &set(work.join("escape")), 20)
        .unwrap();
    store
        .record_change_set(
            "agent:1",
            "text.edit",
            &set(work.join("escape").join("new-folder")),
            30,
        )
        .unwrap();
    store
        .record_change_set("agent:1", "text.edit", &set(work.join("dangling")), 40)
        .unwrap();

    assert_eq!(
        store
            .last_undoable_within(&work)
            .unwrap()
            .expect("the change set under proj")
            .id,
        inside,
        "a link that leaves the root, or a dangling one, is not inside it"
    );
    assert!(store.mark_undone(inside, 50).unwrap());
    assert_eq!(store.last_undoable_within(&work).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn undo_within_finds_a_change_set_through_a_link_to_the_root() {
    use std::os::unix::fs::symlink;

    let store = Store::open_in_memory().unwrap();
    let base = tmp_dir("undo-link-root");
    let real = base.join("real");
    let link = base.join("link");
    std::fs::create_dir_all(real.join("proj")).unwrap();
    symlink(&real, &link).unwrap();

    let set = |root: PathBuf| json!({ "host_root": root, "changes": [] });
    let recorded = store
        .record_change_set("agent:1", "text.edit", &set(real.join("proj")), 10)
        .unwrap();
    for root in [link.join("proj"), link.clone(), real.clone()] {
        assert_eq!(
            store
                .last_undoable_within(&root)
                .unwrap()
                .unwrap_or_else(|| panic!("{} finds it", root.display()))
                .id,
            recorded,
            "{} names the folder the change was recorded in",
            root.display()
        );
    }

    let through_link = store
        .record_change_set("agent:1", "text.edit", &set(link.join("proj")), 20)
        .unwrap();
    assert_eq!(
        store
            .last_undoable_within(&real)
            .unwrap()
            .expect("the change recorded through the link")
            .id,
        through_link
    );
}

#[cfg(unix)]
#[test]
fn undo_within_compares_a_missing_tail_under_the_folder_it_will_be_in() {
    use std::os::unix::fs::symlink;

    let store = Store::open_in_memory().unwrap();
    let base = tmp_dir("undo-missing-tail");
    let real = base.join("real");
    let link = base.join("link");
    std::fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();

    let recorded = store
        .record_change_set(
            "agent:1",
            "text.edit",
            &json!({ "host_root": link.join("not-yet/sub"), "changes": [] }),
            10,
        )
        .unwrap();

    assert_eq!(
        store
            .last_undoable_within(&real)
            .unwrap()
            .expect("the part that exists places the rest under the root")
            .id,
        recorded
    );
    assert_eq!(
        store.last_undoable_within(&real.join("other")).unwrap(),
        None
    );
}

// SQLite is used specifically because GUI, agent and CLI share one
// database file. That is a contract about *concurrent* access, so it is tested
// against a real file with more than one connection.

#[test]
fn two_stores_on_one_file_interleave_writes_and_see_each_others_rows() {
    let dir = tmp_dir("concurrent");
    let path = db_path(&dir);
    let a = Store::open(&path).expect("open a");
    let b = Store::open(&path).expect("open b");

    a.record_audit(&entry("agent:1", "build.run", AuditOutcome::Ok))
        .unwrap();
    b.set_setting(&Scope::Global, "from-b", &json!(1)).unwrap();
    b.record_audit(&entry("agent:2", "vfs.read", AuditOutcome::Ok))
        .unwrap();
    a.set_setting(&Scope::Global, "from-a", &json!(2)).unwrap();

    assert_eq!(a.audit_entries(10).unwrap().len(), 2, "a sees b's row");
    assert_eq!(b.audit_entries(10).unwrap().len(), 2, "b sees a's row");
    assert_eq!(
        b.get_setting(&Scope::Global, "from-a").unwrap(),
        Some(json!(2))
    );
    assert_eq!(
        a.get_setting(&Scope::Global, "from-b").unwrap(),
        Some(json!(1))
    );
}

#[test]
fn a_write_waits_for_a_competing_writer_instead_of_failing() {
    let dir = tmp_dir("contention");
    let path = db_path(&dir);
    let store = Store::open(&path).expect("open");

    // Hold the write lock on a second connection. With no busy timeout the
    // store's insert below would fail immediately with SQLITE_BUSY instead of
    // waiting for this transaction to commit.
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    blocker
        .execute(
            "INSERT INTO settings (scope, key, value) VALUES ('global', 'blocker', '1')",
            [],
        )
        .unwrap();

    let writer = std::thread::spawn(move || {
        store.record_audit(&entry("agent:1", "build.run", AuditOutcome::Ok))
    });
    std::thread::sleep(std::time::Duration::from_millis(250));
    blocker.execute_batch("COMMIT").unwrap();

    writer
        .join()
        .expect("writer thread must not panic")
        .expect("the write must wait for the lock rather than fail with SQLITE_BUSY");
}

// The answer channel between the MCP server process and the GUI process: one
// process raises an `ask` request, a frontend in another process answers it.

#[test]
fn a_requested_confirmation_is_pending_until_it_is_answered() {
    let store = Store::open_in_memory().unwrap();
    let id = store.request_confirmation(&confirmation()).unwrap();

    let pending = store.pending_confirmations().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].actor, "agent:1");
    assert_eq!(pending[0].effect, EffectKind::Mutate);
    assert_eq!(pending[0].answer, None);
    assert_eq!(pending[0].answered_at, None);

    assert!(store
        .answer_confirmation(id, ConfirmAnswer::Once, 30)
        .unwrap());
    assert_eq!(store.pending_confirmations().unwrap().len(), 0);

    let answered = store.confirmation(id).unwrap().expect("the row");
    assert_eq!(answered.answer, Some(ConfirmAnswer::Once));
    assert_eq!(answered.answered_at, Some(30));
}

#[test]
fn a_second_answer_is_refused_and_leaves_the_stored_answer_alone() {
    let store = Store::open_in_memory().unwrap();
    let id = store.request_confirmation(&confirmation()).unwrap();

    assert!(store
        .answer_confirmation(id, ConfirmAnswer::Session, 30)
        .unwrap());
    assert!(
        !store
            .answer_confirmation(id, ConfirmAnswer::Deny, 40)
            .unwrap(),
        "the first writer wins"
    );

    let stored = store.confirmation(id).unwrap().expect("the row");
    assert_eq!(stored.answer, Some(ConfirmAnswer::Session));
    assert_eq!(stored.answered_at, Some(30));
}

#[test]
fn two_stores_on_one_file_raise_and_answer_the_same_request() {
    let dir = tmp_dir("confirm-concurrent");
    let path = db_path(&dir);
    let server = Store::open(&path).expect("open server");
    let gui = Store::open(&path).expect("open gui");

    let id = server.request_confirmation(&confirmation()).unwrap();

    let pending = gui.pending_confirmations().unwrap();
    assert_eq!(pending.len(), 1, "the gui sees the server's request");
    assert_eq!(pending[0].id, id);

    assert!(gui
        .answer_confirmation(pending[0].id, ConfirmAnswer::Session, 30)
        .unwrap());
    assert_eq!(
        server.confirmation(id).unwrap().unwrap().answer,
        Some(ConfirmAnswer::Session),
        "the server reads the gui's answer"
    );
}

#[test]
fn every_persisted_word_round_trips_and_nonsense_is_refused() {
    for effect in [
        EffectKind::Read,
        EffectKind::IndexWrite,
        EffectKind::Mutate,
        EffectKind::Destructive,
        EffectKind::Launch,
        EffectKind::Network,
        EffectKind::Publish,
    ] {
        assert_eq!(effect_from_str(effect_str(effect)).unwrap(), effect);
    }

    assert!(matches!(
        effect_from_str("nonsense"),
        Err(StoreError::Invalid(_))
    ));

    for outcome in [
        AuditOutcome::Ok,
        AuditOutcome::Denied,
        AuditOutcome::Error,
        AuditOutcome::Preview,
    ] {
        assert_eq!(AuditOutcome::parse(outcome.as_str()).unwrap(), outcome);
    }

    assert!(matches!(
        AuditOutcome::parse("nonsense"),
        Err(StoreError::Invalid(_))
    ));

    for answer in [
        ConfirmAnswer::Once,
        ConfirmAnswer::Session,
        ConfirmAnswer::Deny,
    ] {
        assert_eq!(ConfirmAnswer::parse(answer.as_str()).unwrap(), answer);
    }

    assert!(matches!(
        ConfirmAnswer::parse("nonsense"),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn concurrent_opens_of_a_fresh_database_all_migrate_cleanly() {
    for round in 0..20 {
        let db = tmp_dir(&format!("race-{round}")).join("fresh.sqlite");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                std::thread::spawn(move || Store::open(&db).map(|s| s.schema_version().unwrap()))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().expect("open"), SCHEMA_VERSION);
        }
    }
}

// The launch record every process shares: a launcher writes it, the GUI and the
// agent server read it and ask it to stop.

fn new_run(id: &str, at_millis: i64) -> NewRun {
    NewRun {
        id: id.to_owned(),
        actor: "agent:1".to_owned(),
        command: "launch.start".to_owned(),
        source_root: Some(PathBuf::from("/w/mod")),
        profiles_dir: None,
        log_dirs: Vec::new(),
        at_millis,
    }
}

#[test]
fn a_started_run_keeps_its_start_in_milliseconds_and_in_whole_seconds() {
    let store = Store::open_in_memory().unwrap();
    store
        .start_run(&new_run("run-1", 1_700_000_123_456))
        .unwrap();

    let run = store.run("run-1").unwrap().expect("the row");
    assert_eq!(run.started_millis, 1_700_000_123_456);
    assert_eq!(
        (run.started_unix, run.seen_unix),
        (1_700_000_123, 1_700_000_123)
    );
}

#[test]
fn a_v4_database_backfills_a_runs_start_in_milliseconds() {
    let dir = tmp_dir("migrate-v4");
    let path = db_path(&dir);

    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..4] {
            raw.execute_batch(migration.sql).unwrap();
        }
        raw.pragma_update(None, "user_version", 4).unwrap();
        raw.execute(
            "INSERT INTO run (id, actor, command, started_unix, seen_unix, state)
             VALUES ('run-1', 'agent:1', 'diag.run', 100, 100, 'running')",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("a v4 database migrates on open");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let run = store.run("run-1").unwrap().expect("the pre-migration row");
    assert_eq!(run.started_millis, 100_000);
    assert_eq!(run.started_unix, 100);
}

#[test]
fn a_started_run_is_live_until_it_ends() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("run-1", 100_000)).unwrap();

    let live = store.live_runs(100, 30).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, "run-1");
    assert_eq!(live[0].actor, "agent:1");
    assert_eq!(live[0].command, "launch.start");
    assert_eq!(live[0].source_root, Some(PathBuf::from("/w/mod")));
    assert_eq!(live[0].state, RunState::Running);
    assert!(!live[0].stop_requested);

    assert!(store.end_run("run-1", 110, "exited 0").unwrap());
    assert!(store.live_runs(110, 30).unwrap().is_empty());
    let ended = store.run("run-1").unwrap().expect("the row");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.outcome.as_deref(), Some("exited 0"));

    assert!(
        !store.end_run("run-1", 120, "killed").unwrap(),
        "an ended run stays ended"
    );
    assert!(!store.touch_run("run-1", 120).unwrap());
    let after = store.run("run-1").unwrap().expect("the row");
    assert_eq!(
        (after.outcome.as_deref(), after.seen_unix),
        (Some("exited 0"), 110)
    );
}

#[test]
fn a_run_not_seen_for_a_while_is_not_live() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("run-1", 100_000)).unwrap();

    assert!(store.live_runs(200, 30).unwrap().is_empty());

    store.touch_run("run-1", 190).unwrap();
    assert_eq!(store.live_runs(200, 30).unwrap().len(), 1);
}

#[test]
fn live_runs_started_in_the_same_second_come_back_newest_first() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("a", 100_900)).unwrap();
    store.start_run(&new_run("b", 100_200)).unwrap();

    let live = store.live_runs(100, 30).unwrap();
    let ids: Vec<&str> = live.iter().map(|run| run.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn a_stop_request_reaches_another_connection() {
    let dir = tmp_dir("run-stop");
    let path = db_path(&dir);
    let launcher = Store::open(&path).expect("open launcher");
    let gui = Store::open(&path).expect("open gui");
    launcher.start_run(&new_run("run-1", 100_000)).unwrap();

    assert!(gui.request_run_stop("run-1").unwrap());
    assert!(launcher.run_stop_requested("run-1").unwrap());
    assert!(!gui.request_run_stop("no-such-run").unwrap());
}

#[test]
fn a_run_keeps_its_pids_and_log_dirs() {
    let store = Store::open_in_memory().unwrap();
    let mut run = new_run("run-1", 100_000);
    run.log_dirs = vec![PathBuf::from("/w/logs"), PathBuf::from("/w/more")];
    store.start_run(&run).unwrap();

    store.set_run_pids("run-1", &[4242, 4243]).unwrap();

    let run = store.run("run-1").unwrap().expect("the row");
    assert_eq!(run.pids, vec![4242, 4243]);
    assert_eq!(
        run.log_dirs,
        vec![PathBuf::from("/w/logs"), PathBuf::from("/w/more")]
    );
}

#[test]
fn a_second_start_is_refused_only_while_the_first_run_is_live() {
    for (second, refused) in [(105_000, true), (110_000, true), (111_000, false)] {
        let store = Store::open_in_memory().unwrap();
        store.start_run(&new_run("run-1", 100_000)).unwrap();
        let mut again = new_run("run-1", second);
        again.actor = "other".to_owned();

        let started = store.start_run(&again);
        let row = store.run("run-1").unwrap().expect("the row");
        if refused {
            assert!(
                matches!(&started, Err(StoreError::RunLive(id)) if id == "run-1"),
                "{second}: {started:?}"
            );
            assert_eq!(
                (row.started_millis, row.actor.as_str()),
                (100_000, "agent:1"),
                "{second}: a refused start leaves the row alone"
            );
        } else {
            started.unwrap();
            assert_eq!(
                (row.started_millis, row.actor.as_str()),
                (second, "other"),
                "{second}: an abandoned run is replaced"
            );
        }
    }
}

#[test]
fn an_ended_run_id_starts_again_as_a_fresh_row() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("run-1", 100_000)).unwrap();
    assert!(store.set_run_pids("run-1", &[1]).unwrap());
    assert!(store.request_run_stop("run-1").unwrap());
    assert!(store.end_run("run-1", 110, "exited").unwrap());

    store.start_run(&new_run("run-1", 200_000)).unwrap();

    let run = store.run("run-1").unwrap().expect("the row");
    assert_eq!(run.state, RunState::Running);
    assert_eq!(run.outcome, None);
    assert_eq!(run.pids, Vec::<u32>::new());
    assert!(!run.stop_requested);
    assert_eq!(run.started_millis, 200_000);
}

#[test]
fn process_ids_are_only_recorded_for_a_running_run() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("run-1", 100_000)).unwrap();

    assert!(store.set_run_pids("run-1", &[7]).unwrap());
    assert_eq!(store.run("run-1").unwrap().unwrap().pids, vec![7]);

    store.end_run("run-1", 110, "exited").unwrap();
    assert!(!store.set_run_pids("run-1", &[8, 9]).unwrap());
    assert_eq!(
        store.run("run-1").unwrap().unwrap().pids,
        vec![7],
        "an ended run keeps the ids it ended with"
    );

    assert!(!store.set_run_pids("no-such-run", &[8]).unwrap());
}

#[test]
fn old_rows_are_dropped_when_a_run_starts() {
    use modfoundry_store::run::RUN_KEEP_SECS;

    let dir = tmp_dir("run-prune");
    let path = db_path(&dir);
    let now = 10_000_000;
    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        for migration in MIGRATIONS {
            raw.execute_batch(migration.sql).unwrap();
        }
        raw.pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();
        let seen = [
            ("ended-old", "ended", now - RUN_KEEP_SECS - 1),
            ("ended-edge", "ended", now - RUN_KEEP_SECS),
            ("lost-old", "running", now - RUN_KEEP_SECS - 1),
            ("fresh", "running", now),
        ];
        for (id, state, seen_unix) in seen {
            raw.execute(
                "INSERT INTO run (id, actor, command, started_unix, seen_unix, state, outcome)
                 VALUES (?1, 'agent:1', 'diag.run', ?2, ?2, ?3, 'exited')",
                rusqlite::params![id, seen_unix, state],
            )
            .unwrap();
        }
    }

    let store = Store::open(&path).expect("open");
    store.start_run(&new_run("trigger", now * 1000)).unwrap();

    for gone in ["ended-old", "lost-old"] {
        assert!(
            store.run(gone).unwrap().is_none(),
            "{gone} outlived its keep"
        );
    }
    for kept in ["ended-edge", "fresh", "trigger"] {
        assert!(
            store.run(kept).unwrap().is_some(),
            "{kept} was dropped early"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_v5_database_gains_an_empty_run_result_column() {
    let dir = tmp_dir("migrate-v5");
    let path = db_path(&dir);

    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..5] {
            raw.execute_batch(migration.sql).unwrap();
        }
        raw.pragma_update(None, "user_version", 5).unwrap();
        raw.execute(
            "INSERT INTO run (id, actor, command, started_unix, seen_unix, state, outcome)
             VALUES ('run-1', 'agent:1', 'diag.run', 100, 110, 'ended', 'exited')",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&path).expect("a v5 database migrates on open");

    assert_eq!(SCHEMA_VERSION, 6);
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.run_result("run-1").unwrap(), None);
    let run = store.run("run-1").unwrap().expect("the pre-migration row");
    assert_eq!(run.outcome.as_deref(), Some("exited"));
}

#[test]
fn an_ended_runs_result_reads_back_and_a_restart_clears_it() {
    let store = Store::open_in_memory().unwrap();
    store.start_run(&new_run("run-1", 100_000)).unwrap();

    assert!(store
        .set_run_result("run-1", &json!({"errors": []}))
        .unwrap());
    assert!(store
        .set_run_result("run-1", &json!({"errors": [], "hint": "look"}))
        .unwrap());
    assert!(store.end_run("run-1", 110, "exited").unwrap());
    assert_eq!(
        store.run_result("run-1").unwrap(),
        Some(json!({"errors": [], "hint": "look"})),
        "the last result a running run left"
    );

    assert!(!store
        .set_run_result("run-1", &json!({"errors": ["late"]}))
        .unwrap());
    assert_eq!(
        store.run_result("run-1").unwrap(),
        Some(json!({"errors": [], "hint": "look"})),
        "an ended run keeps the result it ended with"
    );
    assert!(!store.set_run_result("no-such-run", &json!({})).unwrap());
    assert_eq!(store.run_result("no-such-run").unwrap(), None);

    store.start_run(&new_run("run-1", 200_000)).unwrap();
    assert_eq!(store.run_result("run-1").unwrap(), None);
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_is_refused_not_rewritten() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let bad = || PathBuf::from(OsStr::from_bytes(b"/w/\xff"));
    for place in ["source_root", "profiles_dir", "logs"] {
        let store = Store::open_in_memory().unwrap();
        let mut run = new_run("run-1", 100_000);
        match place {
            "source_root" => run.source_root = Some(bad()),
            "profiles_dir" => run.profiles_dir = Some(bad()),
            _ => run.log_dirs = vec![PathBuf::from("/w/logs"), bad()],
        }

        let started = store.start_run(&run);
        let message = match started {
            Err(StoreError::Invalid(message)) => message,
            other => panic!("{place}: {other:?}"),
        };
        assert!(message.contains("/w/"), "{place}: {message}");
        assert!(store.run("run-1").unwrap().is_none(), "{place} left a row");
    }

    let store = Store::open_in_memory().unwrap();
    let mut run = new_run("run-1", 100_000);
    run.source_root = Some(PathBuf::from("/w/Größe"));
    store.start_run(&run).unwrap();
    assert_eq!(
        store.run("run-1").unwrap().unwrap().source_root,
        Some(PathBuf::from("/w/Größe"))
    );
}

#[test]
fn a_named_rule_applies_to_its_command_whatever_the_effect() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("*", "build.run", None, Decision::Deny, 9)
        .unwrap();
    store
        .set_policy_rule("agent:1", "build.run", None, Decision::Allow, 1)
        .unwrap();
    store
        .set_policy_rule(
            "agent:3",
            "build.run",
            Some(EffectKind::Read),
            Decision::Deny,
            1,
        )
        .unwrap();
    store
        .set_policy_rule(
            "agent:3",
            "build.run",
            Some(EffectKind::Publish),
            Decision::Allow,
            9,
        )
        .unwrap();
    store
        .set_policy_rule(
            "agent:4",
            "build.run",
            Some(EffectKind::Publish),
            Decision::Deny,
            1,
        )
        .unwrap();
    store
        .set_policy_rule(
            "agent:5",
            "build.run",
            Some(EffectKind::Read),
            Decision::Deny,
            1,
        )
        .unwrap();
    store
        .set_policy_rule("agent:5", "build.run", None, Decision::Allow, 9)
        .unwrap();

    let named = |actor, command, effect| store.named_decision(actor, command, effect).unwrap();
    assert_eq!(
        named("agent:1", "build.run", EffectKind::Read),
        Some(Decision::Allow),
        "exact actor beats a newer actor wildcard"
    );
    assert_eq!(
        named("agent:2", "build.run", EffectKind::Read),
        Some(Decision::Deny),
        "actor wildcard applies when no exact actor rule exists"
    );
    assert_eq!(
        named("agent:3", "build.run", EffectKind::Read),
        Some(Decision::Deny),
        "the call's own effect beats a newer rule for another effect"
    );
    assert_eq!(
        named("agent:5", "build.run", EffectKind::Read),
        Some(Decision::Deny),
        "the call's own effect beats a newer effect wildcard"
    );
    assert_eq!(
        named("agent:4", "build.run", EffectKind::Read),
        Some(Decision::Deny),
        "a rule for another effect still names the command"
    );
    assert_eq!(
        named("agent:1", "text.edit", EffectKind::Read),
        None,
        "a rule for another command does not name this one"
    );
}

#[test]
fn a_named_rule_ranks_as_the_direct_call_does() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_policy_rule("agent:0", "script.check", None, Decision::Allow, 1)
        .unwrap();
    store
        .set_policy_rule(
            "*",
            "script.check",
            Some(EffectKind::IndexWrite),
            Decision::Deny,
            9,
        )
        .unwrap();

    let direct = store
        .decision_for("agent:0", "script.check", EffectKind::IndexWrite)
        .unwrap();
    assert_eq!(direct, Some(Decision::Deny));
    assert_eq!(
        store
            .named_decision("agent:0", "script.check", EffectKind::IndexWrite)
            .unwrap(),
        direct
    );
}
