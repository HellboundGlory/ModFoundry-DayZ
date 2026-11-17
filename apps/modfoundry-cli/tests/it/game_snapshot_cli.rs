//! `mf game-snapshot` end to end.

use std::path::Path;

use crate::binary::{mf, stderr, stdout, Temp};
use crate::layout_cli::pack;

/// An install with a script PBO, a GUI PBO and an addon config.
fn install(dir: &Path) -> std::path::PathBuf {
    let game = dir.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[("3_Game/a.c", b"class Alpha {}\n")],
    );
    pack(
        &game.join("dta/gui.pbo"),
        "gui",
        &[(
            "layouts/a.layout",
            b"FrameWidgetClass Root {\n position 0 0\n size 100 100\n}\n",
        )],
    );
    pack(
        &game.join("addons/data.pbo"),
        "data",
        &[(
            "config.cpp",
            b"class CfgVehicles { class DataThing {}; };\n",
        )],
    );
    game
}

#[test]
fn game_snapshot_refuses_a_folder() {
    let dir = Temp::new("game-snapshot-cli-folder");
    let folder = dir.join("loose");
    std::fs::create_dir_all(folder.join("scripts/3_Game")).unwrap();
    std::fs::write(folder.join("scripts/3_Game/a.c"), "class Alpha {}\n").unwrap();

    let out = mf(&[
        "game-snapshot",
        folder.to_str().unwrap(),
        "--snapshots",
        dir.join("snapshots").to_str().unwrap(),
    ]);

    assert_ne!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("not a game install"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn game_snapshot_saves_an_install() {
    let dir = Temp::new("game-snapshot-cli-install");
    let game = install(dir.path());
    let snapshots = dir.join("snapshots");

    let out = mf(&[
        "game-snapshot",
        game.to_str().unwrap(),
        "--snapshots",
        snapshots.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("saved build unknown-"), "got: {text}");
    assert!(
        text.contains(" scripts, 1 configs, 1 layouts) to "),
        "got: {text}"
    );

    let json = mf(&[
        "game-snapshot",
        game.to_str().unwrap(),
        "--snapshots",
        snapshots.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(json.status.code(), Some(0), "{}", stderr(&json));
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("JSON output");
    assert!(
        value["build"]
            .as_str()
            .is_some_and(|build| build.starts_with("unknown-")),
        "{value}"
    );
    assert_eq!(value["counts"]["scripts"], 1, "{value}");
}
