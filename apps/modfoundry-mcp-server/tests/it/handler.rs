//! MCP handler tests: drive `Server::handle_line` over JSON-RPC lines
//! (initialize, tools/list, tools/call over a bad config and script, error
//! codes, notifications).

use std::path::{Path, PathBuf};

use modfoundry_mcp_server::Server;
use serde_json::{json, Value};

use crate::common::{handle, open_server, req, tool_names, Temp};

fn write(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, contents).unwrap();
    p
}

/// Build a scaffolded mod then poison its config (4_World→3_Game → MF-C310).
fn planted_bad_config_dir() -> Temp {
    let dir = Temp::new("badcfg");
    modfoundry_commands::scaffold(
        dir.path(),
        &modfoundry_commands::ScaffoldOptions::new("MyMod"),
    )
    .unwrap();
    let cfg = dir.join("config.cpp");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("Scripts/4_World", "Scripts/3_Game")).unwrap();
    dir
}

/// Build a tree with one bad Enforce source in a module dir.
fn planted_bad_script_dir() -> Temp {
    let dir = Temp::new("badscript");
    write(dir.path(), "Scripts/3_Game/Broken.c", "}");
    dir
}

fn tool_result_json(res: &Value) -> (Value, bool) {
    let result = &res["result"];
    let text = result["content"][0]["text"].as_str().unwrap();
    (
        serde_json::from_str(text).unwrap(),
        result["isError"].as_bool().unwrap(),
    )
}

#[test]
fn initialize_echoes_version_and_capabilities() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            json!(1),
            "initialize",
            json!({ "protocolVersion": "2025-03-26" }),
        ),
    );
    assert_eq!(res["id"], 1);
    assert_eq!(res["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(res["result"]["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(
        res["result"]["capabilities"]["resources"]["listChanged"],
        false
    );
    assert_eq!(
        res["result"]["capabilities"]["prompts"]["listChanged"],
        false
    );
    assert_eq!(
        res["result"]["capabilities"]["resources"]["subscribe"], false,
        "subscriptions are not implemented, so they are not advertised"
    );
    assert_eq!(res["result"]["serverInfo"]["name"], "modfoundry-mcp-server");
}

#[test]
fn initialize_defaults_version_when_absent() {
    let s = Server::new();
    let res = handle(&s, &req(json!("a"), "initialize", json!({})));
    assert_eq!(res["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn tools_list_is_the_read_subset_plus_the_commands_an_agent_may_only_preview() {
    // The MCP surface is exactly what the effect gate permits:
    // an ungranted agent reads, and sees the mutations and launches it may
    // *preview* — which is what "external agents default to read + preview"
    // means in discovery. A command with
    // no plan (`diag.stop`) and `workspace.undo` stay invisible. Sorted by id.
    let s = Server::new();
    let res = handle(&s, &req(json!(1), "tools/list", json!({})));
    let tools = res["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "build.contents",
            "build.run",
            "ce.coverage",
            "ce.edit",
            "ce.get_tree",
            "ce.index",
            "ce.reference",
            "ce.source",
            "ce.validate",
            "config.binarize",
            "config.derapify",
            "config.edit",
            "config.get_tree",
            "config.index",
            "config.patches",
            "config.resolve",
            "config.source",
            "config.validate",
            "conflict.check",
            "diag.errors",
            "diag.learn_error",
            "diag.run",
            "diag.run_client",
            "diag.run_diag",
            "diag.status",
            "font.build",
            "formats.list",
            "game.diff",
            "game.snapshot",
            "host.changelog",
            "host.doctor",
            "image.convert",
            "imageset.build",
            "layout.check",
            "pbo.unpack",
            "project.dependencies",
            "project.edit_dependencies",
            "project.scaffold",
            "publish.details",
            "publish.save_details",
            "publish.status",
            "script.check",
            "script.complete",
            "script.definition",
            "script.diagnostics",
            "script.find_class",
            "script.hover",
            "script.index",
            "script.index_status",
            "script.lint",
            "script.outline",
            "script.source",
            "search.index",
            "search.query",
            "sign.check",
            "sign.keygen",
            "text.edit",
            "vfs.list",
            "vfs.read",
        ]
    );
    for tool in tools {
        let description = tool["description"].as_str().unwrap();
        assert!(!description.is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object");
        // Exactly the tools whose schema offers `preview` say they are
        // preview-only — a read command advertises no such argument.
        let preview_only = description.contains("preview only");
        let offers_preview = tool["inputSchema"]["properties"]["preview"]["type"] == "boolean";
        assert_eq!(preview_only, offers_preview, "{}", tool["name"]);
    }
}

#[test]
fn tools_call_does_not_expose_mutating_commands() {
    // `build.run` is registered (the CLI dispatches it) but needs a grant an
    // agent has not been given, so it is not a callable tool.
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            json!(1),
            "tools/call",
            json!({ "name": "build.run", "arguments": { "mod_root": "/tmp/nope" } }),
        ),
    );
    assert_eq!(res["error"]["code"], -32602);
}

#[test]
fn diag_errors_tool_maps_a_planted_rpt_to_its_source_file() {
    let dir = Temp::new("mcperrors");
    let logs = dir.join("profiles");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("DayZServer_x64.RPT"),
        " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable\r\n",
    )
    .unwrap();
    write(dir.path(), "scripts/3_game/broken.c", "class Broken {}\n");

    let s = Server::new();
    let res = handle(
        &s,
        &req(
            json!(1),
            "tools/call",
            json!({ "name": "diag.errors", "arguments": {
                "dir": logs.to_string_lossy(),
                "source_root": dir.join("scripts").to_string_lossy(),
            } }),
        ),
    );
    let (errs, is_error) = tool_result_json(&res);
    assert!(!is_error);
    let errs = errs.as_array().unwrap();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0]["line"], 5);
    assert!(errs[0]["file"].as_str().unwrap().ends_with("broken.c"));
}

#[test]
fn notifications_get_no_reply() {
    let s = Server::new();
    let msg = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
    assert!(s.handle_line(&msg).is_none());
}

#[test]
fn ping_returns_empty_result() {
    let s = Server::new();
    let res = handle(&s, &req(json!(5), "ping", json!({})));
    assert_eq!(res["result"], json!({}));
}

#[test]
fn unknown_method_is_not_found() {
    let s = Server::new();
    let res = handle(&s, &req(json!(1), "bogus.method", json!({})));
    assert_eq!(res["error"]["code"], -32601);
    assert_eq!(res["id"], 1);
}

#[test]
fn malformed_json_is_a_parse_error_with_null_id() {
    let s = Server::new();
    let res = handle(&s, "not json at all");
    assert_eq!(res["error"]["code"], -32700);
    assert_eq!(res["id"], Value::Null);
}

#[test]
fn non_object_is_invalid_request() {
    let s = Server::new();
    let res = handle(&s, "[1,2,3]");
    assert_eq!(res["error"]["code"], -32600);
}

#[test]
fn invalid_request_lacking_method_is_rejected() {
    let s = Server::new();
    let msg = json!({ "jsonrpc": "2.0", "id": 1 }).to_string();
    let res = handle(&s, &msg);
    assert_eq!(res["error"]["code"], -32600);
}

#[test]
fn unknown_tool_is_invalid_params() {
    let s = Server::new();
    let res = handle(&s, &req(json!(1), "tools/call", json!({ "name": "nope" })));
    assert_eq!(res["error"]["code"], -32602);
}

#[test]
fn vfs_read_returns_file_text() {
    let dir = Temp::new("read");
    let file = write(dir.path(), "x.txt", "hello world");
    let s = Server::new();
    let req = req(
        json!(1),
        "tools/call",
        json!({ "name": "vfs.read", "arguments": { "path": file.to_string_lossy() } }),
    );
    let res = handle(&s, &req);
    assert_eq!(res["result"]["content"][0]["text"], "hello world");
    assert_eq!(res["result"]["isError"], false);
}

#[test]
fn vfs_read_missing_file_is_tool_error() {
    let s = Server::new();
    let req = req(
        json!(1),
        "tools/call",
        json!({ "name": "vfs.read", "arguments": { "path": "/no/such/file.c" } }),
    );
    let res = handle(&s, &req);
    assert_eq!(res["result"]["isError"], true);
}

#[test]
fn config_validate_reports_mf_c310_for_poisoned_mod() {
    let dir = planted_bad_config_dir();
    let s = Server::new();
    let req = req(
        json!(1),
        "tools/call",
        json!({ "name": "config.validate", "arguments": { "path": dir.path().to_string_lossy() } }),
    );
    let res = handle(&s, &req);
    let (diags, is_error) = tool_result_json(&res);
    assert!(!is_error);
    let codes: Vec<&str> = diags
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"MF-C310"), "got {codes:?}");
    assert!(diags[0]["severity"].is_string());
}

#[test]
fn script_diagnostics_reports_mf_e1001_for_planted_bad_script() {
    let dir = planted_bad_script_dir();
    let s = Server::new();
    let file = dir.join("Scripts/3_Game/Broken.c");
    let req = req(
        json!(1),
        "tools/call",
        json!({ "name": "script.diagnostics", "arguments": { "path": file.to_string_lossy() } }),
    );
    let res = handle(&s, &req);
    let (findings, is_error) = tool_result_json(&res);
    assert!(!is_error);
    let f = findings.as_array().unwrap();
    assert_eq!(f[0]["code"], "MF-E1001");
    assert!(f[0]["path"].as_str().unwrap().ends_with("Broken.c"));
}

/// A file-backed server whose mutations are open for the session, so tests can
/// record real change-sets. Returns the directory holding the database too.
fn db_server(label: &str) -> (Temp, Server) {
    let dir = Temp::new(label);
    let server = open_server(&dir.db()).allow_mutations();
    (dir, server)
}

fn call_tool(server: &Server, id: u64, name: &str, arguments: Value) -> Value {
    handle(
        server,
        &req(
            json!(id),
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        ),
    )
}

fn tool(server: &Server, name: &str) -> Value {
    let res = handle(server, &req(json!(1), "tools/list", json!({})));
    res["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no tool named {name}"))
        .clone()
}

/// Scaffold one mod per folder through the server, recording a change-set for
/// each (the newer folder's row wins the undo stack's newest-first scan).
fn scaffold(server: &Server, dir: &Path, name: &str) {
    let res = call_tool(
        server,
        1,
        "project.scaffold",
        json!({ "name": name, "dir": dir.to_string_lossy() }),
    );
    assert_eq!(res["result"]["isError"], false, "{res}");
}

#[test]
fn confined_undo_reverses_the_named_folder_and_stops_there() {
    let (dir, server) = db_server("undo-confined");
    let a = dir.join("a");
    let b = dir.join("b");
    let server = server.confine_undo(a.clone());
    scaffold(&server, &a, "A");
    scaffold(&server, &b, "B");
    assert!(a.join("config.cpp").is_file());
    assert!(b.join("config.cpp").is_file());

    let undone = call_tool(&server, 2, "workspace.undo", json!({}));
    assert_eq!(undone["result"]["isError"], false, "{undone}");
    assert!(
        !a.join("config.cpp").exists(),
        "A's change-set was reversed"
    );
    assert!(
        b.join("config.cpp").is_file(),
        "B is under a different folder and must be intact"
    );

    let again = call_tool(&server, 3, "workspace.undo", json!({}));
    let text = again["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("nothing to undo under"), "got {text:?}");
    assert!(
        text.contains(&a.to_string_lossy().to_string()),
        "got {text:?}"
    );
    assert!(b.join("config.cpp").is_file(), "B still intact");
}

#[test]
fn unconfined_undo_reverses_the_newest_change_set() {
    let (dir, server) = db_server("undo-unconfined");
    let a = dir.join("a");
    let b = dir.join("b");
    scaffold(&server, &a, "A");
    scaffold(&server, &b, "B");

    let undone = call_tool(&server, 2, "workspace.undo", json!({}));
    assert_eq!(undone["result"]["isError"], false, "{undone}");
    assert!(!b.join("config.cpp").exists(), "B is newest");
    assert!(a.join("config.cpp").is_file(), "A is older");
}

#[test]
fn a_preview_undo_is_refused_and_undoes_nothing() {
    let (dir, server) = db_server("undo-preview");
    let a = dir.join("a");
    scaffold(&server, &a, "A");

    let preview = call_tool(&server, 2, "workspace.undo", json!({ "preview": true }));
    assert_eq!(preview["result"]["isError"], true, "{preview}");
    let text = preview["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("cannot be previewed"), "got {text:?}");
    assert!(a.join("config.cpp").is_file(), "a preview undid nothing");

    let undone = call_tool(&server, 3, "workspace.undo", json!({}));
    assert_eq!(undone["result"]["isError"], false, "{undone}");
    assert!(
        !a.join("config.cpp").exists(),
        "the change-set was still there to undo"
    );
}

#[test]
fn the_undo_tool_takes_no_arguments() {
    let (dir, server) = db_server("undo-noargs");
    let a = dir.join("a");
    scaffold(&server, &a, "A");

    let schema = &tool(&server, "workspace.undo")["inputSchema"];
    assert!(schema.get("required").is_none(), "{schema}");
    assert!(schema["properties"].get("change_set").is_none(), "{schema}");

    for (n, arguments) in [json!({ "change_set": {} }), json!(["x"]), json!("x")]
        .into_iter()
        .enumerate()
    {
        let named = call_tool(&server, 2 + n as u64, "workspace.undo", arguments);
        assert_eq!(named["result"]["isError"], true, "{named}");
        let text = named["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("takes no arguments"), "got {text:?}");
    }
    assert!(
        a.join("config.cpp").is_file(),
        "a named change-set undid nothing"
    );
}

#[test]
fn the_undo_description_names_the_folder_only_when_confined() {
    let (dir, server) = db_server("undo-desc");
    let a = dir.join("a");
    let plain_names = tool_names(&server);
    assert!(!tool(&server, "workspace.undo")["description"]
        .as_str()
        .unwrap()
        .contains("Only changes under"));

    let confined = server.confine_undo(a.clone());
    let sentence = format!(
        "Only changes under `{}` can be undone in this session.",
        a.display()
    );
    assert!(tool(&confined, "workspace.undo")["description"]
        .as_str()
        .unwrap()
        .contains(&sentence));
    assert_eq!(tool_names(&confined), plain_names, "only the text changed");
}

#[test]
fn tools_reject_missing_path_argument() {
    let s = Server::new();
    let req = req(
        json!(1),
        "tools/call",
        json!({ "name": "config.validate", "arguments": {} }),
    );
    let res = handle(&s, &req);
    assert_eq!(res["result"]["isError"], true);
}

#[test]
fn a_server_without_a_database_runs_diag_run_attached() {
    let s = Server::new().allow_mutations();
    let res = call_tool(
        &s,
        1,
        "diag.run",
        json!({ "mod_dir": "/nonexistent/@Mod", "server_dir": "/nonexistent/server" }),
    );
    assert_eq!(res["result"]["isError"], json!(true));
    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        !text.contains("wait: false needs the agent database"),
        "nobody could see or stop a detached run here, so it runs attached: {text}"
    );
}

#[cfg(unix)]
#[test]
fn a_detached_run_is_recorded_under_the_agent_that_started_it() {
    use modfoundry_agent::Store;
    use std::os::unix::fs::PermissionsExt;

    let dir = Temp::new("agent-actor");
    let install = dir.join("srv");
    let profiles = dir.join("pro");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&profiles).unwrap();
    let exe = install.join("DayZServer");
    let body = format!(
        "#!/bin/sh\necho 'Player connect enabled' >> '{}/DayZServer_x64.RPT'\nn=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done\n",
        profiles.display()
    );
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, PermissionsExt::from_mode(0o755)).unwrap();
    let mod_dir = dir.join("mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();

    let server = Server::with_store(Store::open(dir.db()).unwrap(), Some(dir.db()))
        .with_agent(7)
        .allow_mutations();
    let res = call_tool(
        &server,
        1,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "timeout_secs": 10,
            "run_id": "a1",
        }),
    );
    assert_eq!(res["result"]["isError"], json!(false), "{res}");
    let row = Store::open(dir.db())
        .unwrap()
        .run("a1")
        .unwrap()
        .expect("the launch is recorded");
    assert_eq!(row.actor, "agent:7");
    call_tool(&server, 2, "diag.stop", json!({ "run_id": "a1" }));
}
