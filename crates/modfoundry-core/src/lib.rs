//! ModFoundry domain model and Core Domain API.
//!
//! This crate defines the *shapes* both sides of every ModFoundry operation agree on:
//! typed identifiers, workspace records, the effect classes a command can have, and the
//! permission vocabulary used to gate agent/automation access.
//!
//! It intentionally contains no filesystem, parser, or UI code: those live in sibling
//! crates and are reached through the command registry.

pub mod command;
pub mod diagnostic;
pub mod ids;
pub mod text;
pub mod workspace;

pub use command::{
    is_preview, Actor, Command, CommandSpec, Core, Grants, Outcome, HOST_UPDATE, PREVIEW_ARG,
    WORKSPACE_UNDO,
};
pub use diagnostic::{Diagnostic, Severity, Span};

use serde::{Deserialize, Serialize};

/// Broad effect class of a command. Used by the permission policy (agents/plugins)
/// and by audit logging. `Mutate` and above require explicit authorization for
/// non-interactive callers (an agent grant, or the MCP server's `--allow-mutations`); `Destructive`, `Network` and
/// `Publish` additionally require an explicit rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    /// Pure reads and queries (symbols, ASTs, indexes, logs). Always allowed.
    Read,
    /// Mutates derived, rebuildable state only (indexes, caches, DB).
    IndexWrite,
    /// Mutates user content (files in a workspace) in a reversible, change-set tracked way.
    Mutate,
    /// Irreversible-in-practice content destruction (delete trees, overwrite outputs).
    Destructive,
    /// Launches external processes (game, server, diagnostics, Wine tooling).
    Launch,
    /// Network access (workshop metadata, Steam API calls).
    Network,
    /// Publishes content externally (Workshop upload). Highest trust class.
    Publish,
}

impl EffectKind {
    /// Whether a command with this effect may run without an explicit grant.
    pub fn allowed_by_default(self) -> bool {
        matches!(self, EffectKind::Read | EffectKind::IndexWrite)
    }
}

/// Outcome summary of a mutating command, returned to every frontend alike so that
/// GUI toasts, CLI output, MCP tool results and agent logs present the same facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub kind: EffectKind,
    /// Number of host files created/overwritten/deleted (when file-backed).
    pub files_changed: usize,
    /// Human-readable one-line description (localized later, if ever).
    pub description: String,
}

/// Errors surfaced by the Core Domain API. Frontends map these onto their own
/// presentation; they must never need to know crate-internal error types.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("command not found: {0}")]
    NotFound(String),
    #[error("command rejected by permission policy: {0}")]
    PermissionDenied(String),
    #[error("command arguments invalid: {0}")]
    InvalidArguments(String),
    #[error("no workspace is open")]
    NoWorkspace,
    #[error("operation failed: {0}")]
    Failed(String),
}
