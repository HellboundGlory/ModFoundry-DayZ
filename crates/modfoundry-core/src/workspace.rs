//! Workspace and mod project record types (persisted as JSON next to the workspace,
//! mirrored into the SQLite project DB).

use crate::ids::{BuildProfileId, ModId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A ModFoundry workspace: the top-level container the user opens in the GUI,
/// points the CLI at, or exposes to an agent via MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Host path of the workspace root directory.
    pub root: PathBuf,
    pub mods: Vec<Mod>,
    #[serde(default)]
    pub settings: WorkspaceSettings,
}

/// Everything a mod project needs to be *identified*. `path` is arbitrary: an imported
/// tree keeps its own layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    pub id: ModId,
    /// Filesystem-safe slug, unique inside the workspace (also the `@`-less mod name).
    pub slug: String,
    pub title: String,
    /// VFS prefix this mod claims, e.g. `MyMod/Weapons` (PBO `prefix` property).
    /// Two mods in one workspace must not claim overlapping prefixes.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Absolute or workspace-relative path to the mod root directory.
    pub path: PathBuf,
    #[serde(default)]
    pub dependencies: Vec<ModDependency>,
}

/// A dependency on another mod, either local (workspace mod) or remote
/// (Steam Workshop item id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModDependency {
    /// Workshop published file id when the dependency is a Workshop mod.
    pub workshop_id: Option<u64>,
    /// Workspace mod slug when the dependency is local.
    pub local_slug: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    /// Default build profile used by `build` commands when none is named.
    pub default_build_profile: Option<BuildProfileId>,
    /// Paths (host) of the DayZ game/`DayZ Tools` installs when auto-detection
    /// should be overridden.
    pub game_path: Option<PathBuf>,
    pub tools_path: Option<PathBuf>,
    /// Ordered extra VFS layers (workshop mods etc.) — see modfoundry-vfs.
    #[serde(default)]
    pub extra_layers: Vec<String>,
}
