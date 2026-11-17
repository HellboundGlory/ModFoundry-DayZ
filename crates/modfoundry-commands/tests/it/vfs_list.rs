//! `vfs.list` contract tests: the layer stack the GUI tree and
//! MCP clients hand the command, and the entries/skipped payload it answers with.

use std::path::{Path, PathBuf};

use crate::common::{self, tmp, write};
use modfoundry_core::{Actor, Outcome};
use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
use serde_json::{json, Value};

fn pack(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let entries: Vec<PboFileToWrite> = files
        .iter()
        .map(|(n, d)| PboFileToWrite {
            name: (*n).to_string(),
            data: d.to_vec(),
            timestamp: 5,
        })
        .collect();
    let bytes = pack_pbo(&[], &entries, &PackOptions::default()).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn list(layers: Value) -> Value {
    let out = common::core()
        .dispatch(Actor::CliSession, "vfs.list", json!({ "layers": layers }))
        .unwrap_or_else(|e| panic!("vfs.list failed: {e}"));
    match out {
        Outcome::Data(v) => v,
        other => panic!("expected Data, got {other:?}"),
    }
}

fn entry<'a>(payload: &'a Value, rel: &str) -> &'a Value {
    payload["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["rel"] == rel)
        .unwrap_or_else(|| panic!("no entry {rel} in {payload}"))
}

#[test]
fn a_directory_layer_lists_its_files_under_its_layer_id() {
    let dir = tmp("dir");
    write(&dir, "config.cpp", "class CfgPatches {};");
    write(&dir, "Scripts/A.c", "class A {};");

    let payload = list(json!([
        { "id": "src", "kind": "WorkspaceSource", "path": dir },
    ]));

    let entries = payload["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "got {payload}");
    assert_eq!(
        payload["layers"],
        json!([{ "id": "src", "kind": "WorkspaceSource" }])
    );
    assert_eq!(payload["skipped"], json!([]));
    assert_eq!(payload["case_clashes"], json!([]));

    let cfg = entry(&payload, "config.cpp");
    assert_eq!(cfg["layer"], "src");
    assert_eq!(cfg["kind"], "WorkspaceSource");
    assert_eq!(cfg["shadowed_by"], json!([]));
    let script = entry(&payload, "Scripts/A.c");
    assert_eq!(script["layer"], "src");
    assert_eq!(script["kind"], "WorkspaceSource");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_case_clash_in_a_folder_layer_is_reported() {
    let dir = tmp("case-clash");
    write(&dir, "config.cpp", "class First {};");
    write(&dir, "Config.cpp", "class Second {};");

    let payload = list(json!([
        { "id": "src", "kind": "WorkspaceSource", "path": dir },
    ]));

    assert_eq!(
        payload["case_clashes"],
        json!([{ "layer": "src", "kept": "Config.cpp", "dropped": "config.cpp" }]),
        "got {payload}"
    );
    let entries = payload["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "got {payload}");
    assert_eq!(entries[0]["rel"], "Config.cpp");

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_with_a_non_utf8_name_is_skipped() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    struct Unlock(PathBuf, PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }

    let dir = tmp("non-utf8-locked");
    write(&dir, "ok.c", "");
    let locked = dir.join(std::ffi::OsStr::from_bytes(b"b\xFF"));
    std::fs::create_dir(&locked).unwrap();
    let _unlock = Unlock(locked.clone(), dir.clone());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        eprintln!("skipped: the folder is still readable (running as root)");
        return;
    }

    let payload = list(json!([
        { "id": "src", "kind": "WorkspaceSource", "path": dir },
    ]));

    let skipped = payload["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "got {payload}");
    assert!(skipped[0]["path"].as_str().unwrap().ends_with("b\u{FFFD}"));
    assert_eq!(entry(&payload, "ok.c")["layer"], "src");
}

#[test]
fn a_higher_layer_shadows_the_lower_one() {
    let vanilla = tmp("shadow-vanilla");
    let source = tmp("shadow-source");
    write(&vanilla, "a.txt", "vanilla");
    write(&source, "a.txt", "source");

    let payload = list(json!([
        { "id": "vanilla", "kind": "VanillaGame", "path": vanilla },
        { "id": "source", "kind": "WorkspaceSource", "path": source },
    ]));

    assert_eq!(
        payload["entries"].as_array().unwrap().len(),
        1,
        "got {payload}"
    );
    let won = entry(&payload, "a.txt");
    assert_eq!(won["layer"], "source");
    assert_eq!(won["kind"], "WorkspaceSource");
    assert_eq!(won["shadowed_by"], json!(["vanilla"]));

    let _ = std::fs::remove_dir_all(&vanilla);
    let _ = std::fs::remove_dir_all(&source);
}

#[test]
fn an_addons_directory_mounts_each_pbo_as_its_own_layer_in_file_name_order() {
    let mod_root = tmp("addons");
    let addons = mod_root.join("addons");
    pack(
        &addons,
        "b.pbo",
        &[("Scripts\\B.c", b"class B {};"), ("config.cpp", b"x")],
    );
    pack(&addons, "a.pbo", &[("Scripts\\A.c", b"class A {};")]);

    let payload = list(json!([
        { "id": "van", "kind": "VanillaGame", "path": addons },
    ]));

    assert_eq!(
        payload["layers"],
        json!([
            { "id": "van/a", "kind": "VanillaGame" },
            { "id": "van/b", "kind": "VanillaGame" },
        ]),
        "got {payload}"
    );
    let a = entry(&payload, "Scripts/A.c");
    assert_eq!(a["layer"], "van/a");
    assert_eq!(a["kind"], "VanillaGame");
    let b = entry(&payload, "Scripts/B.c");
    assert_eq!(b["layer"], "van/b");
    let cfg = entry(&payload, "config.cpp");
    assert_eq!(cfg["layer"], "van/b");
    assert_eq!(payload["skipped"], json!([]));

    let _ = std::fs::remove_dir_all(&mod_root);
}

#[test]
fn a_corrupt_pbo_is_skipped_and_the_other_layers_still_list() {
    let source = tmp("corrupt-src");
    write(&source, "config.cpp", "class CfgPatches {};");
    let addons = tmp("corrupt").join("addons");
    std::fs::create_dir_all(&addons).unwrap();
    let corrupt = addons.join("broken.pbo");
    std::fs::write(&corrupt, b"this is not a pbo").unwrap();

    let payload = list(json!([
        { "id": "src", "kind": "WorkspaceSource", "path": source },
        { "id": "van", "kind": "VanillaGame", "path": addons },
    ]));

    assert!(
        entry(&payload, "config.cpp")["layer"] == "src",
        "got {payload}"
    );
    assert_eq!(
        payload["layers"],
        json!([{ "id": "src", "kind": "WorkspaceSource" }]),
        "a failed PBO must not be listed as a layer: {payload}"
    );
    let skipped = payload["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "got {payload}");
    assert!(skipped[0]["path"].as_str().unwrap().ends_with("broken.pbo"));
    assert!(!skipped[0]["error"].as_str().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&addons);
}

#[test]
fn a_missing_layer_is_skipped_and_the_other_layers_still_list() {
    let source = tmp("missing-src");
    write(&source, "config.cpp", "class CfgPatches {};");
    let missing = tmp("missing").join("nope.pbo");

    let payload = list(json!([
        { "id": "x", "kind": "WorkshopMod", "path": missing },
        { "id": "src", "kind": "WorkspaceSource", "path": source },
    ]));

    assert_eq!(entry(&payload, "config.cpp")["layer"], "src");
    let skipped = payload["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "got {payload}");
    assert_eq!(skipped[0]["path"], json!(missing));
    let _ = std::fs::remove_dir_all(&source);
}

#[test]
fn a_vanilla_addons_layer_mounts_each_map_folder_addons_pbo() {
    let game = tmp("map-layers");
    pack(
        &game.join("Addons"),
        "a.pbo",
        &[("Scripts\\A.c", b"class A {};")],
    );
    pack(
        &game.join("nasdara/addons"),
        "n.pbo",
        &[("Scripts\\N.c", b"class N {};")],
    );
    write(&game, "nasdara/addons/n.pbo.dayz.bisign", "");

    let vanilla = list(json!([
        { "id": "van", "kind": "VanillaGame", "path": game.join("addons") },
    ]));
    assert_eq!(
        vanilla["layers"],
        json!([
            { "id": "van/a", "kind": "VanillaGame" },
            { "id": "van/nasdara/n", "kind": "VanillaGame" },
        ]),
        "got {vanilla}"
    );
    assert_eq!(entry(&vanilla, "Scripts/A.c")["layer"], "van/a");
    assert_eq!(entry(&vanilla, "Scripts/N.c")["layer"], "van/nasdara/n");
    assert_eq!(vanilla["skipped"], json!([]));

    let workshop = list(json!([
        { "id": "mod", "kind": "WorkshopMod", "path": game.join("addons") },
    ]));
    assert_eq!(
        workshop["layers"],
        json!([{ "id": "mod/a", "kind": "WorkshopMod" }]),
        "got {workshop}"
    );
    assert!(
        workshop["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["rel"] != "Scripts/N.c"),
        "a non-vanilla layer must not mount map folders: {workshop}"
    );

    let _ = std::fs::remove_dir_all(&game);
}
