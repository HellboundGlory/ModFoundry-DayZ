//! Build report + staged-tree manifest written alongside the `@Mod` output. Files are the source of truth; this JSON records what
//! a deterministic run produced and the inputs that pinned its bytes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Serializable summary of one `mf build` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildReport {
    /// Mod name (`@<name>`), PBO stem, and default prefix.
    pub name: String,
    /// Absolute path of the staged `@<name>` directory.
    pub output_dir: PathBuf,
    /// PBO files staged under `addons/`, relative to the mod dir.
    pub addons: Vec<String>,
    /// Whether each addon was signed (a sibling `.bisign` produced).
    pub signed: bool,
    /// Fixed timestamp used for every PBO entry (determinism input).
    pub timestamp: u32,
    /// SHA-256 of each staged PBO (byte-stability evidence).
    pub pbo_sha256: Vec<String>,
    /// Human note appended to the report (e.g. engine-load caveats).
    pub note: Option<String>,
    /// Content warnings the build's file listing gave (`build.contents`).
    #[serde(default)]
    pub warnings: Vec<String>,
}
