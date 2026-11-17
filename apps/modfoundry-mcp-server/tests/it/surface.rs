//! MCP resources/prompts surface.
//!
//! Wire shapes follow the 2025-06-18 revision:
//! `resources/list`,
//! `resources/templates/list`, `resources/read`, `prompts/list` and
//! `prompts/get` — and the same rule as the tool list applies, a resource (or
//! prompt) binds a registered command, so a command the session may not run
//! takes its resource with it.

use modfoundry_agent::Decision;
use modfoundry_mcp_server::Server;
use serde_json::{json, Value};

use crate::common::{handle, open_server, req, store, Temp};

fn names_of(values: &Value, array: &str, field: &str) -> Vec<String> {
    values["result"][array]
        .as_array()
        .unwrap_or_else(|| panic!("no {array} in {values}"))
        .iter()
        .map(|v| v[field].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn resources_list_holds_the_concrete_resources_this_session_may_read() {
    let s = Server::new();
    let res = handle(&s, &req(1, "resources/list", json!({})));

    assert_eq!(
        names_of(&res, "resources", "uri"),
        ["modfoundry://formats", "modfoundry://host"]
    );
    for resource in res["result"]["resources"].as_array().unwrap() {
        assert!(!resource["name"].as_str().unwrap().is_empty());
        assert!(!resource["description"].as_str().unwrap().is_empty());
        assert!(resource["mimeType"].is_string());
    }
    assert!(
        res["result"]["nextCursor"].is_null(),
        "a list that fits one page carries no cursor: {res}"
    );
}

#[test]
fn resource_templates_list_holds_the_parameterized_resources() {
    let s = Server::new();
    let res = handle(&s, &req(1, "resources/templates/list", json!({})));

    assert_eq!(
        names_of(&res, "resourceTemplates", "uriTemplate"),
        [
            "modfoundry://ce/{path}",
            "modfoundry://config/{path}",
            "modfoundry://file/{path}",
            "modfoundry://script/{path}"
        ]
    );
    for template in res["result"]["resourceTemplates"].as_array().unwrap() {
        assert!(!template["name"].as_str().unwrap().is_empty());
    }
}

#[test]
fn reading_a_concrete_resource_returns_its_payload_as_text() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            1,
            "resources/read",
            json!({ "uri": "modfoundry://formats" }),
        ),
    );

    let contents = res["result"]["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "modfoundry://formats");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let formats: Value = serde_json::from_str(contents[0]["text"].as_str().unwrap()).unwrap();
    assert!(formats.as_array().unwrap().len() > 5, "{formats}");
}

#[test]
fn reading_a_file_resource_returns_the_file_text() {
    let dir = Temp::new("resource-file");
    let file = dir.join("My File.c");
    std::fs::write(&file, "class A {};").unwrap();
    // A URI path is percent-encoded on the wire; the server decodes it.
    let uri = format!(
        "modfoundry://file/{}",
        file.display().to_string().replace(' ', "%20")
    );

    let s = Server::new();
    let res = handle(&s, &req(1, "resources/read", json!({ "uri": uri })));

    let contents = res["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["uri"], uri);
    assert_eq!(contents[0]["mimeType"], "text/plain");
    assert_eq!(contents[0]["text"], "class A {};");
}

#[test]
fn reading_a_resource_that_is_not_there_is_resource_not_found() {
    // The spec's own example: a resource whose URI names nothing resolves to
    // `-32002` with the URI in `data` — not an internal error.
    let absent = format!("/tmp/mf-mcp-surface-absent-{}", std::process::id());
    let uris = [
        format!("modfoundry://file/{absent}/Gone.c"),
        format!("modfoundry://config/{absent}/config.cpp"),
        format!("modfoundry://ce/{absent}/db/types.xml"),
    ];
    let s = Server::new();
    for uri in uris {
        let res = handle(&s, &req(1, "resources/read", json!({ "uri": uri })));

        assert_eq!(res["error"]["code"], -32002, "{uri}: {res}");
        assert_eq!(
            res["error"]["message"], "Resource not found",
            "{uri}: {res}"
        );
        assert_eq!(res["error"]["data"]["uri"], uri, "{uri}: {res}");
        assert!(
            res["error"]["data"]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "{uri}: the cause still reaches the agent: {res}"
        );
    }
}

#[test]
fn reading_a_config_resource_returns_the_class_tree() {
    let dir = Temp::new("resource-config");
    let file = dir.join("config.cpp");
    std::fs::write(&file, "class Root { a = 1; };\n").unwrap();
    let uri = format!("modfoundry://config/{}", file.display());

    let s = Server::new();
    let res = handle(&s, &req(1, "resources/read", json!({ "uri": uri })));

    let contents = res["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["uri"], uri);
    assert_eq!(contents[0]["mimeType"], "application/json");
    let tree: Value = serde_json::from_str(contents[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(tree["outline"]["roots"][0]["name"], "Root", "{tree}");
    assert_eq!(tree["diagnostics"], json!([]));
}

#[test]
fn reading_a_ce_resource_returns_the_record_tree() {
    let dir = Temp::new("resource-ce");
    let file = dir.join("types.xml");
    std::fs::write(
        &file,
        "<types>\n    <type name=\"A\">\n        <nominal>1</nominal>\n    </type>\n</types>\n",
    )
    .unwrap();
    let uri = format!("modfoundry://ce/{}", file.display());

    let s = Server::new();
    let res = handle(&s, &req(1, "resources/read", json!({ "uri": uri })));

    let contents = res["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["uri"], uri);
    assert_eq!(contents[0]["mimeType"], "application/json");
    let tree: Value = serde_json::from_str(contents[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(tree["kind"], "types", "{tree}");
    assert_eq!(tree["sections"][0]["records"][0]["key"], "A");
}

#[test]
fn an_unknown_resource_uri_is_not_found() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            1,
            "resources/read",
            json!({ "uri": "modfoundry://nothing/here" }),
        ),
    );
    assert_eq!(res["error"]["code"], -32002);
    assert_eq!(
        res["error"]["data"]["uri"], "modfoundry://nothing/here",
        "{res}"
    );
    assert!(
        res["error"]["data"]["reason"].is_null(),
        "an unknown resource stays opaque — there is no body to blame: {res}"
    );
}

#[test]
fn bad_resource_and_prompt_requests_are_invalid_params() {
    {
        let s = Server::new();
        let res = handle(&s, &req(1, "resources/read", json!({})));
        assert_eq!(res["error"]["code"], -32602);
    }
    {
        let s = Server::new();
        let res = handle(&s, &req(1, "prompts/get", json!({ "name": "nope" })));
        assert_eq!(res["error"]["code"], -32602);
    }
}

#[test]
fn an_unparsable_cursor_is_invalid_params() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(1, "resources/list", json!({ "cursor": "not-a-cursor" })),
    );
    assert_eq!(res["error"]["code"], -32602);
}

#[test]
fn a_denied_command_takes_its_resources_with_it() {
    let work = Temp::new("res-deny");
    let db = work.db();
    store(&db)
        .grant("agent:1", "vfs.read", None, Decision::Deny, 1_000)
        .unwrap();

    let s = open_server(&db).with_agent(1);
    let listed = handle(&s, &req(1, "resources/templates/list", json!({})));
    assert!(
        !names_of(&listed, "resourceTemplates", "uriTemplate")
            .contains(&"modfoundry://file/{path}".into()),
        "a resource this session may not read is not a resource: {listed}"
    );

    let read = handle(
        &s,
        &req(
            1,
            "resources/read",
            json!({ "uri": "modfoundry://file/tmp/x.c" }),
        ),
    );
    assert_eq!(
        read["error"]["code"], -32002,
        "a denied resource is indistinguishable from an unknown one: {read}"
    );
}

#[test]
fn prompts_list_exposes_the_templates_this_session_may_act_on() {
    let s = Server::new();
    let res = handle(&s, &req(1, "prompts/list", json!({})));

    assert_eq!(
        names_of(&res, "prompts", "name"),
        ["diagnose_last_run", "lint_module", "scaffold_mod"]
    );
    for prompt in res["result"]["prompts"].as_array().unwrap() {
        assert!(!prompt["description"].as_str().unwrap().is_empty());
        for argument in prompt["arguments"].as_array().unwrap() {
            assert!(!argument["name"].as_str().unwrap().is_empty());
        }
    }
}

#[test]
fn prompts_get_renders_a_user_message_from_its_arguments() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            1,
            "prompts/get",
            json!({ "name": "lint_module", "arguments": { "path": "/tmp/MyMod/Scripts" } }),
        ),
    );

    let result = &res["result"];
    assert!(result["description"].is_string());
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"]["type"], "text");
    let text = messages[0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("/tmp/MyMod/Scripts"), "{text}");
}

#[test]
fn a_prompt_missing_a_required_argument_is_invalid_params() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            1,
            "prompts/get",
            json!({ "name": "lint_module", "arguments": {} }),
        ),
    );
    assert_eq!(res["error"]["code"], -32602);
}

#[test]
fn a_denied_command_takes_its_prompt_with_it() {
    let work = Temp::new("prompt-deny");
    let db = work.db();
    store(&db)
        .grant("agent:1", "script.diagnostics", None, Decision::Deny, 1_000)
        .unwrap();

    let s = open_server(&db).with_agent(1);
    let listed = handle(&s, &req(1, "prompts/list", json!({})));
    assert!(
        !names_of(&listed, "prompts", "name").contains(&"lint_module".into()),
        "{listed}"
    );

    let got = handle(&s, &req(1, "prompts/get", json!({ "name": "lint_module" })));
    assert_eq!(got["error"]["code"], -32602);
}

#[test]
fn unadvertised_resource_methods_are_method_not_found() {
    let s = Server::new();
    let res = handle(
        &s,
        &req(
            1,
            "resources/subscribe",
            json!({ "uri": "modfoundry://formats" }),
        ),
    );
    assert_eq!(res["error"]["code"], -32601);
}
