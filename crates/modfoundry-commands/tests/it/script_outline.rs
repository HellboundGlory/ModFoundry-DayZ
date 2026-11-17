//! `script.outline` through the registry: the outline of every Enforce file under a folder.

use std::path::Path;

use crate::common::{self, tmp, write};
use modfoundry_commands::script_outline::outline_tree;
use modfoundry_core::{Actor, Core};
use serde_json::{json, Value};

fn outline(core: &Core, path: &Path) -> Value {
    core.dispatch(Actor::CliSession, "script.outline", json!({ "path": path }))
        .unwrap_or_else(|e| panic!("script.outline failed: {e}"))
        .to_json()
}

fn tree_json(path: &Path) -> Value {
    serde_json::to_value(outline_tree(path).unwrap()).unwrap()
}

fn outlined_paths(value: &Value) -> Vec<&str> {
    value["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect()
}

#[cfg(unix)]
#[test]
fn symlinked_files_and_folders_are_outlined_under_the_link_path() {
    use std::os::unix::fs::symlink;
    let root = tmp("script-outline-symlinks");
    let targets = tmp("script-outline-symlink-targets");
    write(&root, "a.c", "class A { }\n");
    write(&targets, "b.c", "class B { }\n");
    write(&targets, "lib/c.c", "class C { }\n");
    symlink(targets.join("b.c"), root.join("b.c")).unwrap();
    symlink(targets.join("lib"), root.join("lib")).unwrap();

    let value = tree_json(&root);

    assert_eq!(outlined_paths(&value), ["a.c", "b.c", "lib/c.c"]);
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_lists_each_file_once() {
    use std::os::unix::fs::symlink;
    let root = tmp("script-outline-loop");
    write(&root, "a.c", "class A { }\n");
    symlink(".", root.join("loop")).unwrap();

    let value = tree_json(&root);

    assert_eq!(outlined_paths(&value), ["a.c"]);
}

#[test]
fn dot_folders_are_not_walked_and_are_named() {
    let root = tmp("script-outline-dot-folders");
    write(&root, ".git/x.c", "class Hidden { }\n");
    write(&root, "scripts/.hidden/y.c", "class Hidden { }\n");
    write(&root, "scripts/a.c", "class A { }\n");

    let value = tree_json(&root);

    assert_eq!(outlined_paths(&value), ["scripts/a.c"]);
    assert_eq!(value["skipped"], json!([".git", "scripts/.hidden"]));
}

#[cfg(unix)]
#[test]
fn a_folder_keeps_its_real_path_over_a_link_to_it() {
    use std::os::unix::fs::symlink;
    let root = tmp("script-outline-real-first");
    write(&root, "lib/x.c", "class X { }\n");
    symlink(root.join("lib"), root.join("alias")).unwrap();

    let value = tree_json(&root);

    assert_eq!(outlined_paths(&value), ["lib/x.c"]);
}

#[cfg(unix)]
#[test]
fn broken_symlinks_are_skipped() {
    use std::os::unix::fs::symlink;
    let root = tmp("script-outline-broken-links");
    write(&root, "a.c", "class A { }\n");
    symlink(root.join("missing.c"), root.join("dangling.c")).unwrap();
    symlink(root.join("self.c"), root.join("self.c")).unwrap();

    let value = tree_json(&root);

    assert_eq!(outlined_paths(&value), ["a.c"]);
}

#[test]
fn a_folder_outlines_its_enforce_files_in_path_order() {
    let root = tmp("script-outline-folder");
    write(&root, "Scripts/4_World/a.c", "class WorldThing { }\n");
    write(&root, "Scripts/3_Game/b.c", "class GameThing { }\n");
    write(&root, ".git/x.c", "class Hidden { }\n");
    write(&root, "readme.txt", "not enforce\n");

    let value = outline(&common::core(), &root);

    assert_eq!(value["schema"], 1);
    let files = value["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["Scripts/3_Game/b.c", "Scripts/4_World/a.c"]);
    assert_eq!(files[0]["layer"], "3_Game");
    assert_eq!(files[0]["classes"][0]["name"], "GameThing");
    assert_eq!(files[1]["layer"], "4_World");
    assert_eq!(files[1]["classes"][0]["name"], "WorldThing");
}

#[test]
fn a_missing_path_is_an_error_that_names_it() {
    let missing = tmp("script-outline-missing").join("nope.c");

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "script.outline",
            json!({ "path": missing }),
        )
        .expect_err("a missing path has no outline");

    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "{err}"
    );
}

#[test]
fn a_missing_file_with_another_extension_is_reported_missing() {
    let missing = tmp("script-outline-missing-txt").join("nope.txt");

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "script.outline",
            json!({ "path": missing }),
        )
        .expect_err("a missing path has no outline");

    let message = err.to_string();
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
    assert!(!message.contains("is not an Enforce script"), "{message}");
}

#[test]
fn a_single_file_that_is_not_a_script_is_refused() {
    let readme = write(
        &tmp("script-outline-not-a-script"),
        "readme.txt",
        "not enforce\n",
    );

    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "script.outline",
            json!({ "path": readme }),
        )
        .expect_err("a non-script file has no outline");

    let message = err.to_string();
    assert!(message.contains(&readme.display().to_string()), "{message}");
    assert!(message.contains("is not an Enforce script"), "{message}");
}

#[test]
fn the_layer_comes_from_the_full_path() {
    let root = tmp("script-outline-layer");
    write(&root, "Scripts/4_World/a.c", "class A { }\n");
    let core = common::core();

    for target in [
        root.join("Scripts/4_World/a.c"),
        root.join("Scripts/4_World"),
    ] {
        let file = &outline(&core, &target)["files"][0];
        assert_eq!(file["path"], "a.c");
        assert_eq!(file["layer"], "4_World", "{}", target.display());
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_an_error_that_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let root = tmp("script-outline-locked");
    write(&root, "locked/a.c", "class A { }\n");
    let locked = root.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let err = common::core()
        .dispatch(Actor::CliSession, "script.outline", json!({ "path": root }))
        .expect_err("an unreadable folder has no outline");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        err.to_string().contains(&locked.display().to_string()),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn links_to_an_ancestor_or_an_unreadable_folder_are_skipped_and_named() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let outer = tmp("script-outline-link-limits");
    write(&outer, "sibling.c", "class Sibling { }\n");
    let root = outer.join("mod");
    write(&root, "a.c", "class A { }\n");
    let locked = tmp("script-outline-locked");
    write(&locked, "x.c", "class X { }\n");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    symlink(&outer, root.join("up")).unwrap();
    symlink(&locked, root.join("locked")).unwrap();

    let value = tree_json(&root);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(outlined_paths(&value), ["a.c"]);
    assert_eq!(value["skipped"], json!(["locked", "up"]));
}
