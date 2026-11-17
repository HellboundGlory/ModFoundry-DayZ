//! End-to-end transport test: drive `serve` over a real reader/writer pair
//! (one newline-delimited JSON object per line), as an MCP client would.

use std::io::{BufReader, Cursor};

use modfoundry_agent::Decision;
use modfoundry_mcp_server::Server;

use crate::common::{open_server, store, Temp};

/// Feed newline-delimited requests and return the response lines.
fn roundtrip(messages: &[&str]) -> Vec<String> {
    let input = messages.join("\n") + "\n";
    let mut reader = BufReader::new(Cursor::new(input));
    let mut out: Vec<u8> = Vec::new();
    Server::new().serve(&mut reader, &mut out).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn handshake_and_tool_call_over_stdio_framing() {
    let dir = Temp::new("e2e");
    std::fs::create_dir_all(dir.join("Scripts/3_Game")).unwrap();
    std::fs::write(dir.join("Scripts/3_Game/Broken.c"), "}").unwrap();

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
    let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"script.diagnostics","arguments":{{"path":"{}"}}}}}}"#,
        dir.join("Scripts/3_Game/Broken.c").display()
    );

    let out = roundtrip(&[init, initialized, list, &call]);

    // initialize + tools/list responses, then the call result; the notification
    // produces no line, so exactly three responses.
    assert_eq!(out.len(), 3);
    let init_res: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
    assert_eq!(init_res["id"], 1);
    assert_eq!(init_res["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(out[0].lines().count(), 1, "one JSON object per line");

    let list_res: serde_json::Value = serde_json::from_str(&out[1]).unwrap();
    let tools: Vec<&str> = list_res["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // The tool list is the registry gated by this session's reach:
    // the read subset, plus the commands an ungranted session may only preview.
    for id in ["vfs.read", "script.diagnostics", "config.validate"] {
        assert!(tools.contains(&id), "{id} missing from {tools:?}");
    }
    assert!(tools.contains(&"build.run"), "{tools:?}");
    assert!(tools.contains(&"diag.run"), "{tools:?}");
    for id in ["diag.stop", "workspace.undo"] {
        assert!(
            !tools.contains(&id),
            "{id} is neither readable nor previewable here: {tools:?}"
        );
    }

    let call_res: serde_json::Value = serde_json::from_str(&out[2]).unwrap();
    assert_eq!(call_res["id"], 3);
    assert_eq!(call_res["result"]["isError"], false);
    let text = call_res["result"]["content"][0]["text"].as_str().unwrap();
    let findings: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(findings[0]["code"], "MF-E1001");
}

#[test]
fn a_served_session_leaves_an_empty_write_ahead_log() {
    let dir = Temp::new("wal");
    let db = dir.db();
    let keeper = store(&db);
    keeper
        .grant("agent:0", "script.diagnostics", None, Decision::Allow, 1)
        .unwrap();
    let wal = db.with_extension("sqlite-wal");
    assert!(
        std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0) > 0,
        "the write went to the log first"
    );

    let mut reader = BufReader::new(Cursor::new(""));
    let mut out: Vec<u8> = Vec::new();
    open_server(&db).serve(&mut reader, &mut out).unwrap();

    assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
}

#[test]
fn garbage_line_yields_parse_error_then_continues() {
    let out = roundtrip(&[
        "not json",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
    ]);
    assert_eq!(out.len(), 2);
    let err: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
    assert_eq!(err["error"]["code"], -32700);
    let pong: serde_json::Value = serde_json::from_str(&out[1]).unwrap();
    assert_eq!(pong["result"], serde_json::json!({}));
}
