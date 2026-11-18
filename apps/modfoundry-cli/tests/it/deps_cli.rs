//! `mf deps`: saving a mod's dependency mods from the command line.

use std::path::{Path, PathBuf};

use modfoundry_commands::project_settings::PREFS_DEFAULTS_KEY;
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::json;

use crate::binary::{mf, mf_in, stderr, stdout, Temp};
use crate::layout_cli::pack;

fn mod_project(base: &Temp) -> PathBuf {
    let dir = base.join("MyMod");
    // The starter script names MissionServer, which the fake game does not declare.
    let mut options = modfoundry_commands::ScaffoldOptions::new("MyMod");
    options.starter = false;
    modfoundry_commands::scaffold(&dir, &options).expect("scaffold");
    dir
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn deps_add_saves_a_folder_that_deps_list_prints() {
    let base = Temp::new("deps-add-list");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("CommunityFramework");
    std::fs::create_dir_all(&dep).unwrap();

    let added = mf_in(&db, &["deps", "add", arg(&dir), arg(&dep)]);
    assert!(added.status.success(), "{}", stderr(&added));
    let listed = mf_in(&db, &["deps", "list", arg(&dir)]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let saved = dep.canonicalize().unwrap();
    let text = stdout(&listed);
    assert!(
        text.contains(&format!("CommunityFramework  {}", saved.display())),
        "{text}"
    );
}

#[test]
fn deps_add_writes_the_mod_file_and_list_marks_the_row_as_from_the_file() {
    let base = Temp::new("deps-add-file");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("CommunityFramework");
    std::fs::create_dir_all(&dep).unwrap();

    let added = mf_in(&db, &["deps", "add", arg(&dir), arg(&dep)]);
    assert!(added.status.success(), "{}", stderr(&added));
    assert!(dir.join(".modfoundry/dependencies.json").is_file());
    let listed = mf_in(&db, &["deps", "list", arg(&dir)]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let saved = dep.canonicalize().unwrap();
    let text = stdout(&listed);
    assert!(
        text.contains(&format!("CommunityFramework  {}  (file)", saved.display())),
        "{text}"
    );
}

#[test]
fn deps_add_machine_saves_this_machines_list_and_marks_the_row() {
    let base = Temp::new("deps-add-machine");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("CommunityFramework");
    std::fs::create_dir_all(&dep).unwrap();

    let added = mf_in(&db, &["deps", "add", "--machine", arg(&dir), arg(&dep)]);
    assert!(added.status.success(), "{}", stderr(&added));
    assert!(!dir.join(".modfoundry/dependencies.json").exists());
    let listed = mf_in(&db, &["deps", "list", arg(&dir)]);
    let saved = dep.canonicalize().unwrap();
    let text = stdout(&listed);
    assert!(
        text.contains(&format!(
            "CommunityFramework  {}  (machine)",
            saved.display()
        )),
        "{text}"
    );
}

#[test]
fn deps_add_with_preview_prints_the_plan_and_saves_nothing() {
    let base = Temp::new("deps-add-preview");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("CommunityFramework");
    std::fs::create_dir_all(&dep).unwrap();

    let previewed = mf_in(&db, &["--preview", "deps", "add", arg(&dir), arg(&dep)]);
    assert!(previewed.status.success(), "{}", stderr(&previewed));
    let text = stdout(&previewed);
    assert!(text.starts_with("would save:"), "{text}");
    assert!(text.contains("CommunityFramework  "), "{text}");

    let listed = mf_in(&db, &["deps", "list", arg(&dir)]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(
        stdout(&listed).contains("no saved dependencies"),
        "{}",
        stdout(&listed)
    );
}

#[test]
fn deps_remove_refuses_a_dependency_that_is_not_saved() {
    let base = Temp::new("deps-remove-unknown");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let missing = base.join("Missing");

    let removed = mf_in(&db, &["deps", "remove", arg(&dir), arg(&missing)]);
    assert!(!removed.status.success(), "{}", stdout(&removed));
    assert!(
        stderr(&removed).contains("is not a saved dependency"),
        "{}",
        stderr(&removed)
    );
}

fn fake_game(base: &Temp) -> PathBuf {
    let game = base.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[("1_Core\\base.c", b"class PrBase\n{\n}\n")],
    );
    game
}

#[test]
fn lint_sees_the_types_of_a_saved_dependency() {
    let base = Temp::new("deps-lint");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("dep");
    let dep_file = dep.join("Scripts/3_Game/dep.c");
    std::fs::create_dir_all(dep_file.parent().unwrap()).unwrap();
    std::fs::write(&dep_file, "class DepThing\n{\n}\n").unwrap();
    let use_c = dir.join("Scripts/4_World/use.c");
    std::fs::create_dir_all(use_c.parent().unwrap()).unwrap();
    std::fs::write(&use_c, "class Use\n{\n\tDepThing d;\n}\n").unwrap();
    let game = fake_game(&base);
    let game_arg = arg(&game);

    let without = mf_in(
        &db,
        &["lint", arg(&dir), "--game", game_arg, "--no-dependencies"],
    );
    assert!(
        stdout(&without).contains("Unknown type 'DepThing'"),
        "{}",
        stdout(&without)
    );

    let added = mf_in(&db, &["deps", "add", arg(&dir), arg(&dep)]);
    assert!(added.status.success(), "{}", stderr(&added));
    let with = mf_in(&db, &["lint", arg(&dir), "--game", game_arg]);
    let text = stdout(&with);
    assert_eq!(with.status.code(), Some(0), "{text}");
    assert!(text.contains("ok: no issues"), "{text}");
    assert!(!text.contains("DepThing"), "{text}");
    assert!(!text.contains("dependencies are not known"), "{text}");
}

#[test]
fn lint_help_says_dependencies_are_saved_with_deps_add() {
    let out = mf(&["lint", "--help"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("saved with `mf deps add`"), "{text}");
    assert!(!text.contains("saved in the app"), "{text}");
}

#[test]
fn deps_add_machine_with_preview_of_a_defaults_dependency_prints_only_the_note() {
    let base = Temp::new("deps-preview-note");
    let db = base.join("modfoundry.sqlite");
    let dir = mod_project(&base);
    let dep = base.join("CommunityFramework");
    std::fs::create_dir_all(&dep).unwrap();
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Global,
            PREFS_DEFAULTS_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "defaults-row",
                "name": "CommunityFramework",
                "source": {"Folder": {"path": dep.canonicalize().unwrap()}},
                "role": "Both",
                "use_as": "Dependency",
                "enabled": true,
            }]}}),
        )
        .unwrap();

    let previewed = mf_in(
        &db,
        &[
            "--preview",
            "deps",
            "add",
            "--machine",
            arg(&dir),
            arg(&dep),
        ],
    );
    assert!(previewed.status.success(), "{}", stderr(&previewed));
    assert_eq!(
        stdout(&previewed).trim(),
        "CommunityFramework is already in the Defaults mods list"
    );
}

#[test]
fn deps_and_lint_help_describe_each_argument() {
    for args in [
        &["deps", "list", "--help"][..],
        &["deps", "add", "--help"],
        &["deps", "remove", "--help"],
        &["lint", "--help"],
    ] {
        let out = mf(args);
        assert!(out.status.success(), "{}", stderr(&out));
        let text = stdout(&out);
        let arguments = text
            .split("Arguments:\n")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .unwrap_or_else(|| panic!("no arguments in {args:?}: {text}"));
        for line in arguments.lines() {
            let (name, described) = line.trim().split_once("  ").unwrap_or((line.trim(), ""));
            assert!(
                !described.trim().is_empty(),
                "{name} has no description: {text}"
            );
        }
    }
}
