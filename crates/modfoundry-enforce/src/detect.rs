//! Pure path classification for Enforce Script sources.
//!
//! DayZ compiles scripts from the canonical module dirs `Scripts/{1..5}_*`
//! (engine load order) and `Scripts/editor`; a `.c` elsewhere compiles only
//! when config.cpp `CfgMods` `files[]` lists its folder: detect
//! by module path, never bare `.c`.

/// Canonical script module dir names, engine compile/load order.
pub const SCRIPT_MODULE_DIRS: [&str; 5] = ["1_Core", "2_GameLib", "3_Game", "4_World", "5_Mission"];

/// How a path relates to Enforce Script and the loadable module layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    /// Not an Enforce source (extension is not `.c`).
    NotEnforce,
    /// A `.c` inside a canonical `Scripts/N_*` module dir → compiles.
    InModule,
    /// A `.c` not under any standard module dir → compiles only when
    /// config.cpp `CfgMods` `files[]` lists its folder.
    Unbound,
}

/// Classify `path` by extension and module placement. Pure: no filesystem IO.
pub fn classify(path: &std::path::Path) -> FileClass {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("c"))
    {
        return FileClass::NotEnforce;
    }
    if is_in_module(path) {
        FileClass::InModule
    } else {
        FileClass::Unbound
    }
}

/// True when a component-pair `Scripts` immediately followed by a canonical
/// `N_*` module dir or `editor` appears anywhere in `path` (tolerates prefix
/// nesting such as `JM/CF/Scripts/3_Game/...`). Names compared case-insensitively.
fn is_in_module(path: &std::path::Path) -> bool {
    let mut after_scripts = false;
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
            continue;
        };
        if after_scripts
            && (name.eq_ignore_ascii_case("editor")
                || SCRIPT_MODULE_DIRS
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(name)))
        {
            return true;
        }
        after_scripts = name.eq_ignore_ascii_case("scripts");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cls(p: &str) -> FileClass {
        classify(Path::new(p))
    }

    #[test]
    fn non_c_extension_is_not_enforce() {
        assert_eq!(cls("Scripts/3_Game/thing.cpp"), FileClass::NotEnforce);
        assert_eq!(cls("config.cpp"), FileClass::NotEnforce);
        assert_eq!(cls("Scripts/3_Game/readme.txt"), FileClass::NotEnforce);
    }

    #[test]
    fn c_inside_each_canonical_module_is_in_module() {
        for m in SCRIPT_MODULE_DIRS {
            assert_eq!(
                cls(&format!("Scripts/{m}/MyMod_Thing.c")),
                FileClass::InModule
            );
        }
    }

    #[test]
    fn prefix_nested_module_path_is_in_module() {
        assert_eq!(
            cls("JM/CF/Scripts/3_Game/Scripts/Entity/MyThing.c"),
            FileClass::InModule
        );
    }

    #[test]
    fn module_match_is_case_insensitive() {
        assert_eq!(cls("scripts/3_game/a.c"), FileClass::InModule);
        assert_eq!(cls("Scripts/3_Game/Thing.C"), FileClass::InModule);
    }

    #[test]
    fn scripts_editor_is_a_module_folder() {
        assert_eq!(cls("Scripts/editor/a.c"), FileClass::InModule);
        assert_eq!(cls("JM/CF/scripts/Editor/b.c"), FileClass::InModule);
    }

    #[test]
    fn loose_c_is_unbound() {
        assert_eq!(cls("a.c"), FileClass::Unbound);
        assert_eq!(cls("src/generated.c"), FileClass::Unbound);
        assert_eq!(cls("Scripts/NotAModule/a.c"), FileClass::Unbound);
    }
}
