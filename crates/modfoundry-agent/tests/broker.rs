//! Broker contract tests.
//!
//! The broker is the ring above `Core` where a persisted policy authorizes a
//! call and every dispatch — allowed or refused — lands in the audit trail. These tests pin the observable contract: what
//! runs, what is refused *before* it runs, what the trail records, how policy
//! specificity and the session mutation switch resolve, and that the trail
//! survives reopening the database.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use modfoundry_agent::{
    actor_key, check_grant, is_shared_agent, validate_actor_key, AgentError, Broker, Confirm,
    ConfirmAnswer, ConfirmRequest, StoreConfirm, DEFAULT_AGENT_ID,
};
use modfoundry_core::{
    Actor, Command, CommandError, CommandSpec, Core, EffectKind, Grants, Outcome, HOST_UPDATE,
    WORKSPACE_UNDO,
};
use modfoundry_store::audit::AuditOutcome;
use modfoundry_store::policy::Decision;
use modfoundry_store::Store;
use serde_json::{json, Value};

/// Unique temp dir per call (label + pid + per-call counter) so parallel tests
/// in one binary never share a database or marker file.
fn tmp(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mf-agent-{label}-{}-{n}", std::process::id()))
}

/// A marker path inside its own temp dir, so "the file exists" means exactly
/// "the command body ran".
fn marker_path(label: &str) -> PathBuf {
    tmp(label).join("marker")
}

const AGENT: Actor = Actor::Agent {
    id: 0,
    grants: Grants::NONE,
};

/// The body both marker commands share: leave a marker file when it runs, and
/// fail on request.
fn marker_body(id: &str, args: &Value) -> Result<Outcome, CommandError> {
    if let Some(path) = args.get("marker").and_then(Value::as_str) {
        std::fs::write(path, id).map_err(|e| CommandError::Failed(e.to_string()))?;
    }
    if args.get("fail").and_then(Value::as_bool) == Some(true) {
        return Err(CommandError::Failed("planted failure".into()));
    }
    Ok(Outcome::Data(json!({ "command": id })))
}

/// A command that leaves a marker file when its body runs, so a test can tell
/// "refused before the body" from "ran and failed".
struct Marker(&'static str, EffectKind);

impl Command for Marker {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: self.0,
            effect: self.1,
            summary: "test command that marks its own execution",
        }
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        marker_body(self.0, &args)
    }
}

/// A read command that opts out of the audit trail.
struct QuietMarker(&'static str);

impl Command for QuietMarker {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: self.0,
            effect: EffectKind::Read,
            summary: "quiet test command that marks its own execution",
        }
    }

    fn quiet(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        marker_body(self.0, &args)
    }
}

fn registry() -> Core {
    let mut core = Core::new();
    core.register(Box::new(Marker("test.read", EffectKind::Read)));
    core.register(Box::new(Marker("test.mutate", EffectKind::Mutate)));
    core.register(Box::new(Marker("test.destroy", EffectKind::Destructive)));
    core.register(Box::new(Marker("test.launch", EffectKind::Launch)));
    core.register(Box::new(Marker("test.publish", EffectKind::Publish)));
    core.register(Box::new(QuietMarker("test.quiet")));
    core
}

fn broker() -> Broker {
    Broker::in_memory(registry()).expect("in-memory broker")
}

/// Whether the command body left its marker behind.
fn ran(marker: &Path) -> bool {
    marker.is_file()
}

/// Dispatch on the agent actor with a marker path, so the test can see whether
/// the body executed. The marker's directory is created first, so a missing
/// marker always means "the body never ran", never "the write failed".
fn dispatch(broker: &Broker, id: &str, marker: &Path) -> Result<Outcome, AgentError> {
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    broker.dispatch_at(AGENT, id, json!({ "marker": marker }), 1_000)
}

#[test]
fn a_read_runs_for_an_agent_with_no_rule_and_is_audited() {
    let broker = broker();
    let marker = marker_path("read-ran");

    assert!(dispatch(&broker, "test.read", &marker).is_ok());
    assert!(ran(&marker));

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail.len(), 1);
    let row = &trail[0];
    assert_eq!(row.actor, "agent:0");
    assert_eq!(row.command, "test.read");
    assert_eq!(row.effect, EffectKind::Read);
    assert_eq!(row.outcome, AuditOutcome::Ok);
    assert!(row.detail.is_none());
}

#[test]
fn an_ungranted_mutation_is_refused_before_the_body_runs_and_is_audited() {
    let broker = broker();
    let marker = marker_path("mutate-denied");

    let err = dispatch(&broker, "test.mutate", &marker).unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)), "{err:?}");
    assert!(!ran(&marker), "the body ran despite the denial");

    let trail = broker.audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "test.mutate");
    assert_eq!(trail[0].effect, EffectKind::Mutate);
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
    assert!(trail[0].detail.is_some(), "a denial records its reason");
}

#[test]
fn an_unknown_command_is_not_found_and_is_not_audited() {
    let broker = broker();
    let err = broker.dispatch(AGENT, "nope.nope", json!({})).unwrap_err();
    assert!(matches!(err, AgentError::NotFound(_)), "{err:?}");
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn a_body_failure_is_audited_as_an_error_with_its_detail() {
    let broker = broker();
    let err = broker
        .dispatch_at(AGENT, "test.read", json!({ "fail": true }), 1_000)
        .unwrap_err();
    assert!(matches!(err, AgentError::Command(_)), "{err:?}");

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail[0].outcome, AuditOutcome::Error);
    assert!(trail[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("planted failure"));
}

#[test]
fn a_quiet_command_runs_but_leaves_no_audit_trail() {
    let broker = broker();
    let marker = marker_path("quiet-ran");

    assert!(dispatch(&broker, "test.quiet", &marker).is_ok());
    assert!(ran(&marker), "the quiet body did not run");
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn a_quiet_command_failure_leaves_no_audit_trail() {
    let broker = broker();
    let err = broker
        .dispatch_at(AGENT, "test.quiet", json!({ "fail": true }), 1_000)
        .unwrap_err();
    assert!(matches!(err, AgentError::Command(_)), "{err:?}");
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn a_denied_quiet_command_is_still_audited() {
    let broker = broker();
    broker
        .grant("agent:0", "test.quiet", None, Decision::Deny, 900)
        .unwrap();
    let marker = marker_path("quiet-denied");

    let err = dispatch(&broker, "test.quiet", &marker).unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)), "{err:?}");
    assert!(!ran(&marker), "the body ran despite the denial");

    let trail = broker.audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "test.quiet");
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn a_persisted_grant_authorizes_the_call_it_names() {
    let broker = broker();
    let marker = marker_path("mutate-granted");
    broker
        .grant("agent:0", "test.mutate", None, Decision::Allow, 900)
        .unwrap();

    assert!(dispatch(&broker, "test.mutate", &marker).is_ok());
    assert!(ran(&marker));

    // The grant is scoped to the command and the actor: another command of the
    // same effect class is still refused.
    let other = marker_path("mutate-other");
    assert!(matches!(
        dispatch(&broker, "test.destroy", &other).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(!ran(&other));
}

#[test]
fn a_wildcard_grant_covers_every_command_and_effect_class() {
    let broker = broker();
    broker
        .grant("agent:0", "*", None, Decision::Allow, 900)
        .unwrap();
    let marker = marker_path("wildcard");
    for id in ["test.mutate", "test.destroy", "test.publish", "test.launch"] {
        assert!(dispatch(&broker, id, &marker).is_ok(), "{id} refused");
    }
}

#[test]
fn a_deny_rule_outranks_an_interactive_actors_default_reach() {
    let broker = broker();
    broker
        .grant("cli", "test.read", None, Decision::Deny, 900)
        .unwrap();
    let marker = marker_path("cli-denied");

    let err = broker
        .dispatch_at(
            Actor::CliSession,
            "test.read",
            json!({ "marker": marker }),
            1_000,
        )
        .unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)), "{err:?}");
    assert!(!ran(&marker));

    let trail = broker.audit(Some("cli"), 10).unwrap();
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn revoking_a_grant_returns_the_call_to_its_default() {
    let broker = broker();
    let marker = marker_path("revoke");
    broker
        .grant("agent:0", "test.mutate", None, Decision::Allow, 900)
        .unwrap();
    assert!(dispatch(&broker, "test.mutate", &marker).is_ok());

    assert!(broker.revoke("agent:0", "test.mutate", None).unwrap());
    assert!(matches!(
        dispatch(&broker, "test.mutate", &marker).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(!broker.revoke("agent:0", "test.mutate", None).unwrap());
}

#[test]
fn an_ask_rule_is_refused_because_no_session_can_confirm() {
    let broker = broker();
    broker
        .grant("agent:0", "test.mutate", None, Decision::Ask, 900)
        .unwrap();
    let marker = marker_path("ask-agent");
    assert!(matches!(
        dispatch(&broker, "test.mutate", &marker).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(!ran(&marker));

    // The interactive actor is refused too: "ask me" must never read as "yes"
    // while the session has no way to ask.
    broker
        .grant("cli", "test.mutate", None, Decision::Ask, 901)
        .unwrap();
    let err = broker
        .dispatch_at(
            Actor::CliSession,
            "test.mutate",
            json!({ "marker": marker }),
            1_000,
        )
        .unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)), "{err:?}");
    assert!(!ran(&marker));

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail.len(), 2);
    assert!(trail.iter().all(|row| row.outcome == AuditOutcome::Denied));
}

#[test]
fn policy_rules_are_listed_for_frontends() {
    let broker = broker();
    broker
        .grant(
            "agent:1",
            "test.mutate",
            Some(EffectKind::Mutate),
            Decision::Allow,
            900,
        )
        .unwrap();
    let rules = broker.store().policy_rules().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].actor, "agent:1");
    assert_eq!(rules[0].command, "test.mutate");
    assert_eq!(rules[0].effect, Some(EffectKind::Mutate));
    assert_eq!(rules[0].decision, Decision::Allow);
}

#[test]
fn a_rule_whose_selectors_could_never_match_is_refused() {
    let broker = broker();

    // A misspelled actor would persist as a grant that never applies.
    let err = broker
        .grant("agent0", "test.mutate", None, Decision::Allow, 900)
        .unwrap_err();
    assert!(err.to_string().contains("unknown actor"), "{err}");

    // A command id this build does not register would do the same.
    let err = broker
        .grant("agent:0", "build.runn", None, Decision::Allow, 900)
        .unwrap_err();
    assert!(err.to_string().contains("unknown command"), "{err}");

    // Neither attempt left a rule behind, and the wildcards still work.
    assert!(broker.store().policy_rules().unwrap().is_empty());
    assert!(broker.grant("*", "*", None, Decision::Allow, 900).is_ok());

    // Revoking is not validated: removing a rule must always be possible.
    assert!(broker.revoke("agent0", "build.runn", None).is_ok());
}

#[test]
fn check_grant_refuses_what_grant_refuses() {
    let cases = [
        ("agent0", "test.mutate"),
        ("agent:0", "build.runn"),
        ("agent:3", "test.mutate"),
        ("agent:3", "*"),
        ("*", "*"),
    ];
    for (actor, command) in cases {
        let broker = broker();
        let checked = check_grant(broker.core(), actor, command);
        let granted = broker.grant(actor, command, None, Decision::Allow, 900);
        assert_eq!(
            checked.is_ok(),
            granted.is_ok(),
            "{actor} / {command} disagreed: {checked:?} vs {granted:?}"
        );
        if let (Err(checked), Err(granted)) = (checked, granted) {
            assert_eq!(
                checked.to_string(),
                granted.to_string(),
                "{actor} / {command}"
            );
        }
    }
}

#[test]
fn only_the_default_agent_id_is_shared() {
    assert_eq!(
        actor_key(Actor::Agent {
            id: DEFAULT_AGENT_ID,
            grants: Grants::NONE,
        }),
        "agent:0"
    );
    assert!(is_shared_agent("agent:0"));
    for key in ["agent:1", "cli", "*"] {
        assert!(!is_shared_agent(key), "{key} counted as shared");
    }
}

#[test]
fn the_actor_key_vocabulary_is_exactly_the_persisted_one() {
    for key in [
        "*",
        "user",
        "cli",
        "automation",
        "agent:0",
        "agent:18446744073709551615",
    ] {
        assert!(validate_actor_key(key).is_ok(), "{key} rejected");
    }
    for key in [
        "",
        "Agent:0",
        "agent:",
        "agent:-1",
        "agent:1.0",
        "agents",
        "clii",
    ] {
        assert!(validate_actor_key(key).is_err(), "{key} accepted");
    }
}

#[test]
fn session_mutations_reach_mutate_and_launch_but_not_destructive_or_publish() {
    let mut broker = broker();
    broker.allow_session_mutations();
    let marker = marker_path("session");
    for id in ["test.mutate", "test.launch"] {
        assert!(dispatch(&broker, id, &marker).is_ok(), "{id} refused");
    }
    for id in ["test.destroy", "test.publish"] {
        assert!(
            matches!(
                dispatch(&broker, id, &marker).unwrap_err(),
                AgentError::Denied(_)
            ),
            "{id} ran without a grant"
        );
    }
    assert!(
        broker.store().policy_rules().unwrap().is_empty(),
        "not persisted"
    );
}

#[test]
fn session_mutations_do_not_override_a_persisted_deny() {
    let mut broker = broker();
    broker.allow_session_mutations();
    broker
        .grant("agent:0", "test.mutate", None, Decision::Deny, 900)
        .unwrap();
    let marker = marker_path("session-denied");
    assert!(matches!(
        dispatch(&broker, "test.mutate", &marker).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(!ran(&marker));
}

#[test]
fn the_args_hash_is_stable_and_distinguishes_arguments() {
    let broker = broker();
    broker
        .dispatch_at(AGENT, "test.read", json!({ "a": 1 }), 1_000)
        .unwrap();
    broker
        .dispatch_at(AGENT, "test.read", json!({ "a": 1 }), 1_001)
        .unwrap();
    broker
        .dispatch_at(AGENT, "test.read", json!({ "a": 2 }), 1_002)
        .unwrap();

    // Newest first: a:2, then a:1 twice.
    let trail = broker.audit(None, 10).unwrap();
    let hashes: Vec<&str> = trail.iter().map(|e| e.args_hash.as_str()).collect();
    assert_eq!(hashes[1], hashes[2], "same args must hash the same");
    assert_ne!(hashes[0], hashes[1], "different args must hash differently");
    assert_eq!(hashes[0].len(), 64, "a SHA-256 digest, not the raw args");
    assert!(hashes[0].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn actor_keys_name_each_actor_kind() {
    assert_eq!(actor_key(Actor::User), "user");
    assert_eq!(actor_key(Actor::CliSession), "cli");
    assert_eq!(
        actor_key(Actor::Automation {
            grants: Grants::NONE
        }),
        "automation"
    );
    assert_eq!(actor_key(AGENT), "agent:0");
    assert_eq!(
        actor_key(Actor::Agent {
            id: 42,
            grants: Grants::NONE
        }),
        "agent:42"
    );
}

#[test]
fn the_trail_can_be_read_without_dispatching_and_filtered_by_actor() {
    let broker = broker();
    let marker = marker_path("filter");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    broker
        .dispatch_at(AGENT, "test.read", json!({ "marker": marker }), 1_000)
        .unwrap();
    broker
        .dispatch_at(
            Actor::CliSession,
            "test.read",
            json!({ "marker": marker }),
            1_001,
        )
        .unwrap();

    // Reading the trail is not itself audited.
    assert_eq!(broker.audit(None, 10).unwrap().len(), 2);
    assert_eq!(broker.audit(Some("cli"), 10).unwrap().len(), 1);
    assert_eq!(broker.audit(Some("agent:0"), 10).unwrap().len(), 1);
}

#[test]
fn can_run_answers_without_dispatching_or_auditing() {
    let broker = broker();
    assert!(broker.can_run(AGENT, "test.read").unwrap());
    assert!(!broker.can_run(AGENT, "test.mutate").unwrap());
    assert!(!broker.can_run(AGENT, "nope.nope").unwrap());
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn authorize_reports_the_effect_class_or_refuses() {
    let broker = broker();
    assert_eq!(
        broker.authorize(AGENT, "test.read").unwrap(),
        EffectKind::Read
    );
    assert!(matches!(
        broker.authorize(AGENT, "test.mutate").unwrap_err(),
        AgentError::Denied(_)
    ));
}

#[test]
fn the_trail_survives_reopening_the_database_file() {
    let db = tmp("persist").join("modfoundry.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let marker = marker_path("persist-marker");

    {
        let broker = Broker::open(registry(), &db).expect("open file broker");
        dispatch(&broker, "test.read", &marker).unwrap();
    }
    let reopened = Broker::open(registry(), &db).expect("reopen file broker");
    let trail = reopened.audit(None, 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "test.read");
    std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
}

#[test]
fn the_real_registry_dispatches_through_the_broker() {
    let broker = Broker::in_memory(modfoundry_commands::core()).unwrap();
    let dir = tmp("real");
    broker
        .grant("agent:0", "project.scaffold", None, Decision::Allow, 900)
        .unwrap();

    let out = broker
        .dispatch_at(
            AGENT,
            "project.scaffold",
            json!({ "name": "Brokered", "dir": dir }),
            1_000,
        )
        .expect("granted scaffold runs");
    assert!(matches!(out, Outcome::Changed { .. }), "{out:?}");
    assert!(dir.join("config.cpp").is_file());

    // A read needs no grant, and both calls are on the trail.
    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_001)
        .is_ok());
    let commands: Vec<String> = broker
        .audit(None, 10)
        .unwrap()
        .iter()
        .map(|e| e.command.clone())
        .collect();
    assert_eq!(commands, vec!["formats.list", "project.scaffold"]);
}

/// A mutation with a plan: its body leaves a marker, its plan returns a
/// change-set, so a test can tell the two apart.
struct Planner(&'static str);

impl Command for Planner {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: self.0,
            effect: EffectKind::Mutate,
            summary: "plannable mutation (test)",
        }
    }

    fn plan(&self, _args: Value) -> Result<Option<Value>, CommandError> {
        Ok(Some(json!({
            "root": "mod:Test",
            "host_root": "/nowhere",
            "changes": [ { "path": "mod:Test:a.c", "op": "create" } ],
        })))
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        if let Some(path) = args.get("marker").and_then(Value::as_str) {
            std::fs::write(path, "ran").map_err(|e| CommandError::Failed(e.to_string()))?;
        }
        Ok(Outcome::applied(
            modfoundry_core::ChangeSummary {
                kind: EffectKind::Mutate,
                files_changed: 1,
                description: "wrote a.c".into(),
            },
            json!({ "root": "mod:Test", "host_root": "/nowhere", "changes": [] }),
        ))
    }
}

fn planner_core() -> Core {
    let mut core = Core::new();
    core.register(Box::new(Planner("test.plan")));
    core
}

fn planner_broker() -> Broker {
    Broker::in_memory(planner_core()).expect("in-memory broker")
}

#[test]
fn a_preview_returns_the_plan_runs_nothing_and_is_audited_as_a_preview() {
    let broker = planner_broker();
    let marker = marker_path("preview");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

    // The agent may not mutate, but it may read — and a preview is a read.
    let out = broker
        .dispatch_at(
            AGENT,
            "test.plan",
            json!({ "marker": marker, "preview": true }),
            1_000,
        )
        .expect("an ungranted agent may preview a mutation");
    let Outcome::Preview(plan) = out else {
        panic!("expected a preview, got {out:?}");
    };
    assert_eq!(plan["root"], "mod:Test");
    assert!(!ran(&marker), "a preview must not run the body");

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail[0].outcome, AuditOutcome::Preview);
    assert_eq!(trail[0].command, "test.plan");
    assert_eq!(trail[0].change_set, None, "a preview records no change-set");
    assert!(
        broker.store().last_undoable().unwrap().is_none(),
        "a preview puts nothing on the undo stack"
    );
}

#[test]
fn a_recorded_change_set_is_referenced_by_its_audit_row() {
    let broker = planner_broker();
    let marker = marker_path("recorded");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

    let actor = Actor::CliSession;
    broker
        .dispatch_at(actor, "test.plan", json!({ "marker": marker }), 1_000)
        .unwrap();

    let trail = broker.audit(None, 10).unwrap();
    let recorded = trail[0].change_set.expect("the audit row references it");
    let stack = broker
        .store()
        .last_undoable()
        .unwrap()
        .expect("on the stack");
    assert_eq!(stack.id, recorded);
    assert_eq!(stack.command, "test.plan");
    assert_eq!(stack.actor, "cli");
    assert_eq!(stack.changes["root"], "mod:Test");
}

#[test]
fn a_mutation_whose_undo_record_cannot_be_saved_still_succeeds_and_is_audited() {
    let (broker, db) = store_broker_over(planner_core(), "undo-write-fails");
    let marker = marker_path("undo-write-fails");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE undo_log")
        .unwrap();

    let out = broker
        .dispatch_at(
            Actor::CliSession,
            "test.plan",
            json!({ "marker": marker }),
            1_000,
        )
        .expect("the files changed, so the call did not fail");
    assert!(matches!(out, Outcome::Changed { .. }));
    assert!(ran(&marker), "the body ran");

    let trail = broker.audit(None, 1).unwrap();
    assert_eq!(trail[0].outcome, AuditOutcome::Ok);
    assert_eq!(trail[0].change_set, None, "there is no stored undo record");
    assert!(
        trail[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("undo record not saved")),
        "the audit row says the undo record was lost: {:?}",
        trail[0].detail
    );
}

#[test]
fn a_call_that_ran_is_not_reported_as_failed_when_its_audit_row_cannot_be_written() {
    let (broker, db) = store_broker_over(planner_core(), "audit-write-fails");
    let marker = marker_path("audit-write-fails");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE audit_entry")
        .unwrap();

    let out = broker
        .dispatch_at(
            Actor::CliSession,
            "test.plan",
            json!({ "marker": marker }),
            1_000,
        )
        .expect("the body ran, so a lost audit row is not the call's result");
    assert!(matches!(out, Outcome::Changed { .. }));
    assert!(ran(&marker));
}

#[test]
fn undo_reverses_the_last_change_set_once_and_refuses_without_a_grant() {
    let db = tmp("undo").join("agent.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let dir = tmp("undo-target");
    let broker = Broker::with_store(
        modfoundry_commands::core(),
        modfoundry_store::Store::open(&db).unwrap(),
    );

    broker
        .dispatch_at(
            Actor::CliSession,
            "project.scaffold",
            json!({ "name": "Undoable", "dir": dir }),
            1_000,
        )
        .expect("scaffold");
    let record = broker.store().last_undoable().unwrap().expect("recorded");
    assert!(dir.join("config.cpp").is_file());

    // An agent without a grant cannot undo, and the refusal is audited.
    assert!(matches!(
        broker.undo(AGENT, 1_001).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(
        dir.join("config.cpp").is_file(),
        "the refusal changed nothing"
    );
    assert!(broker.store().last_undoable().unwrap().is_some());

    let undone = broker
        .undo(Actor::CliSession, 1_002)
        .unwrap()
        .expect("something to undo");
    assert_eq!(undone.change_set, record.id);
    assert!(undone.first, "this session is the one that reversed it");
    assert!(!dir.join("config.cpp").exists(), "the scaffold is gone");

    // The stack only ever walks backwards.
    assert!(broker.store().last_undoable().unwrap().is_none());
    assert!(broker.undo(Actor::CliSession, 1_003).unwrap().is_none());
    assert!(broker.audit(None, 10).unwrap().iter().any(|row| row.command
        == modfoundry_core::WORKSPACE_UNDO
        && row.outcome == AuditOutcome::Ok));
}

fn store_broker(label: &str) -> Broker {
    store_broker_over(modfoundry_commands::core(), label).0
}

/// A file-backed broker over `core`, plus its database file, so a test can
/// break a table under the running broker.
fn store_broker_over(core: Core, label: &str) -> (Broker, PathBuf) {
    let db = tmp(label).join("agent.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let broker = Broker::with_store(core, modfoundry_store::Store::open(&db).unwrap());
    (broker, db)
}

fn scaffold_into(broker: &Broker, dir: &Path, name: &str, at_unix: i64) {
    broker
        .dispatch_at(
            Actor::CliSession,
            "project.scaffold",
            json!({ "name": name, "dir": dir }),
            at_unix,
        )
        .expect("scaffold");
}

#[test]
fn undo_within_reverses_the_named_folder_and_leaves_a_newer_one_alone() {
    let broker = store_broker("within");
    let a = tmp("within-a");
    let b = tmp("within-b");
    scaffold_into(&broker, &a, "A", 1_000);
    scaffold_into(&broker, &b, "B", 1_001);
    let a_record = broker
        .store()
        .last_undoable_within(&a)
        .unwrap()
        .expect("A on the stack");
    assert!(b.join("config.cpp").is_file());

    let undone = broker
        .undo_within(Actor::CliSession, &a, 1_002)
        .unwrap()
        .expect("something to undo under A");
    assert_eq!(undone.change_set, a_record.id);
    assert!(undone.first);
    assert!(!a.join("config.cpp").exists(), "A's scaffold is gone");
    assert!(
        b.join("config.cpp").is_file(),
        "B's newer scaffold is untouched"
    );

    assert!(broker
        .undo_within(Actor::CliSession, &a, 1_003)
        .unwrap()
        .is_none());
    assert!(broker.store().last_undoable_within(&a).unwrap().is_none());

    let b_record = broker
        .store()
        .last_undoable()
        .unwrap()
        .expect("B on the stack");
    let undone = broker
        .undo(Actor::CliSession, 1_004)
        .unwrap()
        .expect("B to undo");
    assert_eq!(undone.change_set, b_record.id);
    assert!(!b.join("config.cpp").exists());
}

#[test]
fn undo_within_a_folder_with_no_change_sets_is_none_and_changes_nothing() {
    let broker = store_broker("within-empty");
    let elsewhere = tmp("within-elsewhere");
    scaffold_into(&broker, &elsewhere, "Else", 1_000);
    let empty = tmp("within-nothing");

    assert!(broker
        .undo_within(Actor::CliSession, &empty, 1_001)
        .unwrap()
        .is_none());

    assert!(!empty.exists(), "nothing was created under the folder");
    assert!(elsewhere.join("config.cpp").is_file());
    assert!(broker.store().last_undoable().unwrap().is_some());
}

#[test]
fn undo_within_refuses_without_a_grant_with_and_without_a_record() {
    let broker = store_broker("within-deny");
    let a = tmp("within-deny-a");
    scaffold_into(&broker, &a, "A", 1_000);

    assert!(matches!(
        broker.undo_within(AGENT, &a, 1_001).unwrap_err(),
        AgentError::Denied(_)
    ));
    assert!(
        a.join("config.cpp").is_file(),
        "the refusal changed nothing"
    );
    assert!(broker.store().last_undoable_within(&a).unwrap().is_some());

    let empty = tmp("within-deny-empty");
    assert!(matches!(
        broker.undo_within(AGENT, &empty, 1_002).unwrap_err(),
        AgentError::Denied(_)
    ));
}

/// A `Confirm` hook with a scripted answer that counts how often it was asked.
/// Cloning shares the counters, so the test keeps one handle while the broker
/// owns the hook.
#[derive(Clone)]
struct Scripted {
    answer: ConfirmAnswer,
    calls: Arc<AtomicUsize>,
    last: Arc<Mutex<Option<ConfirmRequest>>>,
}

impl Scripted {
    fn new(answer: ConfirmAnswer) -> Self {
        Self {
            answer,
            calls: Arc::new(AtomicUsize::new(0)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last(&self) -> ConfirmRequest {
        self.last.lock().clone().expect("the hook was asked")
    }
}

impl Confirm for Scripted {
    fn confirm(&self, req: &ConfirmRequest) -> ConfirmAnswer {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock() = Some(req.clone());
        self.answer
    }
}

/// A broker over the real registry with an `ask` rule on `formats.list`, and
/// optionally a hook that can answer it.
fn ask_broker(hook: Option<Scripted>) -> Broker {
    let mut broker = Broker::in_memory(modfoundry_commands::core()).expect("in-memory broker");
    broker
        .grant("agent:0", "formats.list", None, Decision::Ask, 900)
        .unwrap();
    if let Some(hook) = hook {
        broker.set_confirm(Box::new(hook));
    }
    broker
}

#[test]
fn an_ask_rule_without_a_hook_keeps_the_session_refusal() {
    let broker = ask_broker(None);
    let err = broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_000)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy requires confirmation for \
         formats.list, and this session cannot ask"
    );

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn an_ask_rule_answered_once_runs_the_call_and_asks_again_next_time() {
    let hook = Scripted::new(ConfirmAnswer::Once);
    let broker = ask_broker(Some(hook.clone()));

    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_000)
        .is_ok());
    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_001)
        .is_ok());
    assert_eq!(hook.calls(), 2, "`once` answers one call only");

    let req = hook.last();
    assert_eq!(req.actor, "agent:0");
    assert_eq!(req.command, "formats.list");
    assert_eq!(req.effect, EffectKind::Read);
    assert_eq!(req.summary, "agent:0 wants to run formats.list (Read)");
    assert_eq!(req.args_hash.len(), 64);
}

#[test]
fn an_ask_rule_answered_for_the_session_stops_asking() {
    let hook = Scripted::new(ConfirmAnswer::Session);
    let broker = ask_broker(Some(hook.clone()));

    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_000)
        .is_ok());
    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_001)
        .is_ok());
    assert_eq!(hook.calls(), 1, "the session answer is remembered");
}

/// A mutation that describes its own confirmation prompt.
struct Described;

impl Command for Described {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: "test.described",
            effect: EffectKind::Publish,
            summary: "test command that describes its own confirmation",
        }
    }

    fn confirm_summary(&self, _args: &Value) -> Option<String> {
        Some("Upload the mod as a new item".into())
    }

    fn execute(&self, _args: Value) -> Result<Outcome, CommandError> {
        Ok(Outcome::Data(json!({})))
    }
}

#[test]
fn a_command_describes_its_own_confirmation_prompt() {
    let mut core = Core::new();
    core.register(Box::new(Described));
    let mut broker = Broker::in_memory(core).expect("in-memory broker");
    broker
        .grant("agent:0", "test.described", None, Decision::Ask, 900)
        .unwrap();
    let hook = Scripted::new(ConfirmAnswer::Once);
    broker.set_confirm(Box::new(hook.clone()));

    broker
        .dispatch_at(AGENT, "test.described", json!({}), 1_000)
        .unwrap();

    assert_eq!(hook.last().summary, "Upload the mod as a new item");
}

#[test]
fn a_session_answer_to_a_preview_does_not_cover_the_real_call() {
    let hook = Scripted::new(ConfirmAnswer::Session);
    let mut broker = planner_broker();
    broker
        .grant("agent:0", "test.plan", None, Decision::Ask, 900)
        .unwrap();
    broker.set_confirm(Box::new(hook.clone()));

    let marker = marker_path("session-preview");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

    broker
        .dispatch_at(
            AGENT,
            "test.plan",
            json!({ "marker": marker, "preview": true }),
            1_000,
        )
        .expect("a preview is a read, so the session may answer it");
    assert_eq!(hook.calls(), 1);
    assert_eq!(hook.last().effect, EffectKind::Read);
    assert!(!ran(&marker));

    broker
        .dispatch_at(AGENT, "test.plan", json!({ "marker": marker }), 1_001)
        .expect("the mutation is asked about on its own, then answered");
    assert_eq!(hook.calls(), 2, "a read answer does not cover the mutation");
    assert_eq!(hook.last().effect, EffectKind::Mutate);
    assert!(ran(&marker));
}

#[test]
fn a_session_answer_covers_only_the_actor_it_was_given_to() {
    let hook = Scripted::new(ConfirmAnswer::Session);
    let broker = ask_broker(Some(hook.clone()));
    broker
        .grant("*", "formats.list", None, Decision::Ask, 900)
        .unwrap();

    assert!(broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_000)
        .is_ok());
    assert_eq!(hook.calls(), 1);

    let other = Actor::Agent {
        id: 1,
        grants: Grants::NONE,
    };
    assert!(broker
        .dispatch_at(other, "formats.list", json!({}), 1_001)
        .is_ok());
    assert_eq!(hook.calls(), 2, "the answer belongs to agent:0");
    assert_eq!(hook.last().actor, "agent:1");
}

#[test]
fn an_agent_cannot_dispatch_undo_with_its_own_change_set() {
    let broker = store_broker("undo-agent");
    let dir = tmp("undo-agent-target");
    scaffold_into(&broker, &dir, "Own", 1_000);
    let record = broker.store().last_undoable().unwrap().expect("recorded");
    broker
        .grant("agent:0", WORKSPACE_UNDO, None, Decision::Allow, 1_001)
        .unwrap();

    let err = broker
        .dispatch_at(
            AGENT,
            WORKSPACE_UNDO,
            json!({ "change_set": record.changes }),
            1_002,
        )
        .unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)));
    assert!(
        dir.join("config.cpp").is_file(),
        "the refusal changed nothing"
    );
    assert!(broker.store().last_undoable().unwrap().is_some());
    let trail = broker.audit(None, 1).unwrap();
    assert_eq!(trail[0].command, WORKSPACE_UNDO);
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);

    let undone = broker
        .undo(AGENT, 1_003)
        .unwrap()
        .expect("the broker's own undo still reverses it");
    assert_eq!(undone.change_set, record.id);
    assert!(!dir.join("config.cpp").exists());
}

#[test]
fn an_ask_rule_answered_deny_refuses_the_call_and_is_audited() {
    let hook = Scripted::new(ConfirmAnswer::Deny);
    let broker = ask_broker(Some(hook.clone()));

    let err = broker
        .dispatch_at(AGENT, "formats.list", json!({}), 1_000)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: confirmation declined for formats.list"
    );
    assert_eq!(hook.calls(), 1);

    let trail = broker.audit(None, 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn can_run_never_prompts_even_when_a_hook_could_answer() {
    let hook = Scripted::new(ConfirmAnswer::Once);
    let broker = ask_broker(Some(hook.clone()));

    // Listing may not block on a human, so an Ask rule is "runnable" only
    // because a hook exists — never because the hook was consulted.
    assert!(broker.can_run(AGENT, "formats.list").unwrap());
    assert!(broker
        .can_run_as(AGENT, "formats.list", EffectKind::Read)
        .unwrap());
    assert_eq!(hook.calls(), 0);

    let bare = ask_broker(None);
    assert!(!bare.can_run(AGENT, "formats.list").unwrap());
    assert_eq!(hook.calls(), 0);
}

#[test]
fn store_confirm_denies_on_timeout_and_records_the_denial() {
    let dir = tmp("confirm-timeout");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("modfoundry.sqlite");
    let store = Store::open(&db).unwrap();

    let timeout = Duration::from_millis(300);
    let confirm = StoreConfirm::new(&db, timeout);
    let handle = std::thread::spawn(move || confirm.confirm(&confirm_request()));

    // The request row lands while the hook waits; the id is needed to read the
    // answer it writes on the way out.
    let id = loop {
        if let Some(row) = store.pending_confirmations().unwrap().first() {
            break row.id;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let started = Instant::now();
    assert_eq!(handle.join().unwrap(), ConfirmAnswer::Deny);
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "it must wait out the timeout rather than assume an answer"
    );

    let row = store.confirmation(id).unwrap().expect("the request row");
    assert_eq!(row.answer, Some(ConfirmAnswer::Deny));
    assert!(
        !store
            .answer_confirmation(id, ConfirmAnswer::Once, 2_000)
            .unwrap(),
        "first writer wins: a late answer cannot resurrect a timed-out request"
    );
    assert_eq!(
        store.confirmation(id).unwrap().unwrap().answer,
        Some(ConfirmAnswer::Deny)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_confirm_returns_the_answer_another_thread_wrote() {
    let dir = tmp("confirm-answer");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("modfoundry.sqlite");

    let answerer_db = db.clone();
    let answerer = std::thread::spawn(move || {
        let store = Store::open(&answerer_db).unwrap();
        let started = Instant::now();
        let id = loop {
            if let Some(row) = store.pending_confirmations().unwrap().first() {
                break row.id;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no confirmation request row appeared"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(store
            .answer_confirmation(id, ConfirmAnswer::Once, 2_000)
            .unwrap());
    });

    let confirm = StoreConfirm::new(&db, Duration::from_secs(5));
    assert_eq!(confirm.confirm(&confirm_request()), ConfirmAnswer::Once);
    answerer.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

fn confirm_request() -> ConfirmRequest {
    ConfirmRequest {
        actor: "agent:0".into(),
        command: "formats.list".into(),
        effect: EffectKind::Read,
        args_hash: "0".repeat(64),
        summary: "agent:0 wants to run formats.list (Read)".into(),
    }
}

/// A read command whose body runs `test.inner`, which it declares.
struct Runner;

impl Command for Runner {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: "test.outer",
            effect: EffectKind::Read,
            summary: "test command that runs another command",
        }
    }

    fn runs(&self) -> &'static [&'static str] {
        &["test.inner"]
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        marker_body("test.outer", &args)
    }
}

fn runner_broker(hook: Option<Scripted>) -> Broker {
    let mut core = Core::new();
    core.register(Box::new(Runner));
    core.register(Box::new(Marker("test.inner", EffectKind::Read)));
    let mut broker = Broker::in_memory(core).expect("in-memory broker");
    if let Some(hook) = hook {
        broker.set_confirm(Box::new(hook));
    }
    broker
}

#[test]
fn a_deny_rule_on_an_inner_command_refuses_the_outer_call_and_is_audited() {
    let broker = runner_broker(None);
    broker
        .grant("agent:0", "test.inner", None, Decision::Deny, 900)
        .unwrap();
    let marker = marker_path("outer-denied");

    let err = dispatch(&broker, "test.outer", &marker).unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy denies test.inner for agent:0, which test.outer runs"
    );
    assert!(!ran(&marker), "the outer body ran despite the inner denial");

    let trail = broker.audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "test.outer");
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn an_ask_rule_on_an_inner_command_asks_with_its_id_and_runs_on_once() {
    let hook = Scripted::new(ConfirmAnswer::Once);
    let broker = runner_broker(Some(hook.clone()));
    broker
        .grant("agent:0", "test.inner", None, Decision::Ask, 900)
        .unwrap();
    let marker = marker_path("outer-asked");

    assert!(dispatch(&broker, "test.outer", &marker).is_ok());
    assert!(ran(&marker));
    assert_eq!(hook.calls(), 1);
    assert_eq!(hook.last().command, "test.inner");
}

#[test]
fn a_wildcard_ask_rule_asks_once_for_the_outer_call() {
    let hook = Scripted::new(ConfirmAnswer::Once);
    let broker = runner_broker(Some(hook.clone()));
    broker
        .grant("agent:0", "*", None, Decision::Ask, 900)
        .unwrap();
    let marker = marker_path("outer-wildcard");

    assert!(dispatch(&broker, "test.outer", &marker).is_ok());
    assert!(ran(&marker));
    assert_eq!(
        hook.calls(),
        1,
        "the wildcard rule is judged on the outer call only"
    );
    assert_eq!(hook.last().command, "test.outer");
}

#[test]
fn an_outer_command_runs_as_before_when_no_rule_names_its_inner_command() {
    let broker = runner_broker(None);
    let marker = marker_path("outer-plain");

    assert!(dispatch(&broker, "test.outer", &marker).is_ok());
    assert!(ran(&marker));
}

#[test]
fn an_allow_rule_on_an_inner_command_does_not_lift_a_deny_on_the_outer_one() {
    let broker = runner_broker(None);
    broker
        .grant("agent:0", "test.outer", None, Decision::Deny, 900)
        .unwrap();
    broker
        .grant("agent:0", "test.inner", None, Decision::Allow, 901)
        .unwrap();
    let marker = marker_path("outer-denied-inner-allowed");

    let err = dispatch(&broker, "test.outer", &marker).unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy denies test.outer for agent:0"
    );
    assert!(!ran(&marker));
}

/// A read command whose body runs two others, in order.
struct TwoRunner;

impl Command for TwoRunner {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: "test.both",
            effect: EffectKind::Read,
            summary: "test command that runs two other commands",
        }
    }

    fn runs(&self) -> &'static [&'static str] {
        &["test.first", "test.second"]
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        marker_body("test.both", &args)
    }
}

#[test]
fn the_first_inner_command_refused_gives_the_reason() {
    let mut core = Core::new();
    core.register(Box::new(TwoRunner));
    core.register(Box::new(Marker("test.first", EffectKind::Read)));
    core.register(Box::new(Marker("test.second", EffectKind::Read)));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    broker
        .grant("agent:0", "test.second", None, Decision::Deny, 900)
        .unwrap();
    broker
        .grant("agent:0", "test.first", None, Decision::Deny, 901)
        .unwrap();
    let marker = marker_path("both-denied");

    let err = dispatch(&broker, "test.both", &marker).unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy denies test.first for agent:0, which test.both runs"
    );
    assert!(!ran(&marker));
}

/// A plannable mutation whose body runs `test.inner`.
struct PlanningRunner(Planner);

impl Command for PlanningRunner {
    fn spec(&self) -> CommandSpec<'static> {
        self.0.spec()
    }

    fn plan(&self, args: Value) -> Result<Option<Value>, CommandError> {
        self.0.plan(args)
    }

    fn runs(&self) -> &'static [&'static str] {
        &["test.inner"]
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        self.0.execute(args)
    }
}

#[test]
fn a_preview_is_refused_when_a_rule_denies_an_inner_command() {
    let mut core = Core::new();
    core.register(Box::new(PlanningRunner(Planner("test.plan"))));
    core.register(Box::new(Marker("test.inner", EffectKind::Read)));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    broker
        .grant("agent:0", "test.inner", None, Decision::Deny, 900)
        .unwrap();

    let err = broker
        .dispatch_at(AGENT, "test.plan", json!({ "preview": true }), 1_000)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy denies test.inner for agent:0, which test.plan runs"
    );
}

#[test]
fn a_deny_rule_on_script_check_refuses_script_lint_for_an_agent() {
    let broker = Broker::in_memory(modfoundry_commands::core()).expect("in-memory broker");
    broker
        .grant("agent:0", "script.lint", None, Decision::Allow, 900)
        .unwrap();
    broker
        .grant("agent:0", "script.check", None, Decision::Deny, 901)
        .unwrap();

    let err = broker
        .dispatch_at(
            AGENT,
            "script.lint",
            json!({ "path": tmp("lint-refused"), "game": tmp("lint-refused-game") }),
            1_000,
        )
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: policy denies script.check for agent:0, which script.lint runs"
    );
}

#[test]
fn an_inner_ask_answered_for_the_session_is_not_asked_again() {
    let hook = Scripted::new(ConfirmAnswer::Session);
    let broker = runner_broker(Some(hook.clone()));
    broker
        .grant("agent:0", "test.inner", None, Decision::Ask, 900)
        .unwrap();
    let marker = marker_path("outer-session");

    assert!(dispatch(&broker, "test.outer", &marker).is_ok());
    assert!(dispatch(&broker, "test.outer", &marker).is_ok());
    assert_eq!(
        hook.calls(),
        1,
        "the session answer covers the inner command too"
    );
}

#[test]
fn an_inner_ask_declined_refuses_the_outer_call_and_is_audited() {
    let hook = Scripted::new(ConfirmAnswer::Deny);
    let broker = runner_broker(Some(hook.clone()));
    broker
        .grant("agent:0", "test.inner", None, Decision::Ask, 900)
        .unwrap();
    let marker = marker_path("outer-declined");

    let err = dispatch(&broker, "test.outer", &marker).unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: confirmation declined for test.inner"
    );
    assert!(
        !ran(&marker),
        "the outer body ran despite the declined inner call"
    );

    let trail = broker.audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "test.outer");
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

/// A read command that declares a command the registry does not hold.
struct Lost;

impl Command for Lost {
    fn spec(&self) -> CommandSpec<'static> {
        CommandSpec {
            id: "test.lost",
            effect: EffectKind::Read,
            summary: "test command that runs a command nothing registers",
        }
    }

    fn runs(&self) -> &'static [&'static str] {
        &["test.nowhere"]
    }

    fn execute(&self, args: Value) -> Result<Outcome, CommandError> {
        marker_body("test.lost", &args)
    }
}

#[test]
fn a_command_that_runs_an_unregistered_command_is_refused() {
    let mut core = Core::new();
    core.register(Box::new(Lost));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    let marker = marker_path("lost");

    let err = dispatch(&broker, "test.lost", &marker).unwrap_err();
    assert_eq!(
        err.to_string(),
        "command rejected by permission policy: test.lost runs test.nowhere, which is not registered"
    );
    assert!(!ran(&marker));
}

#[test]
fn host_update_is_refused_to_an_agent_even_with_a_wildcard_grant() {
    let mut core = registry();
    core.register(Box::new(Marker(HOST_UPDATE, EffectKind::Destructive)));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    broker
        .grant("agent:0", "*", None, Decision::Allow, 900)
        .unwrap();
    let marker = marker_path("host-update-agent");

    let err = dispatch(&broker, HOST_UPDATE, &marker).unwrap_err();
    assert!(matches!(err, AgentError::Denied(_)), "{err:?}");
    assert!(!ran(&marker), "the body ran for an agent");
    assert!(!broker.can_run(AGENT, HOST_UPDATE).unwrap());

    let trail = broker.audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, HOST_UPDATE);
    assert_eq!(trail[0].outcome, AuditOutcome::Denied);
}

#[test]
fn host_update_runs_for_the_user() {
    let mut core = registry();
    core.register(Box::new(Marker(HOST_UPDATE, EffectKind::Destructive)));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    let marker = marker_path("host-update-user");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

    let args = json!({ "marker": marker });
    assert!(broker
        .dispatch_at(Actor::User, HOST_UPDATE, args, 1_000)
        .is_ok());
    assert!(ran(&marker));
}

#[test]
fn host_update_is_refused_to_an_automation_granted_destructive_effects() {
    let mut core = registry();
    core.register(Box::new(Marker(HOST_UPDATE, EffectKind::Destructive)));
    let broker = Broker::in_memory(core).expect("in-memory broker");
    let automation = Actor::Automation {
        grants: Grants::NONE.with(EffectKind::Destructive),
    };

    assert!(!broker.can_run(automation, HOST_UPDATE).unwrap());
}
