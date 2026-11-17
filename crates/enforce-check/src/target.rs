use serde::{Deserialize, Serialize};

use crate::diag::Severity;
use crate::input::{Layer, ModInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    RetailServer,
    DiagServer,
    DiagClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleKind {
    Error,
    Warning,
    Soft,
}

// Map names the installed DLCs add; the measured boot had both.
const MAP_NAMES: [&str; 2] = ["sakhal", "nasdara"];

const FEATURES: [&str; 3] = [
    "FEATURE_CURSOR",
    "FEATURE_NETWORK_RECONCILIATION",
    "FEATURE_NETWORK_RECONCILATION",
];

const RETAIL_SERVER: [&str; 7] = [
    "DAYZ_1_30",
    "SERVER_FOR_WINDOWS",
    "SERVER",
    "PLATFORM_LINUX",
    "RELEASE",
    "NO_GUI",
    "NO_GUI_INGAME",
];

const DIAG_SERVER: [&str; 11] = [
    "DAYZ_1_30",
    "SERVER_FOR_WINDOWS",
    "SERVER",
    "PLATFORM_WINDOWS",
    "DIAG",
    "DIAG_DEVELOPER",
    "NO_GUI",
    "NO_GUI_INGAME",
    "ENABLE_LOGGING",
    "ROBOCLIENT",
    "INPUT_OVERRIDE",
];

const DIAG_CLIENT: [&str; 7] = [
    "DAYZ_1_30",
    "PLATFORM_WINDOWS",
    "DIAG",
    "DIAG_DEVELOPER",
    "ENABLE_LOGGING",
    "ROBOCLIENT",
    "INPUT_OVERRIDE",
];

pub fn defines(target: Target, mods: &[ModInfo], extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let (base, tail): (&[&str], &[&str]) = match target {
        Target::RetailServer => (&RETAIL_SERVER, &[]),
        Target::DiagServer => (&DIAG_SERVER, &["LOG_TO_FILE", "LOG_TO_SCRIPT"]),
        Target::DiagClient => (&DIAG_CLIENT, &["LOG_TO_FILE", "LOG_TO_SCRIPT"]),
    };
    out.extend(base.iter().map(|name| (*name).to_string()));
    out.extend(MAP_NAMES.iter().map(|name| (*name).to_string()));
    out.extend(mods.iter().map(|m| m.cfg_class.clone()));
    out.extend(FEATURES.iter().map(|name| (*name).to_string()));
    out.extend(tail.iter().map(|name| (*name).to_string()));
    out.extend(extra.iter().cloned());
    out
}

pub fn layer_defines(target: Target, layer: Layer) -> &'static [&'static str] {
    match (layer, target) {
        (Layer::Mission, Target::RetailServer) => &["ENABLE_CAROUSEL"],
        (Layer::Mission, _) => &["ENABLE_CAROUSEL", "BROADCAST_BUNKER_SP_VIS_ENABLED"],
        _ => &[],
    }
}

pub fn adjust(target: Target, kind: RuleKind) -> (Severity, bool) {
    let retail = target == Target::RetailServer;
    match kind {
        RuleKind::Error => (Severity::Error, false),
        RuleKind::Warning => (Severity::Warning, false),
        RuleKind::Soft if retail => (Severity::Warning, true),
        RuleKind::Soft => (Severity::Error, false),
    }
}
