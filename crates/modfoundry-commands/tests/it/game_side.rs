//! Reading one side of a game-build compare: an install, a loose folder and a single `.pbo`.

use std::path::{Path, PathBuf};

use modfoundry_commands::game_side::{read_side, SideCounts, SideKind};

use crate::common::{config_bin, pack, tmp, write};

const SCRIPT: &str = "class Alpha {}\n";
const LAYOUT: &str = "FrameWidgetClass Root {\n position 0 0\n size 100 100\n}\n";
const ADDON_CONFIG: &str = "class CfgVehicles { class DataThing {}; };\n";
const MAP_CONFIG: &str = "class CfgWorlds { class Sakhal {}; };\n";
const MOD_CONFIG: &str = "class CfgVehicles { class ModThing {}; };\n";

/// An install with a script PBO, a GUI PBO, an addon config, a signed map addon and a `@mod` folder.
fn install(dir: &Path) -> PathBuf {
    let game = dir.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("3_Game\\a.c", SCRIPT.as_bytes())],
    );
    pack(
        &game.join("dta/gui.pbo"),
        &[("prefix", "gui")],
        &[("layouts\\a.layout", LAYOUT.as_bytes())],
    );
    pack(
        &game.join("addons/data.pbo"),
        &[("prefix", "data")],
        &[("config.bin", config_bin(ADDON_CONFIG).as_slice())],
    );
    pack(
        &game.join("sakhal/addons/world_sakhal.pbo"),
        &[("prefix", "sakhal")],
        &[("config.bin", config_bin(MAP_CONFIG).as_slice())],
    );
    std::fs::write(
        game.join("sakhal/addons/world_sakhal.pbo.dayz.bisign"),
        b"signature",
    )
    .unwrap();
    pack(
        &game.join("@mod/addons/m.pbo"),
        &[("prefix", "mod")],
        &[
            ("3_Game\\mod.c", b"class ModOnly {}\n"),
            ("config.bin", config_bin(MOD_CONFIG).as_slice()),
        ],
    );
    game
}

#[test]
fn an_install_reads_vanilla_pbos_only() {
    let dir = tmp("side-install");
    let game = install(&dir);

    let side = read_side(&game).unwrap();

    assert_eq!(side.kind, SideKind::Install);
    assert!(
        side.build
            .as_deref()
            .is_some_and(|key| key.starts_with("unknown-")),
        "{:?}",
        side.build
    );
    assert!(
        side.scripts.classes.contains_key("Alpha"),
        "{:?}",
        side.warnings
    );
    assert!(!side.scripts.classes.contains_key("ModOnly"));
    assert!(side.layouts.files.contains_key("gui/layouts/a.layout"));
    assert!(
        side.configs.classes.contains_key("cfgvehicles/datathing"),
        "{:?}",
        side.warnings
    );
    assert!(side.configs.classes.contains_key("cfgworlds/sakhal"));
    assert!(!side.configs.classes.contains_key("cfgvehicles/modthing"));
    assert_eq!(
        side.counts,
        SideCounts {
            scripts: 1,
            configs: 2,
            layouts: 1
        }
    );
}

#[test]
fn configs_merge_in_required_addons_order() {
    let dir = tmp("side-order");
    let game = dir.join("game");
    std::fs::create_dir_all(game.join("dta")).unwrap();
    pack(
        &game.join("addons/z_early.pbo"),
        &[("prefix", "z_early")],
        &[(
            "config.bin",
            config_bin(
                "class CfgPatches { class ZEarly { requiredAddons[] = {}; }; };\n\
                 class CfgX { class Item { v = 1; }; };\n",
            )
            .as_slice(),
        )],
    );
    pack(
        &game.join("addons/a_late.pbo"),
        &[("prefix", "a_late")],
        &[(
            "config.bin",
            config_bin(
                "class CfgPatches { class ALate { requiredAddons[] = { \"ZEarly\" }; }; };\n\
                 class CfgX { class Item { v = 2; }; };\n",
            )
            .as_slice(),
        )],
    );

    let side = read_side(&game).unwrap();

    let item = &side.configs.classes["cfgx/item"];
    assert_eq!(item.properties["v"].value, "2", "{:?}", side.warnings);
}

#[test]
fn a_folder_and_a_pbo_line_up_with_an_install() {
    let dir = tmp("side-align");
    let game = install(&dir);
    let installed = read_side(&game).unwrap();

    let loose = dir.join("loose");
    write(&loose, "scripts/3_Game/a.c", SCRIPT);
    write(&loose, "gui/layouts/a.layout", LAYOUT);
    write(&loose, "scripts/config.cpp", "class CfgPatches {};\n");
    let folder = read_side(&loose).unwrap();
    assert_eq!(folder.kind, SideKind::Folder);
    assert_eq!(folder.build, None);
    assert_eq!(
        folder.scripts.classes.keys().collect::<Vec<_>>(),
        installed.scripts.classes.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        folder.scripts.classes["Alpha"].place.as_ref().unwrap().file,
        "scripts/3_Game/a.c"
    );
    assert_eq!(
        folder.layouts.files.keys().collect::<Vec<_>>(),
        installed.layouts.files.keys().collect::<Vec<_>>()
    );

    let pbo = read_side(&game.join("dta/gui.pbo")).unwrap();
    assert_eq!(pbo.kind, SideKind::Pbo);
    assert_eq!(pbo.build, None);
    assert_eq!(
        pbo.layouts.files.keys().collect::<Vec<_>>(),
        installed
            .layouts
            .files
            .keys()
            .filter(|key| key.starts_with("gui/"))
            .collect::<Vec<_>>()
    );
    assert_eq!(pbo.counts.layouts, 1);
}

#[test]
fn a_bad_entry_is_a_warning_and_the_rest_is_read() {
    let dir = tmp("side-badentry");
    let pbo = dir.join("broken.pbo");
    pack(
        &pbo,
        &[("prefix", "broken")],
        &[
            ("config.bin", b"\x00not a config at all\xff"),
            ("3_Game\\a.c", SCRIPT.as_bytes()),
        ],
    );

    let side = read_side(&pbo).unwrap();

    assert_eq!(side.warnings.len(), 1, "{:?}", side.warnings);
    assert!(
        side.warnings[0].contains("config.bin"),
        "{:?}",
        side.warnings
    );
    assert!(side.scripts.classes.contains_key("Alpha"));
    assert_eq!(side.counts.scripts, 1);
}

#[test]
fn sides_that_cannot_be_read_are_errors() {
    let dir = tmp("side-errors");
    let missing = dir.join("missing");
    let text = write(&dir, "notes.txt", "x");
    let not_a_pbo = write(&dir, "x.pbo", "x");
    let empty_dta = dir.join("empty_dta");
    std::fs::create_dir_all(empty_dta.join("dta")).unwrap();
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();

    let cases: Vec<(PathBuf, String)> = vec![
        (
            missing.clone(),
            format!("{} does not exist", missing.display()),
        ),
        (
            text.clone(),
            format!(
                "{} is not a game install, a folder or a .pbo",
                text.display()
            ),
        ),
        (
            empty_dta.clone(),
            format!("no game PBOs under {}", empty_dta.display()),
        ),
        (
            empty.clone(),
            format!("no scripts, configs or layouts under {}", empty.display()),
        ),
    ];

    for (path, expected) in cases {
        let err = read_side(&path).expect_err(path.to_str().unwrap());
        assert_eq!(err, expected);
    }
    assert!(
        read_side(&not_a_pbo).is_err(),
        "a broken .pbo is an open error"
    );
}

#[test]
fn two_vanilla_pbos_with_one_name_keep_both_configs() {
    let game = tmp("side-same-name").join("game");
    pack(
        &game.join("addons/data.pbo"),
        &[("prefix", "data")],
        &[("config.bin", config_bin(ADDON_CONFIG).as_slice())],
    );
    pack(
        &game.join("sakhal/addons/data.pbo"),
        &[("prefix", "sakhal")],
        &[("config.bin", config_bin(MAP_CONFIG).as_slice())],
    );
    std::fs::write(
        game.join("sakhal/addons/data.pbo.dayz.bisign"),
        b"signature",
    )
    .unwrap();
    std::fs::create_dir_all(game.join("dta")).unwrap();

    let side = read_side(&game).unwrap();

    assert!(side.configs.classes.contains_key("cfgvehicles/datathing"));
    assert!(side.configs.classes.contains_key("cfgworlds/sakhal"));
    assert_eq!(side.warnings.len(), 1, "{:?}", side.warnings);
    assert!(side.warnings[0].contains("data.pbo"), "{:?}", side.warnings);
}

#[test]
fn an_unreadable_pbo_is_warned_once() {
    let game = tmp("side-bad-pbo").join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("3_Game\\a.c", SCRIPT.as_bytes())],
    );
    write(&game, "dta/bad.pbo", "garbage");

    let side = read_side(&game).unwrap();

    let bad: Vec<&String> = side
        .warnings
        .iter()
        .filter(|warning| warning.contains("bad.pbo"))
        .collect();
    assert_eq!(bad.len(), 1, "{:?}", side.warnings);
}

#[test]
fn a_side_with_nothing_read_says_why() {
    let game = tmp("side-nothing-read").join("game");
    write(&game, "dta/bad.pbo", "garbage");

    let error = read_side(&game).unwrap_err();

    assert!(
        error.starts_with("no scripts, configs or layouts under"),
        "{error}"
    );
    assert!(error.contains("bad.pbo"), "{error}");
}

#[test]
fn a_config_cpp_beside_a_config_bin_is_read_once() {
    let dir = tmp("side-cpp-beside-bin");
    let loose = dir.join("unpacked");
    write(&loose, "a/config.cpp", MOD_CONFIG);
    std::fs::write(loose.join("a/config.bin"), config_bin(MOD_CONFIG)).unwrap();
    std::fs::create_dir_all(loose.join("b")).unwrap();
    std::fs::write(loose.join("b/config.bin"), config_bin(ADDON_CONFIG)).unwrap();

    let side = read_side(&loose).unwrap();

    assert_eq!(side.counts.configs, 2);
    assert!(side.configs.classes.contains_key("cfgvehicles/datathing"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pbo_holding_a_config_cpp_and_a_config_bin_reads_only_the_bin() {
    let dir = tmp("side-pbo-cpp-and-bin");
    let pbo = dir.join("both.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[
            ("config.cpp", MOD_CONFIG.as_bytes()),
            ("config.bin", config_bin(ADDON_CONFIG).as_slice()),
        ],
    );

    let side = read_side(&pbo).unwrap();

    assert_eq!(side.counts.configs, 1);
    assert!(side.configs.classes.contains_key("cfgvehicles/datathing"));
    let _ = std::fs::remove_dir_all(&dir);
}
