use std::path::PathBuf;

use enforce_check::{Layer, Target};
use enforce_check::loader::load_mod_dir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/loader")
        .join(name)
}

fn paths(files: &[enforce_check::SourceFile]) -> Vec<(&str, Layer)> {
    files.iter().map(|file| (file.path.as_str(), file.layer)).collect()
}

// The config names the folders of each script module, whatever they are
// called, and the game prints a file by its path inside the pack.
#[test]
fn a_text_config_names_the_script_modules_and_defines() {
    let loaded = load_mod_dir(&fixture("textmod"), "gear-mod");
    assert_eq!(loaded.info.name, "gear-mod");
    assert_eq!(loaded.info.cfg_class, "My_Gear_Mod");
    assert_eq!(loaded.defines, ["My_Gear_Mod", "MY_GEAR"]);
    assert_eq!(loaded.addons, ["My_Gear"]);
    assert_eq!(loaded.required, ["DZ_Data", "My_Core"]);
    assert_eq!(loaded.inputs, ["UAGearOpen", "GearToggle"]);
    assert_eq!(
        paths(&loaded.files()),
        [
            ("My/Gear/code/game/a.c", Layer::Game),
            ("My/Gear/code/game/b.c", Layer::Game),
            ("My/Gear/code/world/w.c", Layer::World),
            ("My/Gear/extra/x.c", Layer::World),
        ]
    );
    assert!(loaded.files()[0].text.contains("GearA"));
}

// A class of a binarized config, written the way the game's tools write one.
#[derive(Default)]
struct Packed {
    texts: Vec<(&'static str, &'static str)>,
    lists: Vec<(&'static str, Vec<&'static str>)>,
    classes: Vec<(&'static str, Packed)>,
}

fn pack_body(class: &Packed, out: &mut Vec<u8>) {
    out.push(0);
    out.push((class.classes.len() + class.texts.len() + class.lists.len()) as u8);
    let mut pending = Vec::new();
    for (name, child) in &class.classes {
        out.push(0);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        pending.push((out.len(), child));
        out.extend_from_slice(&[0; 4]);
    }
    for (name, text) in &class.texts {
        out.extend_from_slice(&[1, 0]);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    for (name, items) in &class.lists {
        out.push(2);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.push(items.len() as u8);
        for item in items {
            out.push(0);
            out.extend_from_slice(item.as_bytes());
            out.push(0);
        }
    }
    out.extend_from_slice(&[0; 4]);
    for (at, child) in pending {
        let offset = (out.len() as u32).to_le_bytes();
        out[at..at + 4].copy_from_slice(&offset);
        pack_body(child, out);
    }
}

fn pack(root: &Packed) -> Vec<u8> {
    let mut out = b"\0raP".to_vec();
    out.extend_from_slice(&[0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0]);
    pack_body(root, &mut out);
    out
}

fn temp_mod(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("enforce-check-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_binarized_config_reads_like_a_text_one() {
    let module = Packed { lists: vec![("files", vec!["Bin/Pack/scripts/4_World"])], ..Packed::default() };
    let class = Packed {
        texts: vec![("type", "mod")],
        lists: vec![("defines", vec!["BIN_DEFINE"])],
        classes: vec![("defs", Packed { classes: vec![("worldScriptModule", module)], ..Packed::default() })],
    };
    let patch = Packed { lists: vec![("requiredAddons", vec!["DZ_Data"])], ..Packed::default() };
    let root = Packed {
        classes: vec![
            ("CfgPatches", Packed { classes: vec![("Bin_Pack", patch)], ..Packed::default() }),
            ("CfgMods", Packed { classes: vec![("Bin_Mod", class)], ..Packed::default() }),
        ],
        ..Packed::default()
    };
    let dir = temp_mod("binmod");
    let pack_dir = dir.join("pack/Bin/Pack");
    std::fs::create_dir_all(pack_dir.join("scripts/4_World")).unwrap();
    std::fs::write(pack_dir.join("config.bin"), pack(&root)).unwrap();
    std::fs::write(pack_dir.join("scripts/4_World/w.c"), "class BinWorld\n{\n}\n").unwrap();

    let loaded = load_mod_dir(&dir, "bin-mod");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(loaded.info.cfg_class, "Bin_Mod");
    assert_eq!(loaded.defines, ["Bin_Mod", "BIN_DEFINE"]);
    assert_eq!(loaded.addons, ["Bin_Pack"]);
    assert_eq!(loaded.required, ["DZ_Data"]);
    assert_eq!(paths(&loaded.files()), [("Bin/Pack/scripts/4_World/w.c", Layer::World)]);
}

#[test]
fn a_tree_with_no_config_loads_by_its_layer_folder_names() {
    let loaded = load_mod_dir(&fixture("loose"), "loose");
    assert_eq!(loaded.info.cfg_class, "loose");
    assert_eq!(loaded.defines, Vec::<String>::new());
    assert_eq!(
        paths(&loaded.files()),
        [("Scripts/3_Game/g.c", Layer::Game), ("Scripts/4_World/sub/w.c", Layer::World)]
    );
}

// Each module compiles vanilla's files, then each mod's in load order; a file's
// `mod_index` says which mod it came from, and every define a mod's config
// declares is set.
#[test]
fn vanilla_and_mods_are_assembled_module_by_module() {
    let vanilla = vec![
        source("Scripts/3_Game/v.c", Layer::Game),
        source("Scripts/4_World/v.c", Layer::World),
    ];
    let mods = vec![load_mod_dir(&fixture("loose"), "loose"), load_mod_dir(&fixture("textmod"), "gear-mod")];
    let input = enforce_check::loader::assemble(Target::DiagServer, vanilla, mods);

    let order: Vec<(&str, usize)> = input.files.iter().map(|file| (file.path.as_str(), file.mod_index)).collect();
    assert_eq!(
        order,
        [
            ("Scripts/3_Game/v.c", 0),
            ("Scripts/3_Game/g.c", 1),
            ("My/Gear/code/game/a.c", 2),
            ("My/Gear/code/game/b.c", 2),
            ("Scripts/4_World/v.c", 0),
            ("Scripts/4_World/sub/w.c", 1),
            ("My/Gear/code/world/w.c", 2),
            ("My/Gear/extra/x.c", 2),
        ]
    );
    let classes: Vec<&str> = input.mods.iter().map(|info| info.cfg_class.as_str()).collect();
    assert_eq!(classes, ["loose", "My_Gear_Mod", "DayZ"]);
    assert_eq!(input.extra_defines, ["MY_GEAR"]);
    assert_eq!(input.input_names, ["UAGearOpen", "GearToggle"]);
    assert_eq!(input.target, Target::DiagServer);
}

fn source(path: &str, layer: Layer) -> enforce_check::SourceFile {
    enforce_check::SourceFile { path: path.into(), layer, mod_index: 0, text: String::new() }
}

// Expansion lists one folder of `#define`s in every script module, ahead of
// the module's own folder.
#[test]
fn a_folder_listed_in_two_modules_is_loaded_in_both() {
    let dir = temp_mod("shared");
    let pack = dir.join("pack/Sh");
    for folder in ["Common", "3_Game", "4_World"] {
        std::fs::create_dir_all(pack.join(folder)).unwrap();
        std::fs::write(pack.join(folder).join("f.c"), "").unwrap();
    }
    let config = "class CfgMods { class Sh_Mod { class defs {
        class gameScriptModule { files[] = {\"Sh/Common\", \"Sh/3_Game\"}; };
        class worldScriptModule { files[] = {\"Sh/Common\", \"Sh/4_World\"}; };
    }; }; };";
    std::fs::write(pack.join("config.cpp"), config).unwrap();

    let loaded = load_mod_dir(&dir, "shared");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        paths(&loaded.files()),
        [
            ("Sh/Common/f.c", Layer::Game),
            ("Sh/3_Game/f.c", Layer::Game),
            ("Sh/Common/f.c", Layer::World),
            ("Sh/4_World/f.c", Layer::World),
        ]
    );
}

// pk-01 to pk-14: the game compiles packs in the order of their paths inside
// their PBOs, whatever order the mods are given in, and a pack waits for the
// packs it requires. Requiring nothing moves no pack ahead.
#[test]
fn the_packs_of_every_mod_are_compiled_in_path_order_after_the_packs_they_require() {
    let pack = |name: &str, folder: &str, prefix: &str, requires: &str| {
        let dir = temp_mod(name);
        let root = dir.join(folder).join(prefix);
        std::fs::create_dir_all(root.join("3_Game")).unwrap();
        std::fs::write(root.join("3_Game/s.c"), "#define SEEN\n").unwrap();
        let config = format!(
            "class CfgPatches {{ class {prefix}_Addon {{ requiredAddons[] = {{{requires}}}; }}; }};
            class CfgMods {{ class {prefix}_Mod {{ class defs {{ class gameScriptModule {{ files[] = {{\"{prefix}/3_Game\"}}; }}; }}; }}; }};"
        );
        std::fs::write(root.join("config.cpp"), config).unwrap();
        let loaded = load_mod_dir(&dir, name);
        let _ = std::fs::remove_dir_all(&dir);
        loaded
    };
    let mods = vec![
        pack("order-one", "a_pbo", "Beta", "\"DZ_Data\""),
        pack("order-two", "z_pbo", "alpha", "\"Gamma_Addon\""),
        pack("order-three", "m_pbo", "Gamma", ""),
    ];
    let vanilla = vec![source("Scripts/3_Game/v.c", Layer::Game)];
    let input = enforce_check::loader::assemble(Target::RetailServer, vanilla, mods);
    let game: Vec<(&str, usize)> = input.files.iter().map(|file| (file.path.as_str(), file.mod_index)).collect();
    assert_eq!(
        game,
        [("Scripts/3_Game/v.c", 0), ("Beta/3_Game/s.c", 1), ("Gamma/3_Game/s.c", 3), ("alpha/3_Game/s.c", 2)]
    );
}

// Expansion reaches Community Framework through a PBO that holds no script,
// so an unpacked copy shows a requirement nothing provides. Such a pack is
// compiled after the mods listed before it, and the reader is told.
#[test]
fn a_pack_that_requires_an_addon_nobody_provides_follows_the_mods_before_it() {
    let pack = |name: &str, prefix: &str, requires: &str| {
        let dir = temp_mod(name);
        let root = dir.join("pbo").join(prefix);
        std::fs::create_dir_all(root.join("3_Game")).unwrap();
        std::fs::write(root.join("3_Game/s.c"), "#define SEEN\n").unwrap();
        let config = format!(
            "class CfgPatches {{ class {prefix}_Addon {{ requiredAddons[] = {{\"{requires}\"}}; }}; }};
            class CfgMods {{ class {prefix}_Mod {{ class defs {{ class gameScriptModule {{ files[] = {{\"{prefix}/3_Game\"}}; }}; }}; }}; }};"
        );
        std::fs::write(root.join("config.cpp"), config).unwrap();
        dir
    };
    // The game's own addons are named without regard to letter case.
    let dirs = [pack("hidden-one", "Zed", "dz_data"), pack("hidden-two", "Alpha", "Hidden_Addon")];
    let mods = [("one".to_string(), dirs[0].clone()), ("two".to_string(), dirs[1].clone())];
    let loaded = enforce_check::loader::load_input(Target::RetailServer, None, &mods);
    for dir in &dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
    let order: Vec<(&str, usize)> = loaded.input.files.iter().map(|file| (file.path.as_str(), file.mod_index)).collect();
    assert_eq!(order, [("Zed/3_Game/s.c", 1), ("Alpha/3_Game/s.c", 2)]);
    assert_eq!(
        loaded.notes,
        ["pack Alpha requires addon Hidden_Addon, which no mod given provides: it is compiled after the mods listed before it"]
    );
}

// Nothing a mod folder holds may hang the loader or run it out of memory: a
// list with a stray `;`, a binarized config whose classes all point at one
// body or back at themselves, a folder that links to its own parent.
#[test]
fn hostile_configs_and_trees_load_without_hanging() {
    let dir = temp_mod("hostile");
    let text = dir.join("a/A");
    std::fs::create_dir_all(text.join("code")).unwrap();
    std::fs::write(text.join("code/a.c"), "").unwrap();
    std::fs::write(
        text.join("config.cpp"),
        "class CfgMods { class A_Mod { class defs { class gameScriptModule { files[] = {\"A/code\" ; }; }; }; }; };",
    )
    .unwrap();

    // 2,000 classes that share one body holding a long name, then one that
    // points back at the root.
    let shared_at = 16 + 2 + 2_001 * (1 + 2 + 4) + 4;
    let mut bytes = b"\0raP".to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0, 0xd1, 0x0f]);
    for _ in 0..2_000 {
        bytes.extend_from_slice(&[0, b'c', 0]);
        bytes.extend_from_slice(&(shared_at as u32 + 1).to_le_bytes());
    }
    bytes.extend_from_slice(&[0, b'r', 0, 16, 0, 0, 0]);
    bytes.extend_from_slice(&[0; 4]);
    bytes.resize(shared_at + 1, 0);
    bytes.extend_from_slice(&[0, 1, 1, 0]);
    bytes.extend(std::iter::repeat_n(b'n', 200_000));
    bytes.extend_from_slice(&[0, b'v', 0]);
    let binary = dir.join("b/B");
    std::fs::create_dir_all(&binary).unwrap();
    std::fs::write(binary.join("config.bin"), &bytes).unwrap();

    #[cfg(unix)]
    for name in ["up1", "up2", "up3"] {
        std::os::unix::fs::symlink("..", text.join(name)).unwrap();
    }

    let started = std::time::Instant::now();
    let loaded = load_mod_dir(&dir, "hostile");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(paths(&loaded.files()), [("A/code/a.c", Layer::Game)]);
}

// Inside a mod a pack is read after the packs it requires, whatever their
// folders are called; and a config written in parts reads as one.
#[test]
fn packs_are_read_in_dependency_order() {
    let dir = temp_mod("order");
    let first = dir.join("a_main/Main");
    let second = dir.join("z_base/Base");
    std::fs::create_dir_all(first.join("3_Game")).unwrap();
    std::fs::create_dir_all(second.join("3_Game")).unwrap();
    std::fs::write(first.join("3_Game/m.c"), "class Main\n{\n}\n").unwrap();
    std::fs::write(second.join("3_Game/b.c"), "class Base\n{\n}\n").unwrap();
    std::fs::write(
        first.join("config.cpp"),
        "class CfgMods;
        class CfgPatches { class Main { requiredAddons[] = {\"z_BASE\"}; }; };
        }
        class CfgMods { class Main_Mod { defines[] = {\"MAIN\"}; }; };
        class CfgMods { class Main_Mod { class defs { class gameScriptModule { files[] = {\"Main/3_Game\"}; }; }; }; };",
    )
    .unwrap();
    std::fs::write(
        second.join("config.cpp"),
        "class CfgPatches { class Z_Base { requiredAddons[] = {\"DZ_Data\"}; }; };
        class CfgMods { class Base_Mod { class defs { class gameScriptModule { files[] = {\"Base/3_Game\"}; }; }; }; };",
    )
    .unwrap();

    let loaded = load_mod_dir(&dir, "order");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        paths(&loaded.files()),
        [("Base/3_Game/b.c", Layer::Game), ("Main/3_Game/m.c", Layer::Game)]
    );
    assert_eq!(loaded.defines, ["Base_Mod", "Main_Mod", "MAIN"]);
    assert_eq!(loaded.required, ["DZ_Data"]);
}

// Two packs that require each other are still both loaded, in path order.
#[test]
fn packs_that_require_each_other_load_in_path_order() {
    let dir = temp_mod("circle");
    for (prefix, other) in [("Alpha", "Beta"), ("Beta", "Alpha")] {
        let root = dir.join("pbo").join(prefix);
        std::fs::create_dir_all(root.join("3_Game")).unwrap();
        std::fs::write(root.join("3_Game/s.c"), "").unwrap();
        let config = format!(
            "class CfgPatches {{ class {prefix} {{ requiredAddons[] = {{\"{other}\"}}; }}; }};
            class CfgMods {{ class {prefix}_Mod {{ class defs {{ class gameScriptModule {{ files[] = {{\"{prefix}/3_Game\"}}; }}; }}; }}; }};"
        );
        std::fs::write(root.join("config.cpp"), config).unwrap();
    }
    let loaded = load_mod_dir(&dir, "circle");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(paths(&loaded.files()), [("Alpha/3_Game/s.c", Layer::Game), ("Beta/3_Game/s.c", Layer::Game)]);
}
