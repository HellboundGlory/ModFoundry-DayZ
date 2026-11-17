//! MCP server protocol & handler.
//!
//! Thin: tools *are* the registry's dispatchable subset — `tools/list`
//! enumerates the commands this session's actor may run and `tools/call`
//! dispatches them through the agent broker, so an MCP agent can never reach an
//! operation the permission policy denies. No product logic lives here.
//! JSON-RPC framing is in [`jsonrpc`]; [`handle_line`](Server::handle_line)
//! maps one inbound line to a response, and [`serve`](Server::serve) drives it
//! over stdio.

use crate::jsonrpc::{
    decode, Decode, Incoming, Response, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use crate::paging;
use crate::prompts;
use crate::resources;
use modfoundry_agent::{
    actor_key, now_unix, AgentError, Broker, Store, StoreConfirm, DEFAULT_AGENT_ID,
};
use modfoundry_commands::registry::{core_with, Session as RegistrySession};
use modfoundry_commands::update;
use modfoundry_core::{
    is_preview, Actor, Command, EffectKind, Grants, Outcome, PREVIEW_ARG, WORKSPACE_UNDO,
};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Protocol version we advertise when the client sends none.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Stateless MCP frontend over one command registry and its agent broker.
pub struct Server {
    name: &'static str,
    version: &'static str,
    broker: Broker,
    agent: Actor,
    registry: RegistrySession,
    undo_root: Option<PathBuf>,
}

/// Read by the client at start-up; the first 512 characters stand on their own.
const INSTRUCTIONS: &str = "ModFoundry builds, checks and publishes DayZ mods through the `mf` CLI and this MCP server. \
Call host.doctor first to see the DayZ installs and this host. \
Changes need the server started with --allow-mutations or an `mf agent grant`; Destructive, Network and Publish commands always need a grant. \
host.update_check (Network) says whether a newer ModFoundry release exists; only the user installs one, with `mf update`.";

/// The fixed instructions, then the saved update notice when there is one.
fn instructions() -> String {
    match update::saved_notice() {
        Some(notice) => format!("{INSTRUCTIONS}\n\n{notice}"),
        None => INSTRUCTIONS.to_owned(),
    }
}

/// The registry an MCP session dispatches over, recording its launches under `agent`.
fn session_registry(db: Option<PathBuf>, agent: Actor) -> RegistrySession {
    let registry = RegistrySession::new().with_actor(actor_key(agent));
    match db {
        Some(db) => registry.with_db(db),
        None => registry,
    }
}

/// With a database, an agent's `diag.run` and `diag.run_diag` calls return once the server is
/// ready, unless they say otherwise; without one nobody could watch or stop such a run.
fn detached_by_default(name: &str, mut arguments: Value, recorded: bool) -> Value {
    if recorded && matches!(name, "diag.run" | "diag.run_diag") {
        if let Some(args) = arguments.as_object_mut() {
            args.entry("wait").or_insert(json!(false));
        }
    }
    arguments
}

/// How long a closing server waits for its runs to stop and remove their staging.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(30);

/// The actor an MCP session runs as: an agent whose reach is exactly what the
/// persisted policy grants it (plus the session switch).
fn session_actor(id: u64) -> Actor {
    Actor::Agent {
        id,
        grants: Grants::NONE,
    }
}

impl Server {
    /// A server whose policy and audit trail live for this process only — what
    /// a host that points the server at no database gets.
    pub fn new() -> Self {
        let registry = session_registry(None, session_actor(DEFAULT_AGENT_ID));
        Self::from_broker(
            Broker::in_memory(core_with(&registry)).expect("in-memory agent database"),
            registry,
        )
    }

    /// A server over a store the caller already opened — the shape the binary
    /// uses, so a database that will not open is diagnosed (and maybe degraded)
    /// before the server exists. `db` is the file the store came from; the
    /// registry takes project settings from it.
    pub fn with_store(store: Store, db: Option<PathBuf>) -> Self {
        let registry = session_registry(db, session_actor(DEFAULT_AGENT_ID));
        Self::from_broker(Broker::with_store(core_with(&registry), store), registry)
    }

    fn from_broker(broker: Broker, registry: RegistrySession) -> Self {
        Self {
            name: "modfoundry-mcp-server",
            version: env!("CARGO_PKG_VERSION"),
            broker,
            agent: session_actor(DEFAULT_AGENT_ID),
            registry,
            undo_root: None,
        }
    }

    /// Confine `workspace.undo` to change-sets recorded under `root`: a session
    /// started for one mod cannot reverse a change made elsewhere.
    pub fn confine_undo(mut self, root: PathBuf) -> Self {
        self.undo_root = Some(root);
        self
    }

    /// Attribute this session's calls to `id` — policy rules naming
    /// `agent:<id>` apply to it and its audit rows carry the same key.
    pub fn with_agent(mut self, id: u64) -> Self {
        self.agent = session_actor(id);
        self.registry = self.registry.clone().with_actor(actor_key(self.agent));
        self.broker.replace_core(core_with(&self.registry));
        self
    }

    /// Let this session run `Mutate` and `Launch` commands without a persisted
    /// grant (the `--allow-mutations` switch). A persisted `deny` still
    /// outranks it, and `Destructive`, `Network` and `Publish` still need a
    /// rule.
    pub fn allow_mutations(mut self) -> Self {
        self.broker.allow_session_mutations();
        self
    }

    /// Answer policy `ask` rules through the shared database at `db`, waiting
    /// at most `timeout` per rule for a frontend to answer:
    /// the cross-process path a GUI can answer through.
    pub fn with_store_confirm(mut self, db: impl AsRef<Path>, timeout: Duration) -> Self {
        self.broker
            .set_confirm(Box::new(StoreConfirm::new(db, timeout)));
        self
    }

    /// Serve one newline-delimited JSON object per line from `reader` until
    /// EOF, writing each response as its own line to `writer`.
    pub fn serve<R: BufRead, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> std::io::Result<()> {
        let served = self.serve_lines(reader, writer);
        // A run left going must not outlive the server with its staging in the install,
        // however the input ends.
        self.registry.shutdown(SHUTDOWN_WAIT);
        // A failed checkpoint leaves a valid database, so it never replaces the serve result.
        let _ = self.broker.store().checkpoint();
        served
    }

    fn serve_lines<R: BufRead, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            if let Some(response) = self.handle_line(&line) {
                writer.write_all(response.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }

    /// Handle one inbound line, returning the serialized response to emit
    /// (or `None` for notifications / empty lines).
    pub fn handle_line(&self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        let response = match decode(line) {
            Decode::Reject(response) => Some(response),
            Decode::Message(Incoming::Notification) => None,
            Decode::Message(Incoming::Request { id, method, params }) => {
                Some(self.dispatch(&method, id, params))
            }
        };
        response.map(|r| serde_json::to_string(&r).expect("response serializes"))
    }

    fn dispatch(&self, method: &str, id: Value, params: Value) -> Response {
        match method {
            "initialize" => Response::result(id, self.initialize(params)),
            "ping" => Response::result(id, json!({})),
            "tools/list" => match self.tools_list() {
                Ok(tools) => Response::result(id, tools),
                Err(e) => Response::error(id, INTERNAL_ERROR, e.to_string()),
            },
            "tools/call" => self.tools_call(id, params),
            "resources/list" => self.resources_list(id, params),
            "resources/templates/list" => self.resource_templates_list(id, params),
            "resources/read" => self.resources_read(id, params),
            "prompts/list" => self.prompts_list(id, params),
            "prompts/get" => self.prompts_get(id, params),
            _ => Response::error(id, METHOD_NOT_FOUND, "method not found"),
        }
    }

    fn initialize(&self, params: Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        json!({
            "protocolVersion": requested.unwrap_or(DEFAULT_PROTOCOL_VERSION),
            "capabilities": {
                "tools": { "listChanged": false },
                // No subscriptions and no change notifications: nothing here
                // watches the filesystem, so both are declared off rather than
                // left for a client to guess.
                "resources": { "subscribe": false, "listChanged": false },
                "prompts": { "listChanged": false },
            },
            "serverInfo": { "name": self.name, "version": self.version },
            "instructions": instructions(),
        })
    }

    /// Every registered command this session can reach, sorted by id: what it
    /// may run, plus the mutations it may only *preview* — an agent that cannot
    /// mutate still sees what a mutation would do, and would
    /// otherwise have no way to discover it.
    fn exposed(&self) -> Result<Vec<&dyn Command>, AgentError> {
        let mut commands = Vec::new();
        for command in self.broker.core().commands() {
            if self.reachable(command.spec().id)? {
                commands.push(command);
            }
        }
        commands.sort_by_key(|c| c.spec().id);
        Ok(commands)
    }

    /// Whether this session can reach `id` at all — it may run it, or it may
    /// preview a mutation that can plan. One predicate for the tool list, the
    /// resource list and the prompt list, so the three surfaces cannot disagree
    /// about what this session has.
    fn reachable(&self, id: &str) -> Result<bool, AgentError> {
        let Some(command) = self.broker.core().command(id) else {
            return Ok(false);
        };
        if self.broker.can_run(self.agent, id)? {
            return Ok(true);
        }
        Ok(command.previewable() && self.broker.can_run_as(self.agent, id, EffectKind::Read)?)
    }

    fn tools_list(&self) -> Result<Value, AgentError> {
        let mut tools = Vec::new();
        for command in self.exposed()? {
            let id = command.spec().id;
            let summary = command.spec().summary;
            // Say why a mutation this session may not run is on the list at all.
            let mut description = if self.broker.can_run(self.agent, id)? {
                summary.to_owned()
            } else {
                format!("{summary} (preview only: call it with `preview: true`)")
            };
            // The undo log supplies the change-set, so the tool takes none.
            let undo = id == WORKSPACE_UNDO;
            if undo {
                if let Some(root) = &self.undo_root {
                    description.push_str(&format!(
                        " Only changes under `{}` can be undone in this session.",
                        root.display()
                    ));
                }
            }
            let schema = if undo {
                json!({ "type": "object", "properties": {} })
            } else {
                command.args_schema()
            };
            tools.push(json!({
                "name": id,
                "description": description,
                "inputSchema": schema,
            }));
        }
        Ok(json!({ "tools": tools }))
    }

    fn tools_call(&self, id: Value, params: Value) -> Response {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Response::error(id, INVALID_PARAMS, "missing tool `name`");
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if name == WORKSPACE_UNDO {
            return self.undo_tool(id, &arguments);
        }
        match self.broker.dispatch(
            self.agent,
            name,
            detached_by_default(name, arguments, self.registry.has_db()),
        ) {
            Ok(outcome) => Response::result(id, tool_text(outcome_text(&outcome))),
            // A command this agent may not run is not a tool: the wire cannot
            // tell it from one that does not exist (the agent gets no map of
            // what it is missing). The broker has already recorded the refusal.
            Err(AgentError::Denied(_) | AgentError::NotFound(_)) => {
                Response::error(id, INVALID_PARAMS, format!("unknown tool: {name}"))
            }
            Err(e) => Response::result(id, tool_error(e.to_string())),
        }
    }

    /// `workspace.undo`: the change-set to invert is the host's to resolve from
    /// the undo log, so the tool takes no arguments from the agent — a
    /// change-set named by the caller is never trusted.
    fn undo_tool(&self, id: Value, arguments: &Value) -> Response {
        // An undo cannot itself be undone, so a dry run would be a change.
        if is_preview(arguments) {
            return Response::result(
                id,
                tool_error(
                    "workspace.undo cannot be previewed: the change-set it replays is \
                     already recorded; call it without preview to apply it"
                        .into(),
                ),
            );
        }
        let no_arguments = match arguments {
            Value::Null => true,
            Value::Object(args) => args.keys().all(|key| key == PREVIEW_ARG),
            _ => false,
        };
        if !no_arguments {
            return Response::result(
                id,
                tool_error(
                    "workspace.undo takes no arguments: it always undoes the newest \
                     recorded change-set"
                        .into(),
                ),
            );
        }
        let undone = match &self.undo_root {
            Some(root) => self.broker.undo_within(self.agent, root, now_unix()),
            None => self.broker.undo(self.agent, now_unix()),
        };
        match undone {
            Ok(Some(undone)) => Response::result(id, tool_text(outcome_text(&undone.outcome))),
            Ok(None) => {
                let text = match &self.undo_root {
                    Some(root) => format!("nothing to undo under `{}`", root.display()),
                    None => "nothing to undo".to_owned(),
                };
                Response::result(id, tool_text(text))
            }
            Err(AgentError::Denied(_) | AgentError::NotFound(_)) => Response::error(
                id,
                INVALID_PARAMS,
                format!("unknown tool: {WORKSPACE_UNDO}"),
            ),
            Err(e) => Response::result(id, tool_error(e.to_string())),
        }
    }

    /// `resources/list`: the concrete resources this session may read, sorted by
    /// URI. A resource *is* its command's verdict, so a denied command takes its
    /// resource off the list exactly as it takes its tool off `tools/list`.
    fn resources_list(&self, id: Value, params: Value) -> Response {
        let offset = match self.page_offset(&params) {
            Ok(offset) => offset,
            Err(message) => return Response::error(id, INVALID_PARAMS, message),
        };
        let listed = match self.resources() {
            Ok(listed) => listed
                .into_iter()
                .filter(|r| r["uriTemplate"].is_null())
                .collect(),
            Err(e) => return Response::error(id, INTERNAL_ERROR, e.to_string()),
        };
        Response::result(id, list_result("resources", paging::page(listed, offset)))
    }

    /// `resources/templates/list`: the parameterized resources, which are
    /// listed apart from the concrete ones (`uriTemplate`, not `uri`).
    fn resource_templates_list(&self, id: Value, params: Value) -> Response {
        let offset = match self.page_offset(&params) {
            Ok(offset) => offset,
            Err(message) => return Response::error(id, INVALID_PARAMS, message),
        };
        let listed = match self.resources() {
            Ok(listed) => listed
                .into_iter()
                .filter(|r| !r["uriTemplate"].is_null())
                .collect(),
            Err(e) => return Response::error(id, INTERNAL_ERROR, e.to_string()),
        };
        Response::result(
            id,
            list_result("resourceTemplates", paging::page(listed, offset)),
        )
    }

    /// `resources/read`: dispatch the command the URI names and return its
    /// payload as text content.
    ///
    /// A resource that a body cannot produce — the file is gone, the path is
    /// not a file — is the spec's canonical *resource not found* (`-32002` with
    /// the URI in `data`), not an internal error: it is the resource that does
    /// not resolve, and a client asking for a file that is not there is not a
    /// server fault. Only a broker/store failure stays `-32603`.
    fn resources_read(&self, id: Value, params: Value) -> Response {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return Response::error(id, INVALID_PARAMS, "missing resource `uri`");
        };
        // An unknown URI, an unreachable command and a refusal all answer the
        // same thing, so an agent gets no map of what it is missing — but a
        // *body* that cannot produce the resource says why.
        let missing =
            |reason: Option<String>| Response::resource_not_found(id.clone(), uri, reason);
        let Some((resource, arguments)) = resources::resolve(uri) else {
            return missing(None);
        };
        // A resource must be *runnable*, not merely previewable: a preview
        // answers no resource.
        match self.broker.can_run(self.agent, resource.command) {
            Ok(true) => {}
            Ok(false) => return missing(None),
            Err(e) => return Response::error(id, INTERNAL_ERROR, e.to_string()),
        }
        match self
            .broker
            .dispatch(self.agent, resource.command, arguments)
        {
            Ok(outcome) => Response::result(
                id,
                json!({ "contents": [ {
                    "uri": uri,
                    "mimeType": resource.mime_type,
                    "text": outcome_text(&outcome),
                } ] }),
            ),
            Err(AgentError::Denied(_) | AgentError::NotFound(_)) => missing(None),
            Err(AgentError::Command(e)) => missing(Some(e.to_string())),
            Err(e) => Response::error(id, INTERNAL_ERROR, e.to_string()),
        }
    }

    /// `prompts/list`: the workflow templates this session could act on.
    fn prompts_list(&self, id: Value, params: Value) -> Response {
        let offset = match self.page_offset(&params) {
            Ok(offset) => offset,
            Err(message) => return Response::error(id, INVALID_PARAMS, message),
        };
        let mut listed = Vec::new();
        for prompt in prompts::PROMPTS {
            match self.reachable(prompt.command) {
                Ok(true) => listed.push(prompt.listing()),
                Ok(false) => {}
                Err(e) => return Response::error(id, INTERNAL_ERROR, e.to_string()),
            }
        }
        Response::result(id, list_result("prompts", paging::page(listed, offset)))
    }

    /// `prompts/get`: render one template with the caller's arguments.
    fn prompts_get(&self, id: Value, params: Value) -> Response {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Response::error(id, INVALID_PARAMS, "missing prompt `name`");
        };
        let unknown = || {
            Response::error(
                id.clone(),
                INVALID_PARAMS,
                format!("unknown prompt: {name}"),
            )
        };
        let Some(prompt) = prompts::PROMPTS.iter().find(|p| p.name == name) else {
            return unknown();
        };
        match self.reachable(prompt.command) {
            Ok(true) => {}
            Ok(false) => return unknown(),
            Err(e) => return Response::error(id, INTERNAL_ERROR, e.to_string()),
        }
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match prompt.render(&arguments) {
            Ok(rendered) => Response::result(id, rendered),
            Err(message) => Response::error(id, INVALID_PARAMS, message),
        }
    }

    /// The listed resources, gated by this session's reach: concrete URIs carry
    /// `uri`, templates carry `uriTemplate` instead. The gate is *runnable* —
    /// the same one `resources/read` applies — so a resource can never be listed
    /// that a read would refuse.
    fn resources(&self) -> Result<Vec<Value>, AgentError> {
        let mut listed = Vec::new();
        for resource in resources::RESOURCES {
            if !self.broker.can_run(self.agent, resource.command)? {
                continue;
            }
            let mut entry = json!({
                "name": resource.name,
                "description": resource.description,
                "mimeType": resource.mime_type,
            });
            match resource.template() {
                Some(template) => entry["uriTemplate"] = json!(template),
                None => entry["uri"] = json!(resource.uri),
            }
            listed.push(entry);
        }
        Ok(listed)
    }

    /// The page offset a list request names, or the message for the
    /// `-32602` it deserves.
    fn page_offset(&self, params: &Value) -> Result<usize, String> {
        paging::offset(params.get("cursor").and_then(Value::as_str))
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a command outcome as the text an agent reads: raw text for string
/// payloads (file contents, derapified configs), pretty JSON otherwise.
fn outcome_text(outcome: &Outcome) -> String {
    match outcome.to_json() {
        Value::String(text) => text,
        value => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
    }
}

/// A tool result carrying only text content.
fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": false })
}

/// A list result: the page under `key`, and `nextCursor` only when entries
/// follow — the field is optional, so absent is not the same as `null`.
fn list_result(key: &str, page: paging::Page<Value>) -> Value {
    let mut result = json!({ key: page.items });
    if let Some(cursor) = page.next_cursor {
        result["nextCursor"] = json!(cursor);
    }
    result
}

/// A tool result reporting a failed operation to the agent.
fn tool_error(message: String) -> Value {
    json!({ "content": [ { "type": "text", "text": message } ], "isError": true })
}
