//! `EconomyHost` and `ce.index`: the mission reference and the
//! workspace's CE files loaded into one economy report.

use std::path::{Path, PathBuf};

use crate::common::{self, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_core::{Actor, CommandError};
use serde_json::{json, Value};

const TYPES: &str = "\
<types>
  <type name=\"Canteen\"><nominal>40</nominal></type>
  <type name=\"WaterBottle\"><nominal>90</nominal></type>
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
const WORKSPACE_TYPES: &str =
    "<types>\n  <type name=\"FieldCanteen\"><nominal>12</nominal></type>\n</types>\n";
const WORKSPACE_CFG: &str = "class CfgVehicles { class FieldKit_Canteen {}; };\n";
const INIT_C: &str = "void main() {}\n";

fn mission(label: &str) -> PathBuf {
    let dir = tmp(&format!("{label}-mission")).join("dayzOffline.chernarusplus");
    write(&dir, "db/types.xml", TYPES);
    write(&dir, "cfglimitsdefinition.xml", LIMITS);
    write(&dir, "cfglimitsdefinitionuser.xml", LIMITS_USER);
    write(&dir, "cfgeconomycore.xml", ECONOMY_CORE);
    write(&dir, "init.c", INIT_C);
    dir
}

fn workspace(label: &str) -> PathBuf {
    let root = tmp(&format!("{label}-ws"));
    write(&root, "types.xml", WORKSPACE_TYPES);
    write(&root, "config.cpp", WORKSPACE_CFG);
    root
}

fn missing(label: &str) -> PathBuf {
    tmp(&format!("{label}-missing"))
}

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

fn index_args(root: &Path, mission: Option<&Path>, label: &str) -> Value {
    json!({
        "root": root,
        "mission": mission,
        "server": missing(label),
        "game": missing(label),
    })
}

#[test]
fn index_reports_the_mission_files_types_and_vocabulary() {
    let mission = mission("report");
    let root = workspace("report");
    let core = common::core();
    let report = dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "report"),
    );

    assert_eq!(
        report["mission"],
        json!({
            "name": "dayzOffline.chernarusplus",
            "path": mission,
            "origin": "folder",
        }),
        "{report}"
    );

    assert_eq!(
        report["files"],
        json!([
            { "id": "mission:cfgeconomycore.xml", "rel": "cfgeconomycore.xml",
              "path": mission.join("cfgeconomycore.xml"), "origin": "reference",
              "kind": "economy_core", "records": 0, "fragment": false, "loaded": true },
            { "id": "mission:cfglimitsdefinition.xml", "rel": "cfglimitsdefinition.xml",
              "path": mission.join("cfglimitsdefinition.xml"), "origin": "reference",
              "kind": "limits", "records": 4, "fragment": false, "loaded": true },
            { "id": "mission:cfglimitsdefinitionuser.xml", "rel": "cfglimitsdefinitionuser.xml",
              "path": mission.join("cfglimitsdefinitionuser.xml"), "origin": "reference",
              "kind": "limits_user", "records": 2, "fragment": false, "loaded": true },
            { "id": "mission:db/types.xml", "rel": "db/types.xml",
              "path": mission.join("db/types.xml"), "origin": "reference",
              "kind": "types", "records": 2, "fragment": false, "loaded": true },
            { "id": "mission:init.c", "rel": "init.c",
              "path": mission.join("init.c"), "origin": "reference",
              "kind": null, "records": 0, "fragment": false, "loaded": false },
            { "id": root.join("types.xml").to_string_lossy(), "rel": "types.xml",
              "path": root.join("types.xml"), "origin": "workspace",
              "kind": "types", "records": 1, "fragment": false, "loaded": true },
        ]),
        "{report}"
    );

    assert_eq!(report["types"], 3, "{report}");
    assert_eq!(report["warnings"], json!([]), "{report}");
    assert!(report["millis"].is_u64(), "{report}");
    assert_eq!(
        report["vocabulary"],
        json!({
            "categories": ["tools"],
            "tags": ["military"],
            "usage": ["Village"],
            "value": ["Tier1"],
            "user_usage": [{ "name": "TownVillage", "names": ["Village"] }],
            "user_value": [{ "name": "TierOne", "names": ["Tier1"] }],
        }),
        "{report}"
    );
}

#[test]
fn a_workspace_limits_file_adds_its_names_after_the_reference() {
    let mission = mission("wslimits");
    let root = workspace("wslimits");
    write(
        &root,
        "cfglimitsdefinition.xml",
        "<lists><categories><category name=\"extra\"/></categories>\
         <tags><tag name=\"workshoptag\"/></tags></lists>\n",
    );
    let core = common::core();
    let report = dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "wslimits"),
    );

    assert_eq!(
        report["vocabulary"]["categories"],
        json!(["tools", "extra"]),
        "{report}"
    );
    assert_eq!(
        report["vocabulary"]["tags"],
        json!(["military", "workshoptag"]),
        "{report}"
    );
    assert_eq!(
        report["vocabulary"]["usage"],
        json!(["Village"]),
        "{report}"
    );
}

#[test]
fn without_a_reference_the_mission_is_null_and_the_workspace_is_still_listed() {
    let root = workspace("none");
    let core = common::core();
    let report = dispatch(&core, "ce.index", index_args(&root, None, "none"));

    assert_eq!(report["mission"], Value::Null, "{report}");
    let warnings = report["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{report}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .starts_with("No DayZ server or game install found"),
        "{report}"
    );
    assert_eq!(report["types"], 1, "{report}");
    assert_eq!(
        report["vocabulary"],
        json!({
            "categories": [],
            "tags": [],
            "usage": [],
            "value": [],
            "user_usage": [],
            "user_value": [],
        }),
        "{report}"
    );
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{report}");
    assert_eq!(files[0]["origin"], "workspace", "{report}");
    assert_eq!(files[0]["kind"], "types", "{report}");
}

#[test]
fn a_second_index_reports_only_the_second_request() {
    let first_mission = mission("supersede-a");
    let root = workspace("supersede-a");
    let core = common::core();
    let first = dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&first_mission), "supersede-a"),
    );
    assert_eq!(first["types"], 3, "{first}");

    let second_mission = tmp("supersede-b-mission").join("dayzOffline.enoch");
    write(
        &second_mission,
        "db/types.xml",
        "<types><type name=\"Pot\"><nominal>1</nominal></type></types>\n",
    );
    let empty = tmp("supersede-b-ws");
    std::fs::create_dir_all(&empty).unwrap();
    let second = dispatch(
        &core,
        "ce.index",
        index_args(&empty, Some(&second_mission), "supersede-b"),
    );
    assert_eq!(second["types"], 1, "{second}");
    assert_eq!(second["mission"]["name"], "dayzOffline.enoch", "{second}");
}

#[test]
fn index_writes_an_audit_row() {
    let mission = mission("audit");
    let root = workspace("audit");
    let broker = Broker::in_memory(common::core()).unwrap();
    broker
        .dispatch(
            Actor::User,
            "ce.index",
            index_args(&root, Some(&mission), "audit"),
        )
        .unwrap();
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);
}

#[test]
fn corpus_timing_gate() {
    let Ok(game) = std::env::var("MF_DAYZ_INSTALL") else {
        return;
    };
    let root = tmp("corpus-ws");
    std::fs::create_dir_all(&root).unwrap();
    let core = common::core();
    let mut args = index_args(&root, None, "corpus");
    args["game"] = json!(PathBuf::from(game));
    let report = dispatch(&core, "ce.index", args);
    assert_eq!(report["mission"]["origin"], "game", "{report}");
    println!(
        "corpus: types={} millis={}",
        report["types"], report["millis"]
    );
}

const LIMITS_MILITARY: &str = "\
<lists>
  <usageflags><usage name=\"Military\"/></usageflags>
</lists>
";
const USAGE_MILITRY: &str = "\
<types>
  <type name=\"FieldCanteen\"><usage name=\"Militry\"/></type>
</types>
";
const USAGE_BUNKER: &str = "\
<types>
  <type name=\"FieldCanteen\"><usage name=\"Bunker\"/></type>
</types>
";
const LIMITS_BUNKER: &str = "\
<lists>
  <usageflags><usage name=\"Bunker\"/></usageflags>
</lists>
";

fn codes(findings: &Value) -> Vec<String> {
    findings
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn reference_before_the_index_is_not_ready_with_no_sections() {
    let core = common::core();
    let out = dispatch(&core, "ce.reference", json!({ "kind": "types" }));
    assert_eq!(out["ready"], false, "{out}");
    assert_eq!(out["mission"], Value::Null, "{out}");
    assert_eq!(out["kind"], "types", "{out}");
    assert_eq!(out["sections"], json!([]), "{out}");
}

#[test]
fn reference_lists_the_missions_records_of_a_kind() {
    let mission = mission("reference");
    let root = workspace("reference");
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "reference"),
    );

    let out = dispatch(&core, "ce.reference", json!({ "kind": "types" }));
    assert_eq!(out["ready"], true, "{out}");
    assert_eq!(out["mission"]["name"], "dayzOffline.chernarusplus", "{out}");
    assert_eq!(out["kind"], "types", "{out}");

    let sections = out["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 1, "{out}");
    assert_eq!(sections[0]["id"], "types", "{out}");
    let records = sections[0]["records"].as_array().unwrap();
    let keys: Vec<&str> = records
        .iter()
        .map(|record| record["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["Canteen", "WaterBottle"], "{out}");
    for record in records {
        assert_eq!(record["file"], "mission:db/types.xml", "{out}");
        assert_eq!(record["element"], "type", "{out}");
    }
    let nominal = records[0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == "nominal")
        .unwrap();
    assert_eq!(nominal["raw"], "40", "{out}");
}

#[test]
fn a_later_include_replaces_an_earlier_record_of_the_same_key() {
    let mission = mission("include");
    write(
        &mission,
        "cfgeconomycore.xml",
        "<economycore><classes></classes><ce folder=\"custom\">\
         <file name=\"more.xml\" type=\"types\"/></ce></economycore>\n",
    );
    write(
        &mission,
        "custom/more.xml",
        "<types>\
         <type name=\"Canteen\"><nominal>7</nominal></type>\
         <type name=\"Pot\"><nominal>1</nominal></type>\
         </types>\n",
    );
    let root = workspace("include");
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "include"),
    );

    let out = dispatch(&core, "ce.reference", json!({ "kind": "types" }));
    let records = out["sections"][0]["records"].as_array().unwrap();
    let keys: Vec<&str> = records
        .iter()
        .map(|record| record["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["Canteen", "WaterBottle", "Pot"], "{out}");
    let canteens: Vec<&Value> = records
        .iter()
        .filter(|record| record["key"] == "Canteen")
        .collect();
    assert_eq!(canteens.len(), 1, "{out}");
    assert_eq!(canteens[0]["file"], "mission:custom/more.xml", "{out}");
}

#[test]
fn reference_of_a_schemaless_kind_is_ready_and_empty_and_an_unknown_kind_is_invalid() {
    let mission = mission("weather");
    let root = workspace("weather");
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "weather"),
    );

    let out = dispatch(&core, "ce.reference", json!({ "kind": "weather" }));
    assert_eq!(out["ready"], true, "{out}");
    assert_eq!(out["sections"], json!([]), "{out}");

    let err = core
        .dispatch(
            Actor::CliSession,
            "ce.reference",
            json!({ "kind": "bogus" }),
        )
        .unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidArguments(_)),
        "expected InvalidArguments, got {err:?}"
    );
}

#[test]
fn source_serves_a_loaded_file_and_reads_a_listed_one() {
    let mission = mission("source");
    let root = workspace("source");
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "source"),
    );

    let types = dispatch(&core, "ce.source", json!({ "id": "mission:db/types.xml" }));
    assert_eq!(types["id"], "mission:db/types.xml", "{types}");
    assert_eq!(
        types["display"], "dayzOffline.chernarusplus/db/types.xml",
        "{types}"
    );
    assert_eq!(types["kind"], "types", "{types}");
    assert_eq!(types["text"], TYPES, "{types}");

    let init = dispatch(&core, "ce.source", json!({ "id": "mission:init.c" }));
    assert_eq!(init["text"], INIT_C, "{init}");
    assert_eq!(init["kind"], Value::Null, "{init}");

    let err = core
        .dispatch(
            Actor::CliSession,
            "ce.source",
            json!({ "id": "mission:nope.xml" }),
        )
        .unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidArguments(_)),
        "expected InvalidArguments, got {err:?}"
    );
    assert!(format!("{err}").contains("mission:nope.xml"), "{err:?}");
}

#[test]
fn source_before_the_index_names_the_missing_index() {
    let core = common::core();
    let err = core
        .dispatch(
            Actor::CliSession,
            "ce.source",
            json!({ "id": "mission:db/types.xml" }),
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("the economy index is not loaded"),
        "{err:?}"
    );
}

#[test]
fn validate_cross_findings_come_from_the_indexed_economy() {
    let mission = mission("xcheck");
    write(&mission, "cfglimitsdefinition.xml", LIMITS_MILITARY);
    let root = workspace("xcheck");
    let types = write(&root, "types.xml", USAGE_MILITRY);
    let core = common::core();

    let before = dispatch(&core, "ce.validate", json!({ "path": types }));
    assert!(!codes(&before).contains(&"MF-X130".to_string()), "{before}");

    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "xcheck"),
    );

    let after = dispatch(&core, "ce.validate", json!({ "path": types }));
    assert!(codes(&after).contains(&"MF-X130".to_string()), "{after}");
    let message = after
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "MF-X130")
        .unwrap()["message"]
        .as_str()
        .unwrap();
    assert!(message.contains("Military"), "{message}");

    let case_text =
        "<types>\n  <type name=\"FieldCanteen\"><usage name=\"military\"/></type>\n</types>\n";
    let case = dispatch(
        &core,
        "ce.validate",
        json!({ "path": types, "text": case_text }),
    );
    assert!(codes(&case).contains(&"MF-X132".to_string()), "{case}");

    let clean_text =
        "<types>\n  <type name=\"FieldCanteen\"><usage name=\"Military\"/></type>\n</types>\n";
    let clean = dispatch(
        &core,
        "ce.validate",
        json!({ "path": types, "text": clean_text }),
    );
    assert!(!codes(&clean).contains(&"MF-X130".to_string()), "{clean}");
}

#[test]
fn a_limits_overlay_is_per_call_and_a_saved_file_reindexes() {
    let mission = mission("overlay");
    let root = workspace("overlay");
    let types = write(&root, "types.xml", USAGE_BUNKER);
    let limits = write(&root, "cfglimitsdefinition.xml", LIMITS);
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "overlay"),
    );

    let flagged = dispatch(&core, "ce.validate", json!({ "path": types }));
    assert!(
        codes(&flagged).contains(&"MF-X130".to_string()),
        "{flagged}"
    );

    let overlaid = dispatch(
        &core,
        "ce.validate",
        json!({ "path": limits, "text": LIMITS_BUNKER }),
    );
    assert!(
        !codes(&overlaid).contains(&"MF-X130".to_string()),
        "{overlaid}"
    );

    let still = dispatch(&core, "ce.validate", json!({ "path": types }));
    assert!(codes(&still).contains(&"MF-X130".to_string()), "{still}");

    write(&root, "cfglimitsdefinition.xml", LIMITS_BUNKER);
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "overlay"),
    );

    let clean = dispatch(&core, "ce.validate", json!({ "path": types }));
    assert!(!codes(&clean).contains(&"MF-X130".to_string()), "{clean}");
}

#[test]
fn the_reference_source_and_validate_queries_leave_no_audit_row() {
    let mission = mission("quietce");
    let root = workspace("quietce");
    let broker = Broker::in_memory(common::core()).unwrap();
    broker
        .dispatch(
            Actor::User,
            "ce.index",
            index_args(&root, Some(&mission), "quietce"),
        )
        .unwrap();
    for (id, args) in [
        ("ce.reference", json!({ "kind": "types" })),
        ("ce.source", json!({ "id": "mission:db/types.xml" })),
        ("ce.validate", json!({ "path": root.join("types.xml") })),
    ] {
        broker.dispatch(Actor::User, id, args).unwrap();
    }
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);
}

const ECONOMY_CORE_INCLUDES: &str = "\
<economycore>
  <classes></classes>
  <defaults></defaults>
  <ce folder=\"custom\">
    <file name=\"present.xml\" type=\"types\"/>
    <file name=\"missing.xml\" type=\"types\"/>
    <file name=\"mismatch.xml\" type=\"types\"/>
  </ce>
</economycore>
";
const INCLUDED_TYPES: &str =
    "<types>\n  <type name=\"FieldCanteen\"><nominal>3</nominal></type>\n</types>\n";
const INCLUDED_SPAWNABLE: &str =
    "<spawnabletypes>\n  <type name=\"FieldCanteen\"><attachments/></type>\n</spawnabletypes>\n";

#[test]
fn validate_checks_an_economy_cores_includes_on_disk() {
    let mission = mission("includes");
    let root = workspace("includes");
    write(&root, "cfgeconomycore.xml", ECONOMY_CORE_INCLUDES);
    write(&root, "custom/present.xml", INCLUDED_TYPES);
    write(&root, "custom/mismatch.xml", INCLUDED_SPAWNABLE);
    let core = common::core();
    dispatch(
        &core,
        "ce.index",
        index_args(&root, Some(&mission), "includes"),
    );

    let found = dispatch(
        &core,
        "ce.validate",
        json!({ "path": root.join("cfgeconomycore.xml") }),
    );
    let codes = codes(&found);
    assert_eq!(
        codes.iter().filter(|code| *code == "MF-X500").count(),
        1,
        "{found}"
    );
    assert_eq!(
        codes.iter().filter(|code| *code == "MF-X510").count(),
        1,
        "{found}"
    );
    let rows = found.as_array().unwrap();
    let x500 = rows.iter().find(|row| row["code"] == "MF-X500").unwrap();
    assert_eq!(
        x500["message"], "`custom/missing.xml` isn't there.",
        "{found}"
    );
    let x510 = rows.iter().find(|row| row["code"] == "MF-X510").unwrap();
    assert_eq!(
        x510["message"],
        "`mismatch.xml` holds spawnable types, but its type says `types`. Write `spawnabletypes`.",
        "{found}"
    );
}
