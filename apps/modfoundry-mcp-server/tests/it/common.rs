//! Helpers shared by the MCP test modules.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use modfoundry_agent::{Broker, Store};
use modfoundry_mcp_server::Server;
use serde::Serialize;
use serde_json::{json, Value};

/// A temporary directory, removed with its contents when it goes.
pub struct Temp(PathBuf);

impl Temp {
    pub fn new(label: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mf-mcp-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.0.join(rel)
    }

    pub fn db(&self) -> PathBuf {
        self.join("modfoundry.sqlite")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A server over the database at `db`, whose rules `mf agent …` writes reach it.
pub fn open_server(db: &Path) -> Server {
    Server::with_store(Store::open(db).expect("open the agent database"), None)
}

/// Drive one JSON-RPC line through `server` and parse the response.
pub fn handle(server: &Server, line: &str) -> Value {
    let out = server.handle_line(line).expect("a response");
    serde_json::from_str(&out).expect("valid JSON")
}

/// A JSON-RPC request line.
pub fn req(id: impl Serialize, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// A broker over the database at `db`, for arranging policy the way `mf agent …` does.
pub fn store(db: &Path) -> Broker {
    Broker::open(modfoundry_commands::core(), db).expect("open agent database")
}

/// The names of the tools `server` lists.
pub fn tool_names(server: &Server) -> Vec<String> {
    let res = handle(server, &req(1, "tools/list", json!({})));
    res["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}
