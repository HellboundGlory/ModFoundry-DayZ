//! DayZ config.cpp *model*: turn the generic parse tree into the concrete
//! shapes a mod's config declares — CfgPatches (addon names + requiredAddons)
//! and CfgMods (mod class, its script modules bound under `defs`).
//!
//! This is where "the file is a DayZ mod config" becomes structured data the
//! validator and other consumers (VFS roots, Enforce module resolution) read.
//! It stays IO-free: all paths here are the strings declared in the file. Path
//! *existence* checking is the validator's job (it needs a source root).

use crate::ast::{classes, ConfigClass, ConfigFile, Value};
use modfoundry_core::Span;

/// Which script-module slot a binding fills. The engine binds modules by these
/// fixed names; DayZ has five (1_Core..5_Mission) plus editor/plugin modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleSlot {
    Engine,
    GameLib,
    Game,
    World,
    Mission,
}

impl ModuleSlot {
    /// Parse the `defs` member name like `gameScriptModule` → [`ModuleSlot::Game`],
    /// ignoring case. Returns `None` for names that are not a recognized script-module slot.
    pub fn from_defs_name(name: &str) -> Option<ModuleSlot> {
        const SLOTS: [(&str, ModuleSlot); 5] = [
            ("engineScriptModule", ModuleSlot::Engine),
            ("gameLibScriptModule", ModuleSlot::GameLib),
            ("gameScriptModule", ModuleSlot::Game),
            ("worldScriptModule", ModuleSlot::World),
            ("missionScriptModule", ModuleSlot::Mission),
        ];
        SLOTS
            .iter()
            .find(|(defs, _)| defs.eq_ignore_ascii_case(name))
            .map(|(_, slot)| *slot)
    }

    /// The canonical `N_Name` scripts directory for this slot.
    pub fn canonical_dir(self) -> &'static str {
        match self {
            ModuleSlot::Engine => "1_Core",
            ModuleSlot::GameLib => "2_GameLib",
            ModuleSlot::Game => "3_Game",
            ModuleSlot::World => "4_World",
            ModuleSlot::Mission => "5_Mission",
        }
    }
}

/// One declared script module: `defs.<slot>ScriptModule { value; files[]; }`.
#[derive(Debug, Clone)]
pub struct ScriptModule {
    pub slot: ModuleSlot,
    /// The `defs` member name (e.g. `gameScriptModule`).
    pub defs_name: String,
    /// The `defs` child class's name span.
    pub span: Span,
    /// The module entry-point override (`value = "CF_CreateGame";`), if any.
    /// Empty string means default entry point.
    pub value: Option<String>,
    /// `files[]` — module source directories as declared (virtual paths, e.g.
    /// `"MyMod/Scripts/3_Game"`).
    pub files: Vec<String>,
    /// One span per `files[]` item, in declaration order.
    pub file_spans: Vec<Span>,
}

/// A single mod declared under `CfgMods`, with its `defs` script modules.
#[derive(Debug, Clone)]
pub struct ModConfig {
    /// Class name under CfgMods (the mod identifier).
    pub name: String,
    /// The class's name span.
    pub span: Span,
    /// `dir = "JM/CF"` — the mod's content prefix root.
    pub dir: Option<String>,
    /// `type` (usually `"mod"`).
    pub kind: Option<String>,
    /// `dependencies[]` — required module layers (e.g. `Core`, `Game`).
    pub dependencies: Vec<String>,
    /// Script modules bound under `defs`.
    pub script_modules: Vec<ScriptModule>,
}

/// The DayZ-shaped view of a parsed mod config.
#[derive(Debug, Clone)]
pub struct DayZConfig {
    /// Addon names under `CfgPatches` → their `requiredAddons[]`.
    pub patches: Vec<PatchesEntry>,
    /// Mods declared under `CfgMods`.
    pub mods: Vec<ModConfig>,
}

#[derive(Debug, Clone)]
pub struct PatchesEntry {
    pub name: String,
    /// The class's name span.
    pub span: Span,
    /// requiredAddons[] — addons that must load first (engine-ordering).
    pub required_addons: Vec<String>,
    /// requiredVersion if present.
    pub required_version: Option<f64>,
}

impl DayZConfig {
    /// Derive the DayZ view from a parsed config file.
    pub fn from_file(file: &ConfigFile) -> DayZConfig {
        let patches = file
            .find_class("CfgPatches")
            .map(parse_patches)
            .unwrap_or_default();
        let mods = file
            .find_class("CfgMods")
            .map(parse_cfgmods)
            .unwrap_or_default();
        DayZConfig { patches, mods }
    }
}

fn parse_patches(cfg: &ConfigClass) -> Vec<PatchesEntry> {
    let mut out = Vec::new();
    for class in classes(&cfg.members) {
        let required_addons = crate::ast::required_addon_properties(class)
            .filter_map(|p| p.strings())
            .flatten()
            .map(|s| s.to_string())
            .collect();
        let required_version = class
            .property("requiredVersion")
            .and_then(|p| p.value.as_ref())
            .and_then(|v| match v {
                Value::Number(n) => n.parse::<f64>().ok(),
                _ => None,
            });
        out.push(PatchesEntry {
            name: class.name.clone(),
            span: class.spans.name,
            required_addons,
            required_version,
        });
    }
    out
}

fn parse_cfgmods(cfg: &ConfigClass) -> Vec<ModConfig> {
    let mut out = Vec::new();
    for class in classes(&cfg.members) {
        out.push(parse_mod(class));
    }
    out
}

fn parse_mod(class: &ConfigClass) -> ModConfig {
    let dir = str_prop(class, "dir");
    let kind = str_prop(class, "kind").or_else(|| str_prop(class, "type"));
    let dependencies = str_array(class.property("dependencies"))
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let script_modules = class
        .child_class("defs")
        .map(parse_defs)
        .unwrap_or_default();
    ModConfig {
        name: class.name.clone(),
        span: class.spans.name,
        dir,
        kind,
        dependencies,
        script_modules,
    }
}

fn parse_defs(defs: &ConfigClass) -> Vec<ScriptModule> {
    let mut out = Vec::new();
    for class in classes(&defs.members) {
        let Some(slot) = ModuleSlot::from_defs_name(&class.name) else {
            continue; // non-script-module def (skeletonDefinitions etc.)
        };
        let value = str_prop(class, "value");
        let (files, file_spans) = files_with_spans(class.property("files"));
        out.push(ScriptModule {
            slot,
            defs_name: class.name.clone(),
            span: class.spans.name,
            value,
            files,
            file_spans,
        });
    }
    out
}

/// `files[]` values paired with the span of each item, so a diagnostic can
/// point at exactly the entry it rejects even when a non-string item intervenes.
fn files_with_spans(prop: Option<&crate::ast::ConfigProperty>) -> (Vec<String>, Vec<Span>) {
    let Some(prop) = prop else {
        return (Vec::new(), Vec::new());
    };
    let mut files = Vec::new();
    let mut file_spans = Vec::new();
    for (text, span) in prop.spanned_strings() {
        files.push(text.to_owned());
        file_spans.push(span);
    }
    (files, file_spans)
}

fn str_prop(class: &ConfigClass, name: &str) -> Option<String> {
    class
        .property(name)
        .and_then(|p| p.value.as_ref())
        .and_then(|v| match v {
            Value::Str(s) => Some(s.clone()),
            Value::Ident(s) => Some(s.clone()),
            _ => None,
        })
}

fn str_array(prop: Option<&crate::ast::ConfigProperty>) -> Option<Vec<&str>> {
    prop?.strings()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn required_addons_include_appended_arrays() {
        let r = parse(
            r#"class CfgPatches { class M { requiredAddons[] = { "A" }; requiredAddons[] += { "B" }; }; };"#,
        );
        assert!(r.issues.is_empty(), "{:?}", r.issues);
        let dz = DayZConfig::from_file(&r.file);
        assert_eq!(dz.patches[0].required_addons, ["A", "B"]);
    }

    #[test]
    fn extracts_patches_and_mods() {
        let src = r#"class CfgPatches {
            class MyMod {
                units[] = {};
                requiredVersion = 0.1;
                requiredAddons[] = { "DZ_Data", "DZ_Scripts" };
            };
        };
        class CfgMods {
            class MyMod {
                dir = "MyMod";
                type = "mod";
                dependencies[] = { "Game", "World" };
                class defs {
                    class gameScriptModule {
                        value = "CreateGame";
                        files[] = { "MyMod/Scripts/3_Game" };
                    };
                    class worldScriptModule {
                        files[] = { "MyMod/Scripts/4_World" };
                    };
                };
            };
        };"#;
        let r = parse(src);
        assert!(r.issues.is_empty(), "{:?}", r.issues);
        let dz = DayZConfig::from_file(&r.file);
        assert_eq!(dz.patches.len(), 1);
        assert_eq!(dz.patches[0].required_addons, ["DZ_Data", "DZ_Scripts"]);
        assert_eq!(dz.patches[0].required_version, Some(0.1));
        assert_eq!(dz.mods.len(), 1);
        let m = &dz.mods[0];
        assert_eq!(m.name, "MyMod");
        assert_eq!(m.dir.as_deref(), Some("MyMod"));
        assert_eq!(m.kind.as_deref(), Some("mod"));
        assert_eq!(m.dependencies, ["Game", "World"]);
        assert_eq!(m.script_modules.len(), 2);
        assert_eq!(m.script_modules[0].slot, ModuleSlot::Game);
        assert_eq!(m.script_modules[0].files, ["MyMod/Scripts/3_Game"]);
        assert_eq!(m.script_modules[1].slot, ModuleSlot::World);
    }

    #[test]
    fn tolerates_missing_optional_sections() {
        let r = parse("class CfgMods { class X { }; };");
        let dz = DayZConfig::from_file(&r.file);
        assert_eq!(dz.patches.len(), 0);
        assert_eq!(dz.mods.len(), 1);
        assert_eq!(dz.mods[0].dir, None);
        assert!(dz.mods[0].script_modules.is_empty());
    }

    #[test]
    fn slot_mapping() {
        assert_eq!(
            ModuleSlot::from_defs_name("gameScriptModule"),
            Some(ModuleSlot::Game)
        );
        assert_eq!(
            ModuleSlot::from_defs_name("engineScriptModule"),
            Some(ModuleSlot::Engine)
        );
        assert_eq!(ModuleSlot::from_defs_name("notAModule"), None);
        assert_eq!(ModuleSlot::Game.canonical_dir(), "3_Game");
    }
}
