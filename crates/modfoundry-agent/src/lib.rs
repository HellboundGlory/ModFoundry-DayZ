//! The dispatch broker: policy authorization and the audit trail above the
//! command registry.
//!
//! `modfoundry-core` owns the registry and the effect gate, and stays IO-free;
//! `modfoundry-store` owns the durable policy/audit tables, and knows nothing
//! about commands. This crate is the single place the two meet: it resolves a
//! call against the persisted policy, dispatches through the registry, and
//! records the outcome — including refusals.

pub mod actor;
pub mod broker;
pub mod confirm;
pub mod paths;

pub use actor::{actor_key, is_shared_agent, validate_actor_key, DEFAULT_AGENT_ID};
pub use broker::{check_grant, now_unix, AgentError, Broker, Undone};
pub use confirm::{Confirm, ConfirmRequest, StoreConfirm};
pub use paths::{open_db, resolve_db, session_only_warning, Database, OpenedDb};
// The store vocabulary the broker's API is expressed in, re-exported so a
// frontend depends on the broker alone rather than reaching past it.
pub use modfoundry_store::audit::{AuditEntry, AuditOutcome};
pub use modfoundry_store::confirm::ConfirmAnswer;
pub use modfoundry_store::policy::{Decision, PolicyRule};
pub use modfoundry_store::{effect_from_str, effect_str, Store, StoreError};
