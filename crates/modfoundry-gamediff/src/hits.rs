//! The places in a mod that a change between two game builds may break.

use modfoundry_config::ast::ConfigFile;
use modfoundry_script::outline::FileOutline;
use serde::{Deserialize, Serialize};

use crate::script::Place;

/// How sure a hit is to stop the mod working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Breaks,
    Check,
}

/// What the mod does with the vanilla item that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HitKind {
    ModdedClassRemoved,
    BaseClassRemoved,
    BaseClassChanged,
    OverrideRemoved,
    OverrideSignature,
    OverrideDefaults,
    OverrideBody,
    CallRemoved,
    CallSignature,
    ConfigParentRemoved,
    ConfigParentChanged,
    ConfigPatchRemoved,
    ConfigPatchChanged,
    LayoutRemoved,
    LayoutChanged,
}

impl HitKind {
    /// The level every hit of this kind gets.
    pub fn level(self) -> Level {
        match self {
            HitKind::ModdedClassRemoved
            | HitKind::BaseClassRemoved
            | HitKind::OverrideRemoved
            | HitKind::OverrideSignature
            | HitKind::ConfigParentRemoved
            | HitKind::LayoutRemoved => Level::Breaks,
            HitKind::BaseClassChanged
            | HitKind::OverrideDefaults
            | HitKind::OverrideBody
            | HitKind::CallRemoved
            | HitKind::CallSignature
            | HitKind::ConfigParentChanged
            | HitKind::ConfigPatchRemoved
            | HitKind::ConfigPatchChanged
            | HitKind::LayoutChanged => Level::Check,
        }
    }
}

/// One place in a mod that uses a vanilla item the new build removed or changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub level: Level,
    pub kind: HitKind,
    /// Matched by name alone, so the mod may mean something else.
    pub maybe: bool,
    #[serde(rename = "mod")]
    pub mod_name: String,
    pub place: Place,
    /// The vanilla item, such as `PlayerBase`, `PlayerBase.OnConnect()`, `CfgVehicles/Car` or a layout path.
    pub vanilla: String,
    pub detail: String,
    pub diag_only: bool,
}

impl Hit {
    /// A hit at the level its kind gives, neither a guess nor Diag-only.
    pub fn new(
        kind: HitKind,
        mod_name: &str,
        place: Place,
        vanilla: String,
        detail: String,
    ) -> Hit {
        Hit {
            level: kind.level(),
            kind,
            maybe: false,
            mod_name: mod_name.to_owned(),
            place,
            vanilla,
            detail,
            diag_only: false,
        }
    }
}

/// Order hits by file, line, then kind.
pub fn sort_by_place(hits: &mut [Hit]) {
    hits.sort_by(|a, b| {
        (&a.place.file, a.place.line, a.kind).cmp(&(&b.place.file, b.place.line, b.kind))
    });
}

/// One mod's sources, as read from its folder.
#[derive(Debug, Clone, Default)]
pub struct ModInput {
    pub name: String,
    /// `(path, source)` pairs, paths relative to the mod folder.
    pub scripts: Vec<(String, String)>,
    pub configs: Vec<ModConfig>,
}

impl ModInput {
    /// Each script's outline, in script order.
    pub fn outlines(&self) -> Vec<FileOutline> {
        self.scripts
            .iter()
            .map(|(path, source)| modfoundry_script::outline(path, source))
            .collect()
    }
}

/// One config file of a mod.
#[derive(Debug, Clone)]
pub struct ModConfig {
    pub path: String,
    pub file: ConfigFile,
    /// The `config.cpp` text, for line numbers; `None` for a `config.bin`.
    pub text: Option<String>,
}
