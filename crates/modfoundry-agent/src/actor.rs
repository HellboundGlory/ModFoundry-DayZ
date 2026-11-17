//! Identity of the caller, as the policy store and audit trail persist it.
//!
//! `modfoundry-core`'s [`Actor`] is the in-process vocabulary; the store takes
//! plain strings, so the broker owns the mapping between them. One spelling per
//! actor keeps `mf agent grant <actor> …` and an audit row's `actor` column
//! talking about the same thing — and [`validate_actor_key`] keeps a typo from
//! becoming a rule that can never match.

use modfoundry_core::{Actor, Grants};
use modfoundry_store::policy::ANY;
use modfoundry_store::StoreError;

/// The id an MCP session acts as when none is given, so every such session
/// shares it.
pub const DEFAULT_AGENT_ID: u64 = 0;

/// The canonical persisted name of `actor` (`user`, `cli`, `automation`,
/// `agent:<id>`).
pub fn actor_key(actor: Actor) -> String {
    match actor {
        Actor::User => "user".to_owned(),
        Actor::CliSession => "cli".to_owned(),
        Actor::Automation { .. } => "automation".to_owned(),
        Actor::Agent { id, .. } => format!("agent:{id}"),
    }
}

/// Whether `actor` is the key every MCP session without an id shares.
pub fn is_shared_agent(actor: &str) -> bool {
    actor
        == actor_key(Actor::Agent {
            id: DEFAULT_AGENT_ID,
            grants: Grants::NONE,
        })
}

/// Refuse an actor selector that could never match: the canonical keys and the
/// `*` wildcard are the whole vocabulary.
pub fn validate_actor_key(raw: &str) -> Result<(), StoreError> {
    let known = matches!(raw, ANY | "user" | "cli" | "automation")
        || raw
            .strip_prefix("agent:")
            .is_some_and(|id| id.parse::<u64>().is_ok());
    if known {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "unknown actor `{raw}`: use `user`, `cli`, `automation`, `agent:<id>` or `*`"
    )))
}
