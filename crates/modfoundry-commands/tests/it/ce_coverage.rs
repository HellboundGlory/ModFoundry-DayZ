//! `ce.coverage` and the config-class pass of `ce.validate`: the
//! workspace classes with and without a types entry, and the MF-X121/X122/X123
//! findings a types file earns against the config index.

use std::path::{Path, PathBuf};

use crate::common::{self, config_bin, pack, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_commands::registry;
use modfoundry_core::Actor;
use serde_json::{json, Value};

const GAME_CFG: &str = "\
class CfgVehicles {
    class Inventory_Base { scope = 0; };
    class HouseNoDestruct { scope = 0; };
    class Canteen: Inventory_Base { scope = 2; };
};
";

const WORKSPACE_CFG: &str = "\
class CfgPatches { class FieldKit_Data {}; };
class CfgVehicles {
    class Canteen;
    class Inventory_Base;
    class HouseNoDestruct;
    class FieldCanteen: Canteen { scope = 2; };
    class WaterPurifier: Inventory_Base { scope = 2; };
    class Hidden: Inventory_Base { scope = 1; };
    class Shed: HouseNoDestruct { scope = 2; };
};
";

const WORKSPACE_TYPES: &str = "\
<types>
  <type name=\"FieldCanteen\"><nominal>12</nominal></type>
  <type name=\"fieldcanteen_x\"><nominal>1</nominal></type>
  <type name=\"Hidden\"><nominal>1</nominal></type>
  <type name=\"canteen\"><nominal>1</nominal></type>
</types>
";

const REFERENCE_TYPES: &str = "\
<types>
  <type name=\"Canteen\"><nominal>1</nominal></type>
</types>
";

const LIMITS: &str = "\
<lists>
  <categories><category name=\"tools\"/></categories>
  <tags><tag name=\"military\"/></tags>
  <usageflags><usage name=\"Village\"/></usageflags>
  <valueflags><value name=\"Tier1\"/></valueflags>
</lists>
";

const LIMITS_USER: &str = "\
<user_lists>
  <usageflags><user name=\"TownVillage\"><usage name=\"Village\"/></user></usageflags>
  <valueflags><user name=\"TierOne\"><value name=\"Tier1\"/></user></valueflags>
</user_lists>
";

const ECONOMY_CORE: &str = "<economycore><classes></classes><defaults></defaults></economycore>\n";
const INIT_C: &str = "void main() {}\n";

struct Fixture {
    root: PathBuf,
    game: PathBuf,
    mission: PathBuf,
}

fn fixture(label: &str) -> Fixture {
    let root = tmp(&format!("{label}-ws"));
    write(&root, "config.cpp", WORKSPACE_CFG);
    write(&root, "types.xml", WORKSPACE_TYPES);

    let game = tmp(&format!("{label}-game"));
    let cfg = config_bin(GAME_CFG);
    pack(
        &game.join("Addons/a.pbo"),
        &[("prefix", "dz/a")],
        &[("config.bin", cfg.as_slice())],
    );

    let mission = tmp(&format!("{label}-mission")).join("dayzOffline.chernarusplus");
    write(&mission, "db/types.xml", REFERENCE_TYPES);
    write(&mission, "cfglimitsdefinition.xml", LIMITS);
    write(&mission, "cfglimitsdefinitionuser.xml", LIMITS_USER);
    write(&mission, "cfgeconomycore.xml", ECONOMY_CORE);
    write(&mission, "init.c", INIT_C);

    Fixture {
        root,
        game,
        mission,
    }
}

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

fn config_index(core: &modfoundry_core::Core, fix: &Fixture) -> Value {
    dispatch(
        core,
        "config.index",
        json!({ "root": fix.root, "dependencies": [], "game": fix.game }),
    )
}

fn ce_index(core: &modfoundry_core::Core, fix: &Fixture) -> Value {
    dispatch(
        core,
        "ce.index",
        json!({
            "root": fix.root,
            "mission": fix.mission,
            "server": tmp("no-server"),
            "game": tmp("no-game"),
        }),
    )
}

fn codes(findings: &Value) -> Vec<&str> {
    findings
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|finding| finding["code"].as_str())
        .collect()
}

#[test]
fn coverage_is_ready_only_once_both_indexes_are_loaded() {
    let fix = fixture("ready");
    let session = registry::Session::new();

    let before = dispatch(&common::core_with(&session), "ce.coverage", json!({}));
    assert_eq!(before["ready"], false, "{before}");
    assert_eq!(before["classes"], json!([]));
    assert_eq!(before["covered"], 0);
    assert_eq!(before["missing"], 0);
    assert_eq!(before["findings"], json!([]));

    config_index(&common::core_with(&session), &fix);
    let only_config = dispatch(&common::core_with(&session), "ce.coverage", json!({}));
    assert_eq!(only_config["ready"], false, "{only_config}");

    let fresh = registry::Session::new();
    ce_index(&common::core_with(&fresh), &fix);
    let only_economy = dispatch(&common::core_with(&fresh), "ce.coverage", json!({}));
    assert_eq!(only_economy["ready"], false, "{only_economy}");
}

#[test]
fn coverage_lists_spawnable_classes_with_and_without_types() {
    let fix = fixture("coverage");
    let session = registry::Session::new();
    config_index(&common::core_with(&session), &fix);
    ce_index(&common::core_with(&session), &fix);
    let report = dispatch(&common::core_with(&session), "ce.coverage", json!({}));

    assert_eq!(report["ready"], true, "{report}");
    let classes = report["classes"].as_array().unwrap();
    assert_eq!(classes.len(), 2, "{report}");
    assert_eq!(
        classes[0],
        json!({
            "name": "FieldCanteen",
            "scope": 2,
            "kind": "loot",
            "file": fix.root.join("config.cpp"),
            "span": { "start": WORKSPACE_CFG.find("FieldCanteen").unwrap(),
                      "end": WORKSPACE_CFG.find("FieldCanteen").unwrap() + "FieldCanteen".len() },
            "line": 6,
            "has_types": true,
        }),
        "{report}"
    );
    assert_eq!(
        classes[1],
        json!({
            "name": "WaterPurifier",
            "scope": 2,
            "kind": "loot",
            "file": fix.root.join("config.cpp"),
            "span": { "start": WORKSPACE_CFG.find("WaterPurifier:").unwrap(),
                      "end": WORKSPACE_CFG.find("WaterPurifier:").unwrap() + "WaterPurifier".len() },
            "line": 7,
            "has_types": false,
        }),
        "{report}"
    );
    assert_eq!(report["covered"], 1);
    assert_eq!(report["missing"], 1);

    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{report}");
    let finding = &findings[0];
    assert_eq!(finding["code"], "MF-X120");
    assert_eq!(finding["path"], json!(fix.root.join("config.cpp")));
    let span = &finding["span"];
    let text = WORKSPACE_CFG;
    assert_eq!(
        &text[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize],
        "WaterPurifier"
    );
}

#[test]
fn adding_a_type_clears_the_missing_class() {
    let fix = fixture("added");
    let session = registry::Session::new();
    config_index(&common::core_with(&session), &fix);
    ce_index(&common::core_with(&session), &fix);
    let before = dispatch(&common::core_with(&session), "ce.coverage", json!({}));
    assert_eq!(before["missing"], 1);

    let extended = WORKSPACE_TYPES.replace(
        "<type name=\"FieldCanteen\"><nominal>12</nominal></type>",
        "<type name=\"FieldCanteen\"><nominal>12</nominal></type>\n  \
         <type name=\"WaterPurifier\"><nominal>1</nominal></type>",
    );
    write(&fix.root, "types.xml", &extended);
    ce_index(&common::core_with(&session), &fix);

    let after = dispatch(&common::core_with(&session), "ce.coverage", json!({}));
    assert_eq!(after["missing"], 0, "{after}");
    assert_eq!(after["covered"], 2, "{after}");
    assert_eq!(after["findings"], json!([]), "{after}");
}

#[test]
fn validate_flags_types_against_the_config_classes() {
    let fix = fixture("validate");
    let core = common::core();
    config_index(&core, &fix);

    let report = dispatch(
        &core,
        "ce.validate",
        json!({ "path": fix.root.join("types.xml") }),
    );
    let findings = report.as_array().unwrap();

    assert_eq!(
        codes(&report).iter().filter(|c| **c == "MF-X121").count(),
        1
    );
    let fieldcanteen_x = findings
        .iter()
        .find(|f| f["code"] == "MF-X121")
        .expect("MF-X121 for fieldcanteen_x");
    assert!(
        fieldcanteen_x["message"]
            .as_str()
            .unwrap()
            .contains("fieldcanteen_x"),
        "{fieldcanteen_x}"
    );
    assert!(
        fieldcanteen_x["message"]
            .as_str()
            .unwrap()
            .contains("Did you mean `FieldCanteen`?"),
        "{fieldcanteen_x}"
    );

    let hidden = findings
        .iter()
        .find(|f| f["code"] == "MF-X122")
        .expect("MF-X122 for Hidden");
    assert!(
        hidden["message"].as_str().unwrap().contains("scope 1"),
        "{hidden}"
    );

    let canteen = findings
        .iter()
        .find(|f| f["code"] == "MF-X123")
        .expect("MF-X123 for canteen");
    assert!(
        canteen["message"]
            .as_str()
            .unwrap()
            .contains("declared as `Canteen`"),
        "{canteen}"
    );

    assert_eq!(
        codes(&report).iter().filter(|c| **c == "MF-X122").count(),
        1
    );
    assert_eq!(
        codes(&report).iter().filter(|c| **c == "MF-X123").count(),
        1
    );
}

#[test]
fn validate_adds_no_class_findings_without_a_config_index() {
    let fix = fixture("validate-noconfig");
    let core = common::core();
    let report = dispatch(
        &core,
        "ce.validate",
        json!({ "path": fix.root.join("types.xml") }),
    );
    let codes = codes(&report);
    for code in ["MF-X121", "MF-X122", "MF-X123"] {
        assert!(!codes.contains(&code), "{report}");
    }
}

#[test]
fn coverage_leaves_no_audit_row() {
    let broker = Broker::in_memory(common::core()).unwrap();
    for _ in 0..3 {
        broker
            .dispatch(Actor::User, "ce.coverage", json!({}))
            .unwrap();
    }
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

/// Reads one entry out of a PBO without unpacking the whole container.
fn pbo_entry(pbo: &Path, want: &str) -> Option<String> {
    let archive = modfoundry_pbo::PboArchive::open(pbo).ok()?;
    let index = archive
        .entries()
        .iter()
        .position(|e| e.name_str().replace('\\', "/").to_ascii_lowercase() == want)?;
    let bytes = archive.read_entry(index).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The vanilla gate, opt-in via `MF_DAYZ_INSTALL`.
#[test]
fn corpus_types_against_the_vanilla_configs() {
    let Ok(install) = std::env::var("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let install = PathBuf::from(install);
    let session = registry::Session::new();
    let workspace = tmp("corpus-ws");
    std::fs::create_dir_all(&workspace).unwrap();
    dispatch(
        &common::core_with(&session),
        "config.index",
        json!({ "root": workspace, "dependencies": [], "game": install }),
    );

    // Measured on DayZ 1.30.164200: vanilla types.xml lists classes no config declares (MF-X121)
    // and scope-1 classes such as Christmas trees and Nasdara's houses (MF-X122).
    let worlds = [
        (
            "chernarus",
            install.join("Addons/worlds_chernarusplus_ce.pbo"),
            7,
            1,
        ),
        ("livonia", install.join("Addons/worlds_enoch_ce.pbo"), 8, 2),
        (
            "sakhal",
            install.join("sakhal/Addons/worlds_sakhal_ce.pbo"),
            7,
            4,
        ),
        (
            "nasdara",
            install.join("nasdara/addons/worlds_nasdara_ce.pbo"),
            12,
            37,
        ),
    ];
    for (world, pbo, max_x121, max_x122) in worlds {
        let Some(text) = pbo_entry(&pbo, "db/types.xml") else {
            eprintln!("skipped: no db/types.xml in {}", pbo.display());
            continue;
        };
        let report = dispatch(
            &common::core_with(&session),
            "ce.validate",
            json!({ "path": workspace.join("db/types.xml"), "text": text }),
        );
        let codes = codes(&report);
        let count = |code: &str| codes.iter().filter(|c| **c == code).count();
        let (x121, x122, x123) = (count("MF-X121"), count("MF-X122"), count("MF-X123"));
        println!("{world}: MF-X121={x121} MF-X122={x122} MF-X123={x123}");
        assert!(x122 <= max_x122, "{world}: MF-X122 {x122} > {max_x122}");
        assert!(x121 <= max_x121, "{world}: MF-X121 {x121} > {max_x121}");
    }
}
