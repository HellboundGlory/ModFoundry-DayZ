//! ModFoundry MCP server library surface.
//!
//! A thin JSON-RPC 2.0 frontend over the shared command bodies
//! (`modfoundry-commands`), dispatched through the agent broker so the
//! persisted policy authorizes each call and the audit trail records it.
//! [`Server`] handles one inbound MCP line at a time; [`Server::serve`] drives
//! it over a `BufRead`/`Write` pair (the binary uses stdin/stdout). Handlers are
//! transport-agnostic so an SSE transport can be added later without touching
//! tool logic.

pub mod jsonrpc;
pub mod paging;
pub mod prompts;
pub mod resources;
pub mod server;

pub use server::Server;
