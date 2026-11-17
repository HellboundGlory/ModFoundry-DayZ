//! ModFoundry Virtual Filesystem.
//!
//! Pure-userspace layered filesystem abstraction over DayZ's *virtual* paths:
//!
//! ```text
//! VirtualPath  = normalized, case-preserving, '/' separated, anchored at a root
//! Roots        = `game:`  (vanilla Addons + dta), `workshop:<itemid>:`,
//!                `mod:<slug>:` (workspace source), `stage:` (build output),
//!                `mission:<name>:`, `virtual:` (synthetic)
//! Overlays     = ordered layer stacks per root; resolution records provenance
//! Engine form  = legacy `prefix\file` (`DZ\weapons\...` from PBO prefix properties)
//!                converted losslessly to/from VirtualPath by the `pbo` module's
//!                prefix reader and the engine adapter.
//! ```
//!
//! No symlinks, no mounts, no root privileges anywhere in the default design.
//! Mutations go through [`write`]: a [`WriteTxn`] per operation, atomic
//! temp+rename per file, case/escape preflight, and a `ChangeSet` that inverts
//! what it did (see that module for the undo-freshness and secret-retention
//! rules).

pub mod layer;
pub mod path;
pub mod pbo;
pub mod vfs;
pub mod write;

pub use layer::{
    Backend, DirectoryBackend, LayerId, LayerKind, ListedEntry, Overlay, Provenance, Resolved,
};
pub use path::VirtualPath;
pub use pbo::PboBackend;
pub use vfs::Vfs;
pub use write::{root_name, sha256_hex, ChangeOp, ChangeSet, FileChange, WriteTxn};

/// Crate-level error type for VFS operations.
#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("virtual path invalid: {0}")]
    InvalidPath(String),
    #[error("root not mounted: {0}")]
    UnknownRoot(String),
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("unsupported protected container (not a PBO): {0}")]
    UnsupportedContainer(std::path::PathBuf),
    #[error("container read failed: {0}")]
    Container(String),
    #[error("layer provides no readable bytes: {0}")]
    Unreadable(String),
    #[error("path is outside the writable root: {0}")]
    OutsideRoot(String),
    #[error("case-insensitive collision on a case-sensitive host: {0}")]
    CaseConflict(String),
    #[error("change-set carries no content to restore: {0}")]
    Incomplete(String),
    #[error("stale content, refusing the change: {0}")]
    Stale(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
