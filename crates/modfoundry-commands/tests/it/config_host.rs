//! `ConfigHost` and the `config.*` index commands: the vanilla/dependency/
//! workspace load behind `config.index`, and `config.patches` (source ids and
//! vpaths).

use std::path::{Path, PathBuf};

use crate::common::{self, config_bin, pack, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_commands::config_host::{config_source_id, config_vpath};
use modfoundry_config::resolve::ConfigOrigin;
use modfoundry_core::{Actor, CommandError};
use serde_json::{json, Value};

const VANILLA_CFG: &str = "\
class CfgPatches { class DZ_A { requiredAddons[] = { \"DZ_Data\" }; }; };
class CfgVehicles { class Base_A { scope = 1; weight = 5; class DamageSystem {}; }; };
";
const NESTED_CFG: &str = "class CfgVehicles { class Nested_A { scope = 0; }; };\n";
const CORE_CFG: &str = "class CfgVehicles { class Core_A {}; };\n";
const DEP_CFG: &str = "\
class CfgPatches { class DZ_Dep { requiredAddons[] = { \"DZ_A\" }; }; };
class CfgVehicles { class Dep_A: Base_A {}; };
";
const WORKSPACE_CFG: &str = "\
class CfgPatches { class FieldKit_Data { requiredAddons[] = { \"DZ_A\" }; }; };
class CfgVehicles { class FieldKit_Canteen: Base_A { scope = 2; }; };
";

struct Fixture {
    root: PathBuf,
    game: PathBuf,
    dep: PathBuf,
    dep_pbo: PathBuf,
}

/// A fake install (`Addons/a.pbo` with a nested config plus `dta/bin.pbo`), a
/// dependency (`@Dep/addons/d.pbo`) and a workspace `config.cpp`.
fn fixture(label: &str) -> Fixture {
    let root = tmp(label);
    write(&root, "config.cpp", WORKSPACE_CFG);
    write(&root, "mod.cpp", "name = \"Field Kit\";\n");

    let game = tmp(&format!("{label}-game"));
    let a = config_bin(VANILLA_CFG);
    let nested = config_bin(NESTED_CFG);
    pack(
        &game.join("Addons/a.pbo"),
        &[("prefix", "dz/a")],
        &[
            ("config.bin", a.as_slice()),
            ("sub\\config.bin", nested.as_slice()),
        ],
    );
    let core = config_bin(CORE_CFG);
    pack(
        &game.join("dta/bin.pbo"),
        &[],
        &[("config.bin", core.as_slice())],
    );

    let dep = tmp(&format!("{label}-dep"));
    write(&dep, "mod.cpp", "name = \"Dep Mod\";\n");
    let d = config_bin(DEP_CFG);
    let dep_pbo = dep.join("addons/d.pbo");
    pack(
        &dep_pbo,
        &[("prefix", "dep")],
        &[("config.bin", d.as_slice())],
    );

    Fixture {
        root,
        game,
        dep,
        dep_pbo,
    }
}

fn index_args(fix: &Fixture) -> Value {
    json!({ "root": fix.root, "dependencies": [fix.dep], "game": fix.game })
}

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

fn sources(report: &Value) -> Vec<(String, usize)> {
    report["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["origin"].as_str().unwrap().to_string(),
                s["configs"].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

fn configs_total(report: &Value) -> usize {
    sources(report).iter().map(|(_, n)| n).sum()
}

#[test]
fn index_reports_every_origin_its_configs_patches_and_classes() {
    let fix = fixture("index");
    let core = common::core();
    let report = dispatch(&core, "config.index", index_args(&fix));

    assert_eq!(
        sources(&report),
        vec![
            ("vanilla".to_string(), 3),
            ("dependency:0".to_string(), 1),
            ("workspace".to_string(), 1),
        ],
        "{report}"
    );
    assert_eq!(report["sources"][1]["path"], json!(fix.dep));
    assert_eq!(report["sources"][1]["name"], "Dep Mod");
    assert_eq!(report["sources"][2]["path"], json!(fix.root));
    assert_eq!(report["sources"][2]["name"], Value::Null);
    assert_eq!(configs_total(&report), 5);
    assert_eq!(report["patches"], 3, "{report}");
    assert_eq!(report["classes"], 17, "{report}");
    assert!(report["millis"].is_u64(), "{report}");
    assert_eq!(report["warnings"], json!([]), "{report}");
    assert_eq!(report["cycles"], json!([]), "{report}");
}

#[test]
fn vanilla_configs_include_signed_dlc_map_folders() {
    let root = tmp("map-folders-root");
    std::fs::create_dir_all(&root).unwrap();
    let game = tmp("map-folders-game");

    let base = config_bin(VANILLA_CFG);
    pack(
        &game.join("Addons/a.pbo"),
        &[],
        &[("config.bin", base.as_slice())],
    );
    let dta = config_bin(CORE_CFG);
    pack(
        &game.join("dta/bin.pbo"),
        &[],
        &[("config.bin", dta.as_slice())],
    );
    let map = config_bin("class CfgVehicles { class Map_A {}; };\n");
    pack(
        &game.join("nasdara/addons/m.pbo"),
        &[],
        &[("config.bin", map.as_slice())],
    );
    std::fs::write(game.join("nasdara/addons/m.pbo.dayz.bisign"), b"signature").unwrap();
    let staged = config_bin("class CfgVehicles { class Staged_A {}; };\n");
    pack(
        &game.join("@Staged/addons/s.pbo"),
        &[],
        &[("config.bin", staged.as_slice())],
    );
    std::fs::write(game.join("@Staged/addons/s.pbo.dayz.bisign"), b"signature").unwrap();
    let tools = config_bin("class CfgVehicles { class Tools_A {}; };\n");
    pack(
        &game.join("Tools/addons/t.pbo"),
        &[],
        &[("config.bin", tools.as_slice())],
    );

    let core = common::core();
    let report = dispatch(&core, "config.index", json!({ "root": root, "game": game }));

    assert_eq!(
        sources(&report),
        vec![("vanilla".to_string(), 3), ("workspace".to_string(), 0)],
        "{report}"
    );
    let resolve = |name: &str| {
        dispatch(
            &core,
            "config.resolve",
            json!({ "source": "vanilla:a.pbo:config.bin", "class": ["CfgVehicles", name] }),
        )
    };
    let map_class = resolve("Map_A");
    assert_eq!(
        map_class["defined_by"],
        json!([{ "id": "vanilla:m.pbo:config.bin", "display": "m.pbo", "origin": "vanilla" }]),
        "{map_class}"
    );
    for name in ["Staged_A", "Tools_A"] {
        assert!(resolve(name)["class"].is_null(), "{name} is not vanilla");
    }
}

#[test]
fn a_second_index_replaces_the_first_instead_of_accumulating() {
    let fix = fixture("supersede");
    let empty = tmp("supersede-empty");
    std::fs::create_dir_all(&empty).unwrap();
    let core = common::core();

    let first = dispatch(&core, "config.index", index_args(&fix));
    assert_eq!(configs_total(&first), 5);
    let second = dispatch(
        &core,
        "config.index",
        json!({ "root": empty, "game": fix.game }),
    );
    assert_eq!(
        sources(&second),
        vec![("vanilla".to_string(), 3), ("workspace".to_string(), 0),],
        "the dependency source is gone, not accumulated: {second}"
    );
}

#[test]
fn a_corrupt_pbo_and_an_unreadable_bin_are_warnings_and_an_ebo_is_ignored() {
    let fix = fixture("warnings");
    std::fs::write(fix.game.join("Addons/corrupt.pbo"), b"not a pbo at all").unwrap();
    let broken = b"not a rap config.bin".to_vec();
    pack(
        &fix.game.join("Addons/broken.pbo"),
        &[("prefix", "broken")],
        &[("config.bin", broken.as_slice())],
    );
    std::fs::write(fix.game.join("Addons/protected.ebo"), b"not a pbo").unwrap();

    let core = common::core();
    let report = dispatch(&core, "config.index", index_args(&fix));
    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(warnings.len(), 2, "{report}");
    assert!(warnings[0].contains("corrupt.pbo"), "{report}");
    assert!(warnings[1].contains("broken.pbo"), "{report}");
    assert_eq!(
        configs_total(&report),
        6,
        "a.pbo 2, dta 1, the unreadable bin 1, the dependency 1, the workspace 1: {report}"
    );
}

#[test]
fn patches_reads_one_mod_without_the_index() {
    let fix = fixture("patches");
    let core = common::core();

    let dir = dispatch(&core, "config.patches", json!({ "root": fix.dep }));
    assert_eq!(dir["root"], json!(fix.dep));
    assert_eq!(dir["name"], "Dep Mod");
    assert_eq!(
        dir["patches"],
        json!([{
            "name": "DZ_Dep",
            "source": "dependency:0:d.pbo:config.bin",
            "required_addons": ["DZ_A"],
        }]),
        "{dir}"
    );

    let single = dispatch(&core, "config.patches", json!({ "root": fix.dep_pbo }));
    assert_eq!(single["name"], Value::Null);
    assert_eq!(single["patches"], dir["patches"]);

    let source = dispatch(&core, "config.patches", json!({ "root": fix.root }));
    assert_eq!(source["name"], "Field Kit");
    assert_eq!(
        source["patches"],
        json!([{
            "name": "FieldKit_Data",
            "source": "workspace:config.cpp",
            "required_addons": ["DZ_A"],
        }]),
        "{source}"
    );

    let missing = tmp("patches-missing");
    let err = core
        .dispatch(Actor::User, "config.patches", json!({ "root": missing }))
        .unwrap_err();
    let CommandError::Failed(message) = err else {
        panic!("expected Failed, got {err:?}");
    };
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
}

#[test]
fn patches_names_an_unreadable_pbo_and_config_as_warnings() {
    let dir = tmp("patches-warnings");
    write(&dir, "addons/corrupt.pbo", "not a pbo at all");
    pack(
        &dir.join("addons/broken.pbo"),
        &[("prefix", "broken")],
        &[("config.bin", b"not a rap config.bin".as_slice())],
    );

    let core = common::core();
    let report = dispatch(&core, "config.patches", json!({ "root": dir }));
    assert_eq!(report["patches"], json!([]), "{report}");
    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(warnings.len(), 2, "{report}");
    assert!(
        warnings.iter().any(|w| w.contains("corrupt.pbo")),
        "{report}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("broken.pbo")),
        "{report}"
    );
}

#[test]
fn index_keeps_two_vanilla_pbos_with_one_name() {
    const ADDON: &str = "class CfgVehicles { class DataThing {}; };\n";
    const MAP: &str = "class CfgWorlds { class Sakhal {}; };\n";

    let root = tmp("same-name-root");
    std::fs::create_dir_all(&root).unwrap();
    let game = tmp("same-name-game");
    pack(
        &game.join("addons/data.pbo"),
        &[("prefix", "data")],
        &[("config.bin", config_bin(ADDON).as_slice())],
    );
    pack(
        &game.join("sakhal/addons/data.pbo"),
        &[("prefix", "sakhal")],
        &[("config.bin", config_bin(MAP).as_slice())],
    );
    std::fs::write(
        game.join("sakhal/addons/data.pbo.dayz.bisign"),
        b"signature",
    )
    .unwrap();
    std::fs::create_dir_all(game.join("dta")).unwrap();

    let core = common::core();
    let report = dispatch(&core, "config.index", json!({ "root": root, "game": game }));

    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(warnings.len(), 1, "{report}");
    assert!(warnings[0].contains("data.pbo"), "{report}");

    let defined_by = |source: &str, class: Value| {
        let resolved = dispatch(
            &core,
            "config.resolve",
            json!({ "source": source, "class": class }),
        );
        let defined = resolved["defined_by"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        (defined[0]["id"].clone(), defined.len())
    };
    assert_eq!(
        defined_by(
            "vanilla:data.pbo:config.bin",
            json!(["CfgVehicles", "DataThing"])
        ),
        (json!("vanilla:data.pbo:config.bin"), 1)
    );
    assert_eq!(
        defined_by(
            "vanilla:data.pbo:config.bin",
            json!(["CfgWorlds", "Sakhal"])
        ),
        (json!("vanilla:data.pbo:config.bin#2"), 1),
        "the second PBO keeps its own source"
    );
}

#[test]
fn index_writes_an_audit_row_and_patches_does_not() {
    let fix = fixture("audit");
    let broker = Broker::in_memory(common::core()).unwrap();
    for _ in 0..3 {
        broker
            .dispatch(Actor::User, "config.patches", json!({ "root": fix.dep }))
            .unwrap();
    }
    assert!(broker.audit(None, 10).unwrap().is_empty());

    broker
        .dispatch(Actor::User, "config.index", index_args(&fix))
        .unwrap();
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);
}

#[test]
fn source_ids_name_origin_container_and_entry() {
    assert_eq!(
        config_source_id(
            ConfigOrigin::Vanilla,
            Path::new("/game/Addons/Gear_Drinks.PBO"),
            Some("Bos_Taurus\\Config.BIN"),
        ),
        "vanilla:Gear_Drinks.PBO:bos_taurus/config.bin"
    );
    assert_eq!(
        config_source_id(
            ConfigOrigin::Dependency(2),
            Path::new("/deps/@Dep/addons/d.pbo"),
            Some("config.bin"),
        ),
        "dependency:2:d.pbo:config.bin"
    );
    assert_eq!(
        config_source_id(
            ConfigOrigin::Workspace,
            Path::new("Scripts/Foo/config.cpp"),
            None
        ),
        "workspace:Scripts/Foo/config.cpp"
    );
    assert_eq!(
        config_source_id(
            ConfigOrigin::Dependency(0),
            Path::new("Scripts\\a.cpp"),
            None
        ),
        "dependency:0:Scripts/a.cpp"
    );
}

#[test]
fn a_pbo_vpath_is_the_path_the_vfs_index_records() {
    let dir = tmp("vpath");
    let pbo = dir.join("gear.pbo");
    pack(
        &pbo,
        &[("prefix", "dz\\gear")],
        &[("Canteen\\Config.BIN", b"x".as_slice())],
    );

    let backend = modfoundry_vfs::pbo::PboBackend::open(&pbo).unwrap();
    let indexed: Vec<&str> = backend.index().paths().collect();
    assert_eq!(indexed, vec!["dz/gear/Canteen/Config.BIN"]);
    assert_eq!(
        config_vpath(Some("dz\\gear"), b"Canteen\\Config.BIN").as_deref(),
        Some("dz/gear/Canteen/Config.BIN")
    );
    assert_eq!(
        config_vpath(Some("dz\\gear"), b"config.bin").as_deref(),
        Some("dz/gear/config.bin")
    );
    assert_eq!(
        config_vpath(None, b"config.bin").as_deref(),
        Some("config.bin")
    );
    assert_eq!(config_vpath(None, b"../config.bin"), None);
}

fn indexed(label: &str) -> (Fixture, modfoundry_core::Core) {
    let fix = fixture(label);
    let core = common::core();
    dispatch(&core, "config.index", index_args(&fix));
    (fix, core)
}

fn canteen(fix: &Fixture) -> Value {
    json!({
        "path": fix.root.join("config.cpp"),
        "class": ["CfgVehicles", "FieldKit_Canteen"],
    })
}

fn property<'a>(resolved: &'a Value, name: &str) -> &'a Value {
    resolved["class"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no property {name}: {resolved}"))
}

#[test]
fn resolve_reports_inherited_rows_with_their_provenance() {
    let (fix, core) = indexed("resolve-rows");
    let resolved = dispatch(&core, "config.resolve", canteen(&fix));

    assert_eq!(resolved["ready"], true, "{resolved}");
    assert_eq!(
        resolved["class"]["path"],
        json!(["CfgVehicles", "FieldKit_Canteen"])
    );
    let scope = property(&resolved, "scope");
    assert_eq!(scope["value"], "2");
    assert_eq!(scope["own"], true);
    assert_eq!(scope["from_source"]["id"], "workspace:config.cpp");
    assert_eq!(scope["from_source"]["origin"], "workspace");

    let weight = property(&resolved, "weight");
    assert_eq!(weight["value"], "5");
    assert_eq!(weight["own"], false);
    assert_eq!(weight["from_class"], json!(["CfgVehicles", "Base_A"]));
    assert_eq!(
        weight["from_source"],
        json!({
            "id": "vanilla:a.pbo:config.bin",
            "display": "a.pbo",
            "vpath": "dz/a/config.bin",
            "origin": "vanilla",
        })
    );
    let child = &resolved["class"]["children"][0];
    assert_eq!(child["name"], "DamageSystem");
    assert_eq!(child["own"], false);
}

#[cfg(unix)]
#[test]
fn a_relative_index_root_still_resolves_an_absolute_path_to_its_indexed_source() {
    let fix = fixture("relative-root");
    let cwd = std::env::current_dir().unwrap();
    let up: PathBuf = std::iter::repeat_n("..", cwd.ancestors().count() - 1).collect();
    let root = up.join(fix.root.strip_prefix("/").unwrap());
    let core = common::core();
    dispatch(
        &core,
        "config.index",
        json!({ "root": root, "dependencies": [fix.dep], "game": fix.game }),
    );

    let resolved = dispatch(&core, "config.resolve", canteen(&fix));
    assert_eq!(
        property(&resolved, "scope")["from_source"]["id"],
        "workspace:config.cpp",
        "{resolved}"
    );
    assert_eq!(
        resolved["defined_by"],
        json!([{ "id": "workspace:config.cpp", "display": "config.cpp", "origin": "workspace" }]),
        "{resolved}"
    );
}

#[test]
fn resolve_reports_the_owner_patch_required_addons_and_where_the_base_lives() {
    let (fix, core) = indexed("resolve-details");
    let resolved = dispatch(&core, "config.resolve", canteen(&fix));

    assert_eq!(resolved["owner_patch"], "FieldKit_Data");
    assert_eq!(resolved["required_addons"], json!(["DZ_A"]));
    assert_eq!(resolved["base_source"]["id"], "vanilla:a.pbo:config.bin");
    assert_eq!(resolved["base_source"]["vpath"], "dz/a/config.bin");
    assert_eq!(resolved["base_owner_patch"], "DZ_A");
    assert_eq!(resolved["base_owner_required"], true);
    assert_eq!(
        resolved["defined_by"],
        json!([{ "id": "workspace:config.cpp", "display": "config.cpp", "origin": "workspace" }])
    );
    assert_eq!(resolved["unresolved_base"], Value::Null);
}

#[test]
fn a_base_outside_required_addons_is_reported_as_not_required() {
    let (fix, core) = indexed("resolve-required");
    let text = WORKSPACE_CFG.replace("{ \"DZ_A\" }", "{}");
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({
            "path": fix.root.join("config.cpp"),
            "text": text,
            "class": ["CfgVehicles", "FieldKit_Canteen"],
        }),
    );

    assert_eq!(resolved["required_addons"], json!([]));
    assert_eq!(resolved["base_owner_required"], false, "{resolved}");
}

#[test]
fn an_unresolved_base_is_named() {
    let (fix, core) = indexed("resolve-missing");
    let text = WORKSPACE_CFG.replace("Base_A", "Nowhere_A");
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({
            "path": fix.root.join("config.cpp"),
            "text": text,
            "class": ["CfgVehicles", "FieldKit_Canteen"],
        }),
    );

    assert_eq!(resolved["unresolved_base"], "Nowhere_A", "{resolved}");
    assert_eq!(resolved["base_source"], Value::Null);
}

#[test]
fn values_are_rendered_as_text() {
    let (fix, core) = indexed("resolve-values");
    let text = "\
class CfgPatches { class FieldKit_Data {}; };
class CfgVehicles { class FieldKit_Canteen { model = \"a\\b.p3d\"; ratio = 0.5; kind = Drink; list[] = { \"x\", 2, { 3 } }; }; };
";
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({
            "path": fix.root.join("config.cpp"),
            "text": text,
            "class": ["CfgVehicles", "FieldKit_Canteen"],
        }),
    );

    assert_eq!(property(&resolved, "model")["value"], "a\\b.p3d");
    assert_eq!(property(&resolved, "ratio")["value"], "0.5");
    assert_eq!(property(&resolved, "kind")["value"], "Drink");
    let list = property(&resolved, "list");
    assert_eq!(list["value"], "{x, 2, {3}}");
    assert_eq!(list["array"], true);

    assert_eq!(property(&resolved, "model")["kind"], "str");
    assert_eq!(property(&resolved, "model")["items"], json!([]));
    assert_eq!(property(&resolved, "ratio")["kind"], "number");
    assert_eq!(property(&resolved, "kind")["kind"], "ident");
    assert_eq!(list["kind"], "array");
    assert_eq!(
        list["items"],
        json!([
            { "kind": "str", "text": "x" },
            { "kind": "number", "text": "2" },
            { "kind": "array", "text": "{3}" },
        ])
    );
}

#[test]
fn without_a_class_resolve_lists_the_file_externals_with_their_origin() {
    let (fix, core) = indexed("resolve-externals");
    let text = "\
class CfgVehicles { class Base_A; class Core_A; class Ghost_A; class FieldKit_X: Base_A {}; };
";
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({ "path": fix.root.join("config.cpp"), "text": text }),
    );

    assert_eq!(resolved["ready"], true);
    assert_eq!(
        resolved["externals"],
        json!([
            { "path": ["CfgVehicles", "Base_A"], "origin": "vanilla", "source_display": "a.pbo" },
            { "path": ["CfgVehicles", "Core_A"], "origin": "vanilla", "source_display": "bin.pbo" },
            { "path": ["CfgVehicles", "Ghost_A"], "origin": null, "source_display": null },
        ]),
        "{resolved}"
    );
}

#[test]
fn the_text_overlay_changes_the_answer_without_touching_the_disk() {
    let (fix, core) = indexed("resolve-overlay");
    let overlaid = WORKSPACE_CFG.replace("scope = 2", "scope = 9");
    let mut args = canteen(&fix);
    args["text"] = json!(overlaid);

    let with_text = dispatch(&core, "config.resolve", args);
    assert_eq!(property(&with_text, "scope")["value"], "9");

    let without = dispatch(&core, "config.resolve", canteen(&fix));
    assert_eq!(property(&without, "scope")["value"], "2");
    assert_eq!(
        std::fs::read_to_string(fix.root.join("config.cpp")).unwrap(),
        WORKSPACE_CFG
    );
}

#[test]
fn a_file_outside_the_index_resolves_from_its_text() {
    let (fix, core) = indexed("resolve-unindexed");
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({
            "path": fix.root.join("new/config.cpp"),
            "text": "class CfgVehicles { class Fresh: Base_A { scope = 3; }; };\n",
            "class": ["CfgVehicles", "Fresh"],
        }),
    );

    assert_eq!(property(&resolved, "scope")["value"], "3");
    assert_eq!(
        property(&resolved, "weight")["from_source"]["origin"],
        "vanilla"
    );
}

#[test]
fn a_missing_class_resolves_to_none() {
    let (fix, core) = indexed("resolve-absent");
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({ "path": fix.root.join("config.cpp"), "class": ["CfgVehicles", "Absent"] }),
    );

    assert_eq!(resolved["ready"], true);
    assert_eq!(resolved["class"], Value::Null, "{resolved}");
}

#[test]
fn resolve_is_not_ready_before_an_index_and_never_blocks() {
    let fix = fixture("resolve-cold");
    let core = common::core();
    let resolved = dispatch(&core, "config.resolve", canteen(&fix));

    assert_eq!(resolved["ready"], false);
    assert!(
        resolved.get("class").is_none_or(Value::is_null),
        "{resolved}"
    );
    let bare = dispatch(
        &core,
        "config.resolve",
        json!({ "path": fix.root.join("config.cpp") }),
    );
    assert_eq!(bare["ready"], false);
    assert_eq!(bare["externals"], json!([]));
}

#[test]
fn source_by_id_and_by_vpath_round_trips_a_config_bin() {
    let (_fix, core) = indexed("source-bin");
    let expected =
        modfoundry_config::to_text(&modfoundry_config::read_rap(&config_bin(VANILLA_CFG)).unwrap());

    let by_id = dispatch(
        &core,
        "config.source",
        json!({ "id": "vanilla:a.pbo:config.bin" }),
    );
    assert_eq!(by_id["text"], json!(expected));
    assert_eq!(by_id["id"], "vanilla:a.pbo:config.bin");
    assert_eq!(by_id["display"], "a.pbo");
    assert_eq!(by_id["vpath"], "dz/a/config.bin");
    assert_eq!(by_id["origin"], "vanilla");

    let by_vpath = dispatch(
        &core,
        "config.source",
        json!({ "vpath": "DZ/A/Config.BIN" }),
    );
    assert_eq!(by_vpath, by_id);
}

#[test]
fn source_returns_a_config_cpp_verbatim() {
    let (_fix, core) = indexed("source-cpp");
    let source = dispatch(
        &core,
        "config.source",
        json!({ "id": "workspace:config.cpp" }),
    );

    assert_eq!(source["text"], WORKSPACE_CFG);
    assert_eq!(source["origin"], "workspace");
}

#[test]
fn the_last_loaded_source_wins_a_shared_vpath() {
    let fix = fixture("source-shadow");
    let shadow = tmp("source-shadow-dep");
    let cfg = config_bin(
        "class CfgPatches { class DZ_Shadow { requiredAddons[] = { \"DZ_A\" }; }; };\nclass CfgVehicles { class Shadow_A {}; };\n",
    );
    pack(
        &shadow.join("addons/s.pbo"),
        &[("prefix", "dz/a")],
        &[("config.bin", cfg.as_slice())],
    );
    let core = common::core();
    dispatch(
        &core,
        "config.index",
        json!({ "root": fix.root, "dependencies": [shadow], "game": fix.game }),
    );

    let source = dispatch(
        &core,
        "config.source",
        json!({ "vpath": "dz/a/config.bin" }),
    );

    assert_eq!(source["id"], "dependency:0:s.pbo:config.bin", "{source}");
}

#[test]
fn source_refuses_an_unknown_id_or_vpath_and_a_cold_index() {
    let fix = fixture("source-refuse");
    let core = common::core();
    assert!(core
        .dispatch(Actor::User, "config.source", json!({ "id": "x" }))
        .is_err());

    dispatch(&core, "config.index", index_args(&fix));
    for args in [
        json!({ "id": "nope" }),
        json!({ "vpath": "no/such.bin" }),
        json!({}),
    ] {
        let err = core
            .dispatch(Actor::User, "config.source", args.clone())
            .unwrap_err();
        assert!(
            matches!(err, CommandError::InvalidArguments(_)),
            "should refuse {args} as invalid arguments, got {err:?}"
        );
    }
}

#[test]
fn resolve_and_source_write_no_audit_rows() {
    let (fix, _) = indexed("resolve-audit");
    let broker = Broker::in_memory(common::core()).unwrap();
    broker
        .dispatch(Actor::User, "config.index", index_args(&fix))
        .unwrap();
    let before = broker.audit(None, 50).unwrap().len();
    broker
        .dispatch(Actor::User, "config.resolve", canteen(&fix))
        .unwrap();
    broker
        .dispatch(
            Actor::User,
            "config.source",
            json!({ "id": "workspace:config.cpp" }),
        )
        .unwrap();
    assert_eq!(broker.audit(None, 50).unwrap().len(), before);
}

fn codes(diagnostics: &Value) -> Vec<String> {
    diagnostics
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn validate_adds_c410_only_once_the_index_is_ready() {
    let fix = fixture("validate-c410");
    let core = common::core();
    let text = WORKSPACE_CFG.replace("\"DZ_A\"", "\"DZ_A\", \"Missing_Mod\"");
    let args = json!({ "path": fix.root.join("config.cpp"), "text": text });

    let cold = dispatch(&core, "config.validate", args.clone());
    assert!(!codes(&cold).contains(&"MF-C410".to_string()), "{cold}");

    dispatch(&core, "config.index", index_args(&fix));
    let warm = dispatch(&core, "config.validate", args);
    assert_eq!(
        codes(&warm).iter().filter(|c| *c == "MF-C410").count(),
        1,
        "{warm}"
    );
    let c410 = warm
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "MF-C410")
        .unwrap();
    assert!(c410["message"].as_str().unwrap().contains("`Missing_Mod`"));
}

#[test]
fn validate_output_is_unchanged_when_the_config_is_clean_against_the_index() {
    let fix = fixture("validate-clean");
    let core = common::core();
    let path = fix.root.join("config.cpp");
    let cold = dispatch(&core, "config.validate", json!({ "path": path }));

    dispatch(&core, "config.index", index_args(&fix));
    let warm = dispatch(&core, "config.validate", json!({ "path": path }));

    assert_eq!(warm, cold);
}

#[test]
fn validate_checks_the_file_on_disk_when_no_text_is_given() {
    let fix = fixture("validate-disk");
    let core = common::core();
    dispatch(&core, "config.index", index_args(&fix));
    write(
        &fix.root,
        "config.cpp",
        &WORKSPACE_CFG.replace("\"DZ_A\"", "\"Gone_Mod\""),
    );

    let found = dispatch(
        &core,
        "config.validate",
        json!({ "path": fix.root.join("config.cpp") }),
    );

    assert!(codes(&found).contains(&"MF-C410".to_string()), "{found}");
}

#[test]
fn a_name_another_indexed_source_declares_is_not_flagged() {
    let fix = fixture("validate-dep");
    let core = common::core();
    dispatch(&core, "config.index", index_args(&fix));
    let text = WORKSPACE_CFG.replace("\"DZ_A\"", "\"DZ_A\", \"DZ_Dep\", \"DZ_Nowhere\"");

    let found = dispatch(
        &core,
        "config.validate",
        json!({ "path": fix.root.join("config.cpp"), "text": text }),
    );

    let c410: Vec<_> = found
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "MF-C410")
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        c410.len(),
        1,
        "only DZ_Nowhere (named by nothing in the fixture) is unknown: {c410:?}"
    );
    assert!(c410[0].contains("DZ_Nowhere"));
}

#[test]
fn corpus_vanilla_index_meets_the_target() {
    let Some(game) = std::env::var_os("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL is not set");
        return;
    };
    let root = tmp("corpus-ws");
    std::fs::create_dir_all(&root).unwrap();
    let core = common::core();

    let started = std::time::Instant::now();
    let report = dispatch(
        &core,
        "config.index",
        json!({ "root": root, "game": PathBuf::from(game) }),
    );
    let wall = started.elapsed().as_millis() as u64;
    let millis = report["millis"].as_u64().unwrap();
    println!(
        "config.index over the real install: {millis} ms (load), {wall} ms (wall), \
         {} configs, {} patches, {} classes, {} warnings",
        configs_total(&report),
        report["patches"],
        report["classes"],
        report["warnings"].as_array().unwrap().len(),
    );

    assert_eq!(configs_total(&report), 267, "{report}");
    assert_eq!(report["patches"], 266, "{report}");
    assert_eq!(report["classes"], 169_513, "{report}");
    assert_eq!(report["warnings"], json!([]), "{report}");
    assert!(millis <= 2000, "{millis} ms exceeds the 2 s target");
}

#[test]
fn resolve_by_source_uses_that_sources_own_identity() {
    let (fix, core) = indexed("resolve-source");
    let by_source = dispatch(
        &core,
        "config.resolve",
        json!({
            "source": "vanilla:a.pbo:config.bin",
            "class": ["CfgVehicles", "Base_A"],
        }),
    );

    assert_eq!(by_source["ready"], true, "{by_source}");
    assert_eq!(by_source["class"]["path"], json!(["CfgVehicles", "Base_A"]));
    let scope = property(&by_source, "scope");
    assert_eq!(scope["value"], "1");
    assert_eq!(scope["own"], true);
    assert_eq!(scope["from_source"]["id"], "vanilla:a.pbo:config.bin");
    assert_eq!(by_source["owner_patch"], "DZ_A");
    assert_eq!(by_source["required_addons"], json!(["DZ_Data"]));
    assert_eq!(
        by_source["defined_by"],
        json!([{ "id": "vanilla:a.pbo:config.bin", "display": "a.pbo", "origin": "vanilla" }])
    );

    let by_path = dispatch(&core, "config.resolve", canteen(&fix));
    let mut source_keys: Vec<&str> = by_source
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut path_keys: Vec<&str> = by_path
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    source_keys.sort();
    path_keys.sort();
    assert_eq!(
        source_keys, path_keys,
        "the source form reports the same fields"
    );
}

#[test]
fn resolve_by_source_lists_that_sources_forward_declarations() {
    let fix = fixture("resolve-source-externals");
    write(
        &fix.root,
        "extra/config.cpp",
        "class CfgVehicles { class Base_A; class Ghost_A; };\n",
    );
    let core = common::core();
    dispatch(&core, "config.index", index_args(&fix));

    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({ "source": "workspace:extra/config.cpp" }),
    );

    assert_eq!(
        resolved["externals"],
        json!([
            { "path": ["CfgVehicles", "Base_A"], "origin": "vanilla", "source_display": "a.pbo" },
            { "path": ["CfgVehicles", "Ghost_A"], "origin": null, "source_display": null },
        ]),
        "{resolved}"
    );
}

#[test]
fn resolve_by_source_resolves_a_dependency_source() {
    let (_fix, core) = indexed("resolve-source-dependency");
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({ "source": "dependency:0:d.pbo:config.bin", "class": ["CfgVehicles", "Dep_A"] }),
    );

    assert_eq!(resolved["owner_patch"], "DZ_Dep", "{resolved}");
    let weight = property(&resolved, "weight");
    assert_eq!(weight["from_source"]["id"], "vanilla:a.pbo:config.bin");
}

#[test]
fn resolve_refuses_an_unknown_source() {
    let (_fix, core) = indexed("resolve-unknown-source");
    let err = core
        .dispatch(
            Actor::User,
            "config.resolve",
            json!({ "source": "vanilla:nope.pbo:config.bin" }),
        )
        .unwrap_err();
    let CommandError::InvalidArguments(message) = err else {
        panic!("expected InvalidArguments, got {err:?}");
    };
    assert_eq!(
        message,
        "unknown config source `vanilla:nope.pbo:config.bin`"
    );
}

#[test]
fn resolve_needs_exactly_one_of_path_and_source() {
    let (fix, core) = indexed("resolve-one-of");
    for args in [
        json!({}),
        json!({ "text": "class CfgVehicles {};" }),
        json!({ "path": fix.root.join("config.cpp"), "source": "workspace:config.cpp" }),
    ] {
        let err = core
            .dispatch(Actor::User, "config.resolve", args.clone())
            .unwrap_err();
        assert!(
            matches!(err, CommandError::InvalidArguments(_)),
            "should refuse {args} as invalid arguments, got {err:?}"
        );
    }
}

#[test]
fn resolve_by_source_is_not_ready_before_an_index() {
    let _fix = fixture("resolve-source-cold");
    let core = common::core();
    let resolved = dispatch(
        &core,
        "config.resolve",
        json!({ "source": "vanilla:a.pbo:config.bin" }),
    );
    assert_eq!(resolved["ready"], false, "{resolved}");
    assert_eq!(resolved["externals"], json!([]));
}
