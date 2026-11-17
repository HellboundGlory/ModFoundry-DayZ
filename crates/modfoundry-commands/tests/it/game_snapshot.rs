//! Saves and loads game-build snapshots in mf's data folder.

use std::fs;
use std::path::{Path, PathBuf};

use modfoundry_commands::game_snapshot::{
    auto_snapshot, load_snapshot, save_snapshot, saved_builds, snapshot_install, snapshot_side,
};
use modfoundry_core::{Actor, Outcome};
use modfoundry_gamediff::config::config_model;
use modfoundry_gamediff::layout::layout_model;
use modfoundry_gamediff::script::script_model;
use modfoundry_gamediff::snapshot::Snapshot;
use serde_json::json;

use crate::common::{config_bin, pack, tmp};

const SCRIPT: &str = "class Alpha {}\n";
const LAYOUT: &str = "FrameWidgetClass Root {\n position 0 0\n size 100 100\n}\n";
const ADDON_CONFIG: &str = "class CfgVehicles { class DataThing {}; };\n";

/// An install with a script PBO, a GUI PBO and an addon config.
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
    game
}

fn snapshot(build: &str, mass: &str) -> Snapshot {
    let scripts = script_model(&[(
        "scripts/3_Game/a.c".to_owned(),
        "class A : B\n{\n\tvoid F(int x) {}\n}\n".to_owned(),
    )]);
    let parsed = modfoundry_config::parser::parse(&format!(
        "class CfgVehicles {{ class A {{ mass = {mass}; }}; }};"
    ));
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let configs = config_model(&[("config.cpp".to_owned(), parsed.file)]);
    let layouts = layout_model(&[(
        "gui/layouts/a.layout".to_owned(),
        "FrameWidgetClass Root {\n position 0 0\n}\n".to_owned(),
    )]);
    Snapshot::new(build.to_owned(), scripts, configs, layouts)
}

fn file_names(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_saved_snapshot_loads_back_and_a_second_save_replaces_it() {
    let dir = tmp("snapshot-save");
    let first = snapshot("1.30.1.2", "10");

    let path = save_snapshot(&dir, &first).unwrap();

    assert_eq!(path, dir.join("1.30.1.2.json.gz"));
    assert_eq!(fs::read(&path).unwrap()[..2], [0x1f, 0x8b]);
    assert_eq!(load_snapshot(&path).unwrap(), first);

    let second = snapshot("1.30.1.2", "12");
    assert_eq!(save_snapshot(&dir, &second).unwrap(), path);
    assert_eq!(load_snapshot(&path).unwrap(), second);
    assert_eq!(file_names(&dir), ["1.30.1.2.json.gz"]);
}

#[test]
fn a_build_that_cannot_name_a_file_is_refused() {
    let dir = tmp("snapshot-bad-build");

    for build in ["../x", "a/b", "", ".."] {
        let snapshot = snapshot(build, "10");
        let error = save_snapshot(&dir, &snapshot).unwrap_err();
        assert_eq!(error, format!("build {build} cannot name a snapshot file"));
    }

    assert!(!dir.exists());
}

#[test]
fn saved_builds_lists_snapshot_stems_only() {
    let dir = tmp("snapshot-list");

    assert!(saved_builds(&dir).is_empty());

    save_snapshot(&dir, &snapshot("1.30.2.1", "10")).unwrap();
    save_snapshot(&dir, &snapshot("1.30.1.2", "10")).unwrap();
    fs::write(dir.join("1.30.3.1.json.1234.part"), "{}").unwrap();
    fs::write(dir.join("notes.txt"), "hello").unwrap();
    fs::write(
        dir.join("1.29.1.1.json"),
        snapshot("1.29.1.1", "10").to_json().unwrap(),
    )
    .unwrap();

    assert_eq!(saved_builds(&dir), ["1.29.1.1", "1.30.1.2", "1.30.2.1"]);
}

#[test]
fn snapshot_side_finds_paths_and_build_numbers() {
    let dir = tmp("snapshot-side");
    let saved = snapshot("1.30.1.2", "10");
    let saved_path = save_snapshot(&dir, &saved).unwrap();
    let elsewhere = tmp("snapshot-elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();

    let direct = saved_path.to_str().unwrap();
    assert_eq!(snapshot_side(direct, &dir).unwrap().unwrap(), saved);

    let upper = elsewhere.join("SNAP.JSON");
    fs::write(&upper, saved.to_json().unwrap()).unwrap();
    assert_eq!(
        snapshot_side(upper.to_str().unwrap(), &dir)
            .unwrap()
            .unwrap(),
        saved
    );

    assert!(snapshot_side(elsewhere.to_str().unwrap(), &dir).is_none());

    assert_eq!(snapshot_side("1.30.1.2", &dir).unwrap().unwrap(), saved);
    assert_eq!(
        snapshot_side("1.30.9.9", &dir).unwrap().unwrap_err(),
        "no snapshot saved for build 1.30.9.9 (saved: 1.30.1.2)"
    );
    assert_eq!(
        snapshot_side("1.30.9.9", &elsewhere).unwrap().unwrap_err(),
        "no snapshot saved for build 1.30.9.9 (none saved)"
    );
    assert!(snapshot_side("missing/1.30.9.9", &dir).is_none());

    let plain = snapshot("1.29.1.1", "10");
    fs::write(dir.join("1.29.1.1.json"), plain.to_json().unwrap()).unwrap();
    assert_eq!(snapshot_side("1.29.1.1", &dir).unwrap().unwrap(), plain);
    assert_eq!(
        snapshot_side("missing.json.gz", &dir).unwrap().unwrap_err(),
        "missing.json.gz does not exist"
    );
}

#[test]
fn a_failed_save_leaves_no_part_file() {
    let dir = tmp("snapshot-failed-save");
    fs::create_dir_all(dir.join("1.30.1.2.json.gz")).unwrap();

    assert!(save_snapshot(&dir, &snapshot("1.30.1.2", "10")).is_err());

    assert_eq!(file_names(&dir), ["1.30.1.2.json.gz"]);
}

#[test]
fn an_install_snapshot_saves_and_loads_back() {
    let dir = tmp("snapshot-install");
    let game = install(&dir);
    let snapshots = dir.join("snapshots");

    let saved = snapshot_install(&game, &snapshots).unwrap();

    assert!(saved.build.starts_with("unknown-"), "{}", saved.build);
    assert_eq!(
        saved.path,
        snapshots.join(format!("{}.json.gz", saved.build))
    );
    assert!(saved.counts.scripts > 0 && saved.counts.configs > 0 && saved.counts.layouts > 0);
    assert_eq!(load_snapshot(&saved.path).unwrap().build, saved.build);
}

#[test]
fn a_folder_is_not_snapshotted() {
    let dir = tmp("snapshot-folder");
    let folder = dir.join("loose");
    crate::common::write(&folder, "scripts/3_Game/a.c", SCRIPT);
    let snapshots = dir.join("snapshots");

    let error = snapshot_install(&folder, &snapshots).unwrap_err();

    assert_eq!(
        error,
        format!(
            "{} is not a game install; a snapshot needs the build number of an install",
            folder.display()
        )
    );
    assert!(!snapshots.exists());
}

#[test]
fn the_automatic_snapshot_saves_once_per_build() {
    let dir = tmp("snapshot-auto");
    let game = install(&dir);
    let snapshots = dir.join("snapshots");

    assert!(auto_snapshot(&game, None).is_none());

    let line = auto_snapshot(&game, Some(&snapshots)).expect("the first call saves");
    assert!(
        line.starts_with("saved a snapshot of build unknown-"),
        "{line}"
    );
    let saved: Vec<PathBuf> = fs::read_dir(&snapshots)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(saved.len(), 1);
    let first = fs::read(&saved[0]).unwrap();

    assert!(auto_snapshot(&game, Some(&snapshots)).is_none());
    assert_eq!(fs::read(&saved[0]).unwrap(), first);

    let folder = dir.join("loose");
    crate::common::write(&folder, "scripts/3_Game/a.c", SCRIPT);
    assert!(auto_snapshot(&folder, Some(&snapshots)).is_none());
}

#[test]
fn a_save_replaces_the_uncompressed_file_of_its_build() {
    let dir = tmp("snapshot-old-json");
    let old = snapshot("1.30.1.2", "10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("1.30.1.2.json"), old.to_json().unwrap()).unwrap();
    fs::write(dir.join("1.30.5.5.json"), b"").unwrap();

    let new = snapshot("1.30.1.2", "12");
    save_snapshot(&dir, &new).unwrap();

    assert_eq!(file_names(&dir).len(), 2);
    assert!(!dir.join("1.30.1.2.json").exists());
    assert_eq!(snapshot_side("1.30.1.2", &dir).unwrap().unwrap(), new);
    assert_eq!(saved_builds(&dir), ["1.30.1.2"]);
}

#[test]
fn the_automatic_snapshot_skips_an_install_read_with_warnings() {
    let dir = tmp("snapshot-auto-partial");
    let game = install(&dir);
    fs::write(game.join("addons/broken.pbo"), b"not a pbo").unwrap();
    let snapshots = dir.join("snapshots");

    let line = auto_snapshot(&game, Some(&snapshots)).expect("a skipped save still reports");

    assert!(line.starts_with("no snapshot of build unknown-"), "{line}");
    assert!(line.contains("broken.pbo"), "{line}");
    assert!(saved_builds(&snapshots).is_empty());
}

#[test]
fn an_empty_snapshot_file_does_not_count_as_saved() {
    let dir = tmp("snapshot-auto-empty");
    let game = install(&dir);
    let snapshots = dir.join("snapshots");
    let saved = snapshot_install(&game, &snapshots).unwrap();
    fs::write(&saved.path, b"").unwrap();

    let line = auto_snapshot(&game, Some(&snapshots)).expect("an empty file is replaced");

    assert!(line.starts_with("saved a snapshot"), "{line}");
    assert_eq!(load_snapshot(&saved.path).unwrap().build, saved.build);
}

#[test]
fn game_snapshot_dispatches_through_the_registry() {
    let dir = tmp("snapshot-registry");
    let game = install(&dir);
    let snapshots = dir.join("snapshots");
    let core = crate::common::core();

    let outcome = core
        .dispatch(
            Actor::CliSession,
            "game.snapshot",
            json!({ "game": game, "snapshots": snapshots }),
        )
        .unwrap();
    let Outcome::Data(saved) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    let build = saved["build"].as_str().expect("a build");
    assert!(build.starts_with("unknown-"), "{saved}");
    assert_eq!(
        saved["path"],
        json!(snapshots.join(format!("{build}.json.gz")))
    );

    let outcome = core
        .dispatch(
            Actor::CliSession,
            "game.diff",
            json!({ "old": build, "new": game, "snapshots": snapshots }),
        )
        .unwrap();
    let Outcome::Data(report) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    assert_eq!(report["summary"]["scripts"], 0, "{report}");
    assert_eq!(report["summary"]["configs"], 0, "{report}");
    assert_eq!(report["summary"]["layouts"], 0, "{report}");
}

#[test]
fn a_missing_json_path_is_named_as_missing() {
    let dir = tmp("snapshot-missing-json");

    assert_eq!(
        snapshot_side("old.json", &dir).unwrap().unwrap_err(),
        "old.json does not exist"
    );
}
