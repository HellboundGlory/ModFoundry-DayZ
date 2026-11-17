//! MCP × agent-policy wiring.
//!
//! The tool surface is not a fixed list: the persisted policy decides which
//! registered commands become tools, a grant written by `mf agent grant`
//! reaches a freshly started session, and every tool call lands in the audit
//! trail under the session's agent id. These tests drive the real handler over
//! JSON-RPC lines and read the trail back out of the same database file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use modfoundry_agent::{ConfirmAnswer, Decision, Store};
use modfoundry_mcp_server::Server;
use serde_json::{json, Value};

use crate::common::{handle, open_server, req, store, tool_names, Temp};

fn call(server: &Server, name: &str, arguments: Value) -> Value {
    handle(
        server,
        &req(
            2,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        ),
    )
}

#[test]
fn session_mutations_expose_and_run_a_mutating_tool() {
    let server = Server::new().allow_mutations();
    assert!(
        tool_names(&server).contains(&"project.scaffold".to_owned()),
        "--allow-mutations must open the mutating tools"
    );

    let work = Temp::new("scaffold");
    let dir = work.join("mod");
    let res = call(
        &server,
        "project.scaffold",
        json!({ "name": "McpMod", "dir": dir }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(dir.join("config.cpp").is_file(), "the tool ran");
}

#[test]
fn a_persisted_grant_reaches_a_fresh_session_for_that_agent_only() {
    let work = Temp::new("grant");
    let db = work.db();
    {
        let broker = store(&db);
        broker
            .grant("agent:5", "project.scaffold", None, Decision::Allow, 1_000)
            .unwrap();
    }

    let granted = open_server(&db).with_agent(5);
    assert!(tool_names(&granted).contains(&"project.scaffold".to_owned()));

    let dir = work.join("granted-scaffold");
    let res = call(
        &granted,
        "project.scaffold",
        json!({ "name": "Granted", "dir": dir }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(dir.join("config.cpp").is_file());

    // The grant names one agent id. Another session still *sees* the mutation —
    // it may preview it — but its tool is marked preview-only, which is exactly
    // the difference the grant makes.
    let other = open_server(&db).with_agent(6);
    assert!(tool_names(&other).contains(&"project.scaffold".to_owned()));
    let tools = handle(&other, &req(1, "tools/list", json!({})));
    let scaffold = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "project.scaffold")
        .expect("listed")
        .clone();
    assert!(
        scaffold["description"]
            .as_str()
            .unwrap()
            .contains("preview only"),
        "an ungranted session may only preview it: {scaffold}"
    );

    // …and that is *behaviour*, not just a label: the same call agent:5 was
    // granted to run is refused for agent:6, and it writes nothing.
    let other_dir = work.join("granted-scaffold-other");
    let res = call(
        &other,
        "project.scaffold",
        json!({ "name": "Granted", "dir": other_dir }),
    );
    assert_eq!(
        res["error"]["code"], -32602,
        "the grant names one agent id: {res}"
    );
    assert!(!other_dir.exists(), "a refused call writes nothing");

    // A preview, on the other hand, is reachable by both.
    let res = call(
        &other,
        "project.scaffold",
        json!({ "name": "Granted", "dir": other_dir, "preview": true }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(!other_dir.exists());
}

#[test]
fn a_persisted_deny_removes_a_read_tool_and_records_the_refusal() {
    let work = Temp::new("deny");
    let db = work.db();
    {
        let broker = store(&db);
        broker
            .grant("agent:0", "formats.list", None, Decision::Deny, 1_000)
            .unwrap();
    }

    let server = open_server(&db);
    assert!(!tool_names(&server).contains(&"formats.list".to_owned()));

    // A denied command is not a tool at all, and the refusal is on the trail.
    let res = call(&server, "formats.list", json!({}));
    assert_eq!(res["error"]["code"], -32602, "{res}");

    let trail = store(&db).audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "formats.list");
    assert_eq!(
        trail[0].outcome,
        modfoundry_agent::AuditOutcome::Denied,
        "a refused call is the interesting row"
    );
}

#[test]
fn every_tool_call_is_audited_under_the_session_agent_id() {
    let work = Temp::new("attribution");
    let db = work.db();
    let server = open_server(&db).with_agent(7);
    let res = call(&server, "formats.list", json!({}));
    assert_eq!(res["result"]["isError"], false, "{res}");

    let trail = store(&db).audit(None, 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].actor, "agent:7");
    assert_eq!(trail[0].command, "formats.list");
    assert_eq!(trail[0].outcome, modfoundry_agent::AuditOutcome::Ok);
    assert_eq!(trail[0].args_hash.len(), 64);
}

#[test]
fn an_ungranted_session_can_preview_a_mutation_it_may_not_run() {
    let server = Server::new();
    let work = Temp::new("preview");
    let dir = work.join("mod");
    let res = call(
        &server,
        "project.scaffold",
        json!({ "name": "Previewed", "dir": dir, "preview": true }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("config.cpp"), "{text}");
    assert!(!dir.exists(), "a preview writes nothing");

    // The same session may not run it for real.
    let res = call(
        &server,
        "project.scaffold",
        json!({ "name": "X", "dir": dir }),
    );
    assert_eq!(res["error"]["code"], -32602, "{res}");
}

#[test]
fn workspace_undo_rolls_the_last_tool_call_back() {
    let work = Temp::new("undo");
    let db = work.db();
    let server = open_server(&db).allow_mutations();
    let dir = work.join("undo-target");
    let res = call(
        &server,
        "project.scaffold",
        json!({ "name": "Undoable", "dir": dir }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(dir.join("config.cpp").is_file());

    let res = call(&server, "workspace.undo", json!({}));
    assert_eq!(res["result"]["isError"], false, "{res}");
    assert!(!dir.join("config.cpp").exists(), "the scaffold is gone");

    // The stack only ever walks backwards.
    let res = call(&server, "workspace.undo", json!({}));
    assert_eq!(
        res["result"]["content"][0]["text"], "nothing to undo",
        "{res}"
    );

    let trail = store(&db).audit(None, 10).unwrap();
    assert!(
        trail
            .iter()
            .any(|row| row.command == modfoundry_core::WORKSPACE_UNDO),
        "the undo is on the trail"
    );
}

#[test]
fn a_session_without_a_grant_cannot_undo() {
    let work = Temp::new("undo-denied");
    let db = work.db();
    {
        // Arrange a change-set through the host (a CLI session may mutate).
        let host = store(&db);
        let dir = work.join("undo-denied-target");
        host.dispatch_at(
            modfoundry_core::Actor::CliSession,
            "project.scaffold",
            json!({ "name": "Denied", "dir": dir }),
            1_000,
        )
        .unwrap();
    }
    let server = open_server(&db);
    let res = call(&server, "workspace.undo", json!({}));
    assert_eq!(
        res["error"]["code"], -32602,
        "an unauthorized undo is not a tool: {res}"
    );
}

#[test]
fn a_session_sees_the_mutations_it_may_only_preview() {
    let server = Server::new();
    let names = tool_names(&server);
    assert!(
        names.contains(&"project.scaffold".to_owned()),
        "an ungranted agent must be able to discover what it may preview: {names:?}"
    );
    assert!(
        !names.contains(&"workspace.undo".to_owned()),
        "nothing may be previewed through undo, and it is not granted: {names:?}"
    );

    // The tool says so, and its schema advertises the argument that works.
    let tools = handle(&server, &req(1, "tools/list", json!({}))).clone();
    let scaffold = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "project.scaffold")
        .expect("listed")
        .clone();
    assert!(
        scaffold["description"]
            .as_str()
            .unwrap()
            .contains("preview only"),
        "{scaffold}"
    );
    assert_eq!(
        scaffold["inputSchema"]["properties"]["preview"]["type"],
        "boolean"
    );
}

#[test]
fn a_persisted_deny_also_hides_a_previewable_mutation() {
    let work = Temp::new("preview-denied");
    let db = work.db();
    {
        let broker = store(&db);
        broker
            .grant("agent:0", "project.scaffold", None, Decision::Deny, 1_000)
            .unwrap();
    }
    let server = open_server(&db);
    assert!(!tool_names(&server).contains(&"project.scaffold".to_owned()));

    let dir = work.join("preview-denied-target");
    let res = call(
        &server,
        "project.scaffold",
        json!({ "name": "X", "dir": dir, "preview": true }),
    );
    assert_eq!(
        res["error"]["code"], -32602,
        "a denied command is not a tool, preview or not: {res}"
    );
    assert!(!dir.exists());
}

/// Answer the next confirmation raised in `db`, the way a GUI frontend would.
fn answer_next_confirmation(db: PathBuf, answer: ConfirmAnswer) {
    let store = Store::open(&db).unwrap();
    for _ in 0..400 {
        if let Some(row) = store.pending_confirmations().unwrap().first() {
            assert!(
                store.answer_confirmation(row.id, answer, 2_000).unwrap(),
                "this answerer is the first to answer"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no confirmation request ever reached the database");
}

fn ask_on_formats_list(db: &Path) {
    store(db)
        .grant("agent:0", "formats.list", None, Decision::Ask, 1_000)
        .unwrap();
}

#[test]
fn an_ask_timeout_session_runs_a_call_a_gui_answers() {
    let work = Temp::new("ask-answered");
    let db = work.db();
    ask_on_formats_list(&db);
    let server = open_server(&db).with_store_confirm(&db, Duration::from_secs(2));

    let answerer = std::thread::spawn({
        let db = db.clone();
        move || answer_next_confirmation(db, ConfirmAnswer::Once)
    });

    let res = call(&server, "formats.list", json!({}));
    assert_eq!(res["result"]["isError"], false, "{res}");
    answerer.join().unwrap();

    let trail = store(&db).audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].outcome, modfoundry_agent::AuditOutcome::Ok);
}

#[test]
fn an_ask_timeout_session_without_an_answerer_refuses_and_audits_it() {
    let work = Temp::new("ask-unanswered");
    let db = work.db();
    ask_on_formats_list(&db);
    let server = open_server(&db).with_store_confirm(&db, Duration::from_secs(1));

    let started = Instant::now();
    let res = call(&server, "formats.list", json!({}));
    assert_eq!(res["error"]["code"], -32602, "{res}");
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "it must wait out the timeout rather than refuse at once"
    );

    let trail = store(&db).audit(Some("agent:0"), 10).unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].command, "formats.list");
    assert_eq!(trail[0].outcome, modfoundry_agent::AuditOutcome::Denied);
}
#[test]
fn an_ungranted_session_can_preview_an_edit_but_not_apply_it() {
    let server = Server::new();
    let work = Temp::new("edit-preview");
    let rows = [
        (
            "config.edit",
            "config.cpp",
            "class Root\n{\n    // keep\n    value = 1;\n};\n",
            json!([{ "add_property": { "class": ["Root"], "name": "other", "value": { "number": "2" }, "position": "last" } }]),
            "config",
        ),
        (
            "ce.edit",
            "types.xml",
            "<types>\n    <!-- keep -->\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n    </type>\n</types>\n",
            json!([{ "set_field": { "record": { "key": "FieldCanteen" }, "field": "nominal", "value": "30" } }]),
            "ce",
        ),
    ];
    for (command, file, original, ops, label) in rows {
        let path = work.join(file);
        std::fs::write(&path, original).unwrap();

        assert!(
            tool_names(&server).contains(&command.to_owned()),
            "{label}: an ungranted agent must be able to discover the edit it may preview"
        );

        let res = call(
            &server,
            command,
            json!({ "path": path, "ops": ops.clone(), "preview": true }),
        );
        assert_eq!(res["result"]["isError"], false, "{label}: {res}");
        let text = res["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(file), "{label}: {text}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "{label}: a preview writes nothing"
        );

        let res = call(&server, command, json!({ "path": path, "ops": ops }));
        assert_eq!(res["error"]["code"], -32602, "{label}: {res}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "{label}: a refused apply writes nothing"
        );
    }
}
