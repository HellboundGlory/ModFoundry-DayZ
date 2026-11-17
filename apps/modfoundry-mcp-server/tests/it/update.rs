//! `host.update` replaces the install, so no agent session lists it or runs it.

use modfoundry_mcp_server::Server;
use serde_json::json;

use crate::common::{handle, req, tool_names};

#[test]
fn an_agent_neither_lists_nor_runs_host_update() {
    for server in [Server::new(), Server::new().allow_mutations()] {
        assert!(!tool_names(&server).contains(&"host.update".to_owned()));
        let res = handle(
            &server,
            &req(
                2,
                "tools/call",
                json!({ "name": "host.update", "arguments": {} }),
            ),
        );
        assert_eq!(
            res["error"]["message"], "unknown tool: host.update",
            "{res}"
        );
    }
}

#[test]
fn a_saved_wildcard_grant_still_leaves_host_update_out() {
    let work = crate::common::Temp::new("update-wildcard");
    let db = work.db();
    crate::common::store(&db)
        .grant(
            "agent:5",
            "*",
            None,
            modfoundry_agent::Decision::Allow,
            1_000,
        )
        .unwrap();
    let server = crate::common::open_server(&db)
        .with_agent(5)
        .allow_mutations();

    assert!(!tool_names(&server).contains(&"host.update".to_owned()));
    let res = handle(
        &server,
        &req(
            2,
            "tools/call",
            json!({ "name": "host.update", "arguments": {} }),
        ),
    );
    assert_eq!(
        res["error"]["message"], "unknown tool: host.update",
        "{res}"
    );
}
