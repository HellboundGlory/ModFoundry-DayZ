//! Runtime diagnostics + launch profiles.
//!
//! The build→launch→capture→navigate loop: launch the native DayZ server (or a
//! client under Proton) via a [`profile`], capture the RPT + script.log it writes,
//! then [`parse`] them into structured [`LogRecord`]s and [`map`] error locations
//! back to the mod sources that produced them.
//!
//! Parser + profile models are pure (no IO) so they run in tests/CI/WASM; the only
//! IO sits at the boundary (process spawn + file capture in `run`).

pub mod fingerprint;
pub mod map;
pub mod parse;
pub mod profile;
pub mod run;
pub mod tail;
pub mod verdict;

pub use fingerprint::{error_fingerprint, fingerprint, is_generic, short_id};
pub use map::{
    count_storage_errors, dedup_errors, map_logs, mark_baseline, storage_flood_hint, KnownRef,
    MappedError, Resolver,
};
pub use parse::{parse_log, Channel, Level, LogRecord};
pub use profile::{ClientProfile, Connect, LaunchKind, LogFlags, ServerProfile};
pub use run::{
    capture_client, capture_command, capture_run, find_logs, log_states, run_log_text,
    wait_for_log_marker, ClientRunReport, ClientWait, LaunchCommand, LogState, LogStates,
    MarkerWait, RunReport, StopSignal,
};
pub use tail::{LogTail, TailedRecord};
pub use verdict::{judge, LinePattern, Verdict, VerdictRules};
