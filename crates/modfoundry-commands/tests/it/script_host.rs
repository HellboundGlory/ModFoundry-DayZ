//! `ScriptHost` and the `script.*` query commands: index, status, hover,
//! definition and source, over workspace + vanilla sources with the caller's
//! unsaved buffer overlaid.

use std::path::{Path, PathBuf};

use crate::common::{self, pack, tmp, write};
use modfoundry_agent::Broker;
use modfoundry_commands::FindingReport;
use modfoundry_core::{Actor, CommandError};
use serde_json::{json, Value};

const VANILLA: &str = "class PlayerBase\n{\n}\n";

/// A workspace containing one `Scripts/4_World/a.c` and a fake game dir whose
/// `dta/scripts.pbo` holds one vanilla file.
fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = tmp(label);
    let a = write(
        &root,
        "Scripts/4_World/a.c",
        "modded class PlayerBase { }\n",
    );
    let game = tmp(&format!("{label}-game"));
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("4_World/playerbase.c", VANILLA.as_bytes())],
    );
    (root, game, a)
}

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Value {
    core.dispatch(Actor::User, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
        .to_json()
}

fn index_args(root: &Path, game: &Path) -> Value {
    json!({ "root": root, "game": game })
}

#[test]
fn index_then_status_reports_ready_with_every_source() {
    let (root, game, _) = fixture("index");
    let core = common::core();
    let report = dispatch(&core, "script.index", index_args(&root, &game));
    assert_eq!(report["files"], 2, "{report}");
    assert!(report["suggested"].is_array());

    let status = dispatch(&core, "script.index_status", json!({}));
    assert_eq!(status["state"], "ready");
    assert_eq!(status["fraction"], 1.0);
    assert_eq!(status["files"], 2);
    assert_eq!(status["root"], json!(root));
}

#[test]
fn definition_prefers_the_vanilla_target() {
    let (root, game, a) = fixture("definition");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let text = "modded class PlayerBase { }\nclass Zed { PlayerBase p; }";
    let offset = text.rfind("PlayerBase").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": a, "text": text, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert!(!targets.is_empty());
    let first = &targets[0];
    assert_eq!(first["origin"], "vanilla");
    assert_eq!(first["read_only"], true);
    assert_eq!(first["display_path"], "scripts/4_World/playerbase.c");
    assert_eq!(first["line"], 1);
    assert_eq!(first["column"], 7);
    assert_eq!(first["layer"], "4_World");
    assert_eq!(first["dependency"], Value::Null);
    assert_eq!(out["symbol"]["kind"], "class", "{out}");
    assert_eq!(out["symbol"]["name"], "PlayerBase", "{out}");
}

#[test]
fn source_returns_the_cached_vanilla_text() {
    let (root, game, a) = fixture("source");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let text = "modded class PlayerBase { }\nclass Zed { PlayerBase p; }";
    let offset = text.rfind("PlayerBase").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": a, "text": text, "offset": offset }),
    );
    let id = out["targets"][0]["id"].clone();
    let src = dispatch(&core, "script.source", json!({ "id": id }));
    assert_eq!(src["text"], VANILLA);
    assert_eq!(src["read_only"], true);
    assert_eq!(src["display_path"], "scripts/4_World/playerbase.c");
}

#[test]
fn hover_on_a_method_call_reports_the_method_and_signature() {
    let core = common::core();
    let path = tmp("hover").join("b.c");
    let text = "class Foo { void Bar() { } }\nclass Zed { void Run() { Foo f; f.Bar(); } }";
    let offset = text.rfind("Bar").unwrap();
    let out = dispatch(
        &core,
        "script.hover",
        json!({ "path": path, "text": text, "offset": offset }),
    );
    assert_eq!(out["symbol"]["kind"], "method");
    assert_eq!(out["symbol"]["signature"], "void Bar()");
    assert_eq!(out["symbol"]["container"], "Foo");
    assert_eq!(out["symbol"]["origin"], "workspace");
}

#[test]
fn an_overlaid_class_absent_from_disk_resolves_in_the_same_query() {
    let (root, game, a) = fixture("overlay");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let text = "modded class PlayerBase { }\nclass Zed { NeoSymbol n; }\nclass NeoSymbol { }";
    let offset = text.find("NeoSymbol n").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": a, "text": text, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{out}");
    assert_eq!(targets[0]["origin"], "workspace");
    assert_eq!(targets[0]["read_only"], false);
}

/// A buffer's declarations belong to the query that sent them: a later query on
/// another file must not see a class that exists only in an earlier overlay.
#[test]
fn an_overlay_does_not_outlive_its_query() {
    let (root, game, a) = fixture("overlay-life");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let overlaid = "modded class PlayerBase { }\nclass Ghost { }\n";
    dispatch(
        &core,
        "script.complete",
        json!({ "path": &a, "text": overlaid, "offset": overlaid.find("Gh").unwrap() + 2 }),
    );

    let other = root.join("Scripts/4_World/b.c");
    let text = "class Zed { Ghost g; }";
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": other, "text": text, "offset": text.find("Ghost").unwrap() }),
    );
    assert_eq!(out["targets"], json!([]), "{out}");
}

#[test]
fn a_query_by_vanilla_id_keeps_the_vanilla_origin() {
    let (root, game, _) = fixture("pboid");
    let pbo = game.join("dta/scripts.pbo");
    let id = format!("pbo:{}!4_World/playerbase.c", pbo.display());
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let offset = VANILLA.find("PlayerBase").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": id, "text": VANILLA, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert!(!targets.is_empty());
    assert_eq!(targets[0]["origin"], "vanilla");
    assert_eq!(targets[0]["read_only"], true);
    assert_eq!(targets[0]["id"], id);
}

#[test]
fn definition_with_nothing_resolved_has_a_null_symbol_and_no_targets() {
    let core = common::core();
    let path = tmp("defnothing").join("f.c");
    let text = "class Zed { Unknown u; }";
    let offset = text.find("Unknown").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": path, "text": text, "offset": offset }),
    );
    assert!(out["symbol"].is_null(), "{out}");
    assert_eq!(out["targets"], json!([]), "{out}");
}

#[test]
fn definition_without_a_session_resolves_a_same_file_class() {
    let core = common::core();
    let path = tmp("nosession").join("c.c");
    let text = "class Local { }\nclass Zed { Local l; }";
    let offset = text.find("Local l").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": path, "text": text, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{out}");
    assert_eq!(targets[0]["id"], json!(path));
}

#[test]
fn offsets_off_the_end_or_inside_a_char_are_invalid_arguments() {
    let core = common::core();
    let path = tmp("offsets").join("d.c");
    let text = "class Ünïcode { }";
    let past = core.dispatch(
        Actor::User,
        "script.definition",
        json!({ "path": path, "text": text, "offset": text.len() + 1 }),
    );
    assert!(
        matches!(past, Err(CommandError::InvalidArguments(_))),
        "{past:?}"
    );

    let inner = core.dispatch(
        Actor::User,
        "script.definition",
        json!({ "path": path, "text": text, "offset": 7 }),
    );
    assert!(
        matches!(inner, Err(CommandError::InvalidArguments(_))),
        "{inner:?}"
    );
}

#[test]
fn diagnostics_take_semantic_findings_only_once_the_index_is_ready() {
    let (root, game, _) = fixture("semantics");
    let path = write(&root, "Scripts/3_Game/b.c", "");
    let text = "class B { PlayerBase p; }\n@\n";
    let core = common::core();

    let before = dispatch(
        &core,
        "script.diagnostics",
        json!({ "path": path, "text": text }),
    );
    let expected: Value =
        modfoundry_enforce::lint_text(&path, &modfoundry_enforce::parse_tree(text), text)
            .into_iter()
            .map(|diagnostic| {
                serde_json::to_value(FindingReport {
                    diagnostic,
                    path: path.clone(),
                })
                .unwrap()
            })
            .collect();
    assert_eq!(before, expected);
    assert!(before
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["code"] == "MF-E1001"));

    dispatch(&core, "script.index", index_args(&root, &game));
    let after = dispatch(
        &core,
        "script.diagnostics",
        json!({ "path": path, "text": text }),
    );
    let layer: Vec<&Value> = after
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "MF-E3001")
        .collect();
    assert_eq!(layer.len(), 1, "{after}");
    assert_eq!(
        layer[0]["message"],
        "class `PlayerBase` is defined in 4_World; 3_Game code cannot see it"
    );
    assert_eq!(layer[0]["severity"], "error");
    assert_eq!(layer[0]["path"], json!(path));

    let dir = dispatch(&core, "script.diagnostics", json!({ "path": root }));
    assert!(
        !dir.as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "MF-E3001"),
        "{dir}"
    );
}

#[test]
fn a_folder_lint_reports_a_variable_named_like_a_visible_type() {
    let root = tmp("folder-semantic");
    write(&root, "Scripts/3_Game/foo.c", "class Foo\n{\n}\n");
    let bar = write(
        &root,
        "Scripts/5_Mission/bar.c",
        "class Zed\n{\n    void Run(int Foo)\n    {\n    }\n}\n",
    );
    let game = tmp("folder-semantic-game");
    let core = common::core();

    let before = dispatch(&core, "script.diagnostics", json!({ "path": root }));
    assert!(
        !before
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "MF-E3003"),
        "{before}"
    );

    dispatch(&core, "script.index", index_args(&root, &game));
    let after = dispatch(&core, "script.diagnostics", json!({ "path": root }));
    let found: Vec<&Value> = after
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "MF-E3003")
        .collect();
    assert_eq!(found.len(), 1, "{after}");
    assert_eq!(found[0]["path"], json!(bar));
}

#[test]
fn diagnostics_skip_non_enforce_files_on_disk_but_lint_given_text() {
    let (root, game, _) = fixture("non-enforce");
    let source = "class B { PlayerBase p; }\n";
    let notes = write(&root, "Scripts/3_Game/notes.txt", source);
    let script = write(&root, "Scripts/3_Game/b.c", source);
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let cases = [
        (json!({ "path": notes }), Vec::<&str>::new()),
        (json!({ "path": notes, "text": source }), vec!["MF-E3001"]),
        (json!({ "path": script }), vec!["MF-E3001"]),
    ];
    for (args, expected) in cases {
        let out = dispatch(&core, "script.diagnostics", args.clone());
        let codes: Vec<&str> = out
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, expected, "{args}: {out}");
    }
}

#[test]
fn quiet_flags_match_the_contract() {
    let core = common::core();
    let quiet = |id: &str| core.command(id).unwrap().quiet();
    assert!(quiet("script.hover"));
    assert!(quiet("script.definition"));
    assert!(quiet("script.source"));
    assert!(quiet("script.index_status"));
    assert!(quiet("script.complete"));
    assert!(quiet("script.find_class"));
    assert!(quiet("ce.get_tree"));
    assert!(quiet("ce.coverage"));
    assert!(quiet("ce.reference"));
    assert!(quiet("ce.source"));
    assert!(quiet("ce.validate"));
    assert!(!quiet("ce.index"));
    assert!(quiet("config.get_tree"));
    assert!(quiet("config.patches"));
    assert!(quiet("config.resolve"));
    assert!(quiet("config.source"));
    assert!(!quiet("config.index"));
    assert!(!quiet("script.index"));
}

#[test]
fn complete_on_unsaved_text_matches_the_pure_function_and_is_quiet() {
    let core = common::core();
    let broker = Broker::in_memory(common::core()).unwrap();
    let path = tmp("complete").join("e.c");
    let text = "class Foo { void Bar() { } }\nclass Zed { void Run() { Foo f; f.Ba } }";
    let offset = text.rfind("f.Ba").unwrap() + "f.Ba".len();
    let out = dispatch(
        &core,
        "script.complete",
        json!({ "path": path, "text": text, "offset": offset }),
    );
    let items = out["items"].as_array().unwrap();
    assert!(!items.is_empty(), "{out}");
    assert_eq!(items[0]["label"], "Bar");
    assert_eq!(items[0]["kind"], "method");
    assert_eq!(out["inferred"], true);
    assert_eq!(out["replace"]["start"], offset - 2);
    assert_eq!(out["replace"]["end"], offset);

    broker
        .dispatch(
            Actor::User,
            "script.complete",
            json!({ "path": path, "text": text, "offset": offset }),
        )
        .unwrap();
    assert!(broker.audit(None, 10).unwrap().is_empty());
}

#[test]
fn quiet_queries_leave_no_audit_row_while_index_does() {
    let (root, game, _) = fixture("audit");
    let broker = Broker::in_memory(common::core()).unwrap();
    let path = root.join("Scripts/4_World/a.c");
    let text = "modded class PlayerBase { }\nclass Zed { PlayerBase p; }";
    let offset = text.rfind("PlayerBase").unwrap();
    for _ in 0..3 {
        broker
            .dispatch(
                Actor::User,
                "script.hover",
                json!({ "path": path, "text": text, "offset": offset }),
            )
            .unwrap();
    }
    assert!(broker.audit(None, 10).unwrap().is_empty());

    broker
        .dispatch(Actor::User, "script.index", index_args(&root, &game))
        .unwrap();
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);
}

#[test]
fn a_workspace_target_shows_a_path_relative_to_the_workspace_root() {
    let (root, game, a) = fixture("wsdisplay");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let text = "modded class PlayerBase { }\nclass Zed { Zed z; }";
    let offset = text.find("Zed z").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": a, "text": text, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{out}");
    assert_eq!(targets[0]["origin"], "workspace");
    assert_eq!(targets[0]["read_only"], false);
    assert_eq!(targets[0]["display_path"], "Scripts/4_World/a.c", "{out}");
}

#[test]
fn a_relative_root_gives_workspace_relative_paths() {
    let (root, game, a) = fixture("relative-root");
    let mut relative = PathBuf::new();
    for _ in std::env::current_dir().unwrap().components() {
        relative.push("..");
    }
    relative.push(root.strip_prefix("/").unwrap());

    let core = common::core();
    dispatch(&core, "script.index", index_args(&relative, &game));

    let text = "modded class PlayerBase { }\nclass Zed { Zed z; }";
    let offset = text.find("Zed z").unwrap();
    let out = dispatch(
        &core,
        "script.definition",
        json!({ "path": a, "text": text, "offset": offset }),
    );
    let targets = out["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{out}");
    assert_eq!(targets[0]["display_path"], "Scripts/4_World/a.c", "{out}");
}

#[test]
fn complete_reports_truncation_when_the_candidate_list_hits_the_cap() {
    let core = common::core();
    let path = tmp("truncate").join("t.c");

    let mut text = String::from("class Big {\n");
    for i in 0..205 {
        text.push_str(&format!("\tvoid M{i}();\n"));
    }
    text.push_str("}\nclass Zed { void Run(Big b) { b. } }");
    let offset = text.find("b.").unwrap() + 2;
    let out = dispatch(
        &core,
        "script.complete",
        json!({ "path": path, "text": text, "offset": offset }),
    );
    assert_eq!(out["truncated"], true, "{out}");
    assert_eq!(out["items"].as_array().unwrap().len(), 200, "{out}");

    let small = "class Small { void Only(); }\nclass Zed { void Run(Small s) { s. } }";
    let offset = small.find("s.").unwrap() + 2;
    let out = dispatch(
        &core,
        "script.complete",
        json!({ "path": path, "text": small, "offset": offset }),
    );
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["items"].as_array().unwrap().len(), 1, "{out}");
}

#[test]
fn find_class_reports_a_workspace_class_before_and_after_its_modded_link() {
    let root = tmp("findclass");
    write(
        &root,
        "Scripts/4_World/fieldcanteen.c",
        "class FieldCanteen\n{\n}\n",
    );
    write(
        &root,
        "Scripts/5_Mission/fieldcanteen_mod.c",
        "modded class FieldCanteen { }\n",
    );
    let game = tmp("findclass-game");
    let core = common::core();
    dispatch(&core, "script.index", index_args(&root, &game));

    let out = dispatch(
        &core,
        "script.find_class",
        json!({ "name": "FieldCanteen" }),
    );
    assert_eq!(out["ready"], true, "{out}");
    let locations = out["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2, "{out}");
    assert_eq!(
        locations[0]["display_path"], "Scripts/4_World/fieldcanteen.c",
        "{out}"
    );
    assert_eq!(locations[0]["line"], 1, "{out}");
    assert_eq!(locations[0]["column"], 7, "{out}");
    assert_eq!(locations[0]["modded"], false, "{out}");
    assert_eq!(locations[0]["read_only"], false, "{out}");
    assert_eq!(
        locations[1]["display_path"], "Scripts/5_Mission/fieldcanteen_mod.c",
        "{out}"
    );
    assert_eq!(locations[1]["modded"], true, "{out}");
}

#[test]
fn find_class_is_exact_case_sensitive_and_empty_before_indexing() {
    let (root, game, _) = fixture("findclass-exact");
    let core = common::core();

    let before = dispatch(&core, "script.find_class", json!({ "name": "PlayerBase" }));
    assert_eq!(before["ready"], false, "{before}");
    assert_eq!(before["locations"], json!([]), "{before}");

    dispatch(&core, "script.index", index_args(&root, &game));
    let lower = dispatch(&core, "script.find_class", json!({ "name": "playerbase" }));
    assert_eq!(lower["ready"], true, "{lower}");
    assert_eq!(lower["locations"], json!([]), "{lower}");

    let unknown = dispatch(&core, "script.find_class", json!({ "name": "NoSuchClass" }));
    assert_eq!(unknown["locations"], json!([]), "{unknown}");

    let exact = dispatch(&core, "script.find_class", json!({ "name": "PlayerBase" }));
    let locations = exact["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2, "{exact}");
    assert_eq!(locations[0]["origin"], "vanilla", "{exact}");
    assert_eq!(locations[1]["origin"], "workspace", "{exact}");
}

#[test]
fn find_class_leaves_no_audit_row() {
    let (root, game, _) = fixture("findclass-audit");
    let broker = Broker::in_memory(common::core()).unwrap();
    broker
        .dispatch(Actor::User, "script.index", index_args(&root, &game))
        .unwrap();
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);

    broker
        .dispatch(
            Actor::User,
            "script.find_class",
            json!({ "name": "PlayerBase" }),
        )
        .unwrap();
    assert_eq!(broker.audit(None, 10).unwrap().len(), 1);
}

/// A mod whose `config.cpp` binds `MyMod/Scripts/Common`, with one file under
/// that folder and one under an unbound folder.
fn bound_and_stray_mod(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = tmp(label);
    write(
        &root,
        "config.cpp",
        "class CfgMods\n{\n    class MyMod\n    {\n        class defs\n        {\n            class gameScriptModule\n            {\n                files[] = { \"MyMod/Scripts/Common\" };\n            };\n        };\n    };\n};\n",
    );
    let bound = write(&root, "Scripts/Common/a.c", "class A\n{\n}\n");
    let stray = write(&root, "Scripts/Other/b.c", "class B\n{\n}\n");
    (root, bound, stray)
}

fn codes(out: &Value) -> Vec<&str> {
    out.as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect()
}

#[test]
fn a_folder_cfgmods_binds_has_no_unbound_warning() {
    let (root, bound, stray) = bound_and_stray_mod("cfgmods-bound");
    let core = common::core();

    let folder = dispatch(&core, "script.diagnostics", json!({ "path": root }));
    let warned: Vec<&Value> = folder
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "MF-E2001")
        .collect();
    assert_eq!(warned.len(), 1, "{folder}");
    assert_eq!(warned[0]["path"], json!(stray), "{folder}");

    let file = dispatch(&core, "script.diagnostics", json!({ "path": bound }));
    assert!(codes(&file).is_empty(), "{file}");

    let text = dispatch(
        &core,
        "script.diagnostics",
        json!({ "path": bound, "text": "class A\n{\n}\n" }),
    );
    assert!(codes(&text).is_empty(), "{text}");
}

#[test]
fn a_config_in_a_folder_above_binds_too() {
    let (root, bound, _) = bound_and_stray_mod("cfgmods-above");
    let core = common::core();
    let out = dispatch(
        &core,
        "script.diagnostics",
        json!({ "path": root.join("Scripts") }),
    );
    assert!(
        !out.as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == json!(bound) && f["code"] == "MF-E2001"),
        "{out}"
    );
}

#[test]
fn an_unreadable_config_leaves_the_warning() {
    let root = tmp("cfgmods-unreadable");
    std::fs::create_dir_all(root.join("config.cpp")).unwrap();
    let stray = write(&root, "loose/scratch.c", "class Scratch\n{\n}\n");
    let core = common::core();
    let out = dispatch(&core, "script.diagnostics", json!({ "path": root }));
    let warned: Vec<&Value> = out
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "MF-E2001")
        .collect();
    assert_eq!(warned.len(), 1, "{out}");
    assert_eq!(warned[0]["path"], json!(stray), "{out}");
}

/// `path` written relative to the test's working folder, through `..` steps.
fn relative_to_cwd(path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let mut out = PathBuf::new();
    for _ in cwd.components().skip(1) {
        out.push("..");
    }
    out.join(path.strip_prefix("/").unwrap())
}

#[test]
fn a_relative_path_still_finds_the_config_above_it() {
    let (root, bound, _) = bound_and_stray_mod("cfgmods-relative");
    let core = common::core();

    for path in [
        relative_to_cwd(&root.join("Scripts")),
        relative_to_cwd(&bound),
    ] {
        let out = dispatch(&core, "script.diagnostics", json!({ "path": path }));
        assert!(
            !out.as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == "MF-E2001" && f["path"].as_str().unwrap().ends_with("a.c")),
            "{path:?}: {out}"
        );
    }
}

#[test]
fn a_config_with_a_windows_1252_byte_still_binds() {
    let (root, bound, _) = bound_and_stray_mod("cfgmods-cp1252");
    let mut config = b"// caf\xe9\n".to_vec();
    config.extend(std::fs::read(root.join("config.cpp")).unwrap());
    std::fs::write(root.join("config.cpp"), config).unwrap();

    let out = dispatch(
        &common::core(),
        "script.diagnostics",
        json!({ "path": bound }),
    );

    assert!(codes(&out).is_empty(), "{out}");
}

#[test]
fn a_bound_file_keeps_its_other_findings_in_order() {
    let (root, bound, _) = bound_and_stray_mod("cfgmods-other-findings");
    std::fs::write(&bound, "class A\n{\n@\n}\n").unwrap();

    let out = dispatch(
        &common::core(),
        "script.diagnostics",
        json!({ "path": root }),
    );

    let for_bound: Vec<&str> = out
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["path"] == json!(bound))
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(!for_bound.is_empty(), "{out}");
    assert!(for_bound.iter().all(|code| *code != "MF-E2001"), "{out}");
    assert!(
        codes(&out).contains(&"MF-E2001"),
        "the stray file keeps its warning: {out}"
    );
}

#[test]
fn lint_target_drops_a_bound_file_warning() {
    let (root, bound, stray) = bound_and_stray_mod("cfgmods-lint-target");

    let findings = modfoundry_commands::lint_target(&root).unwrap();

    let warned: Vec<&Path> = findings
        .iter()
        .filter(|f| f.diagnostic.code == "MF-E2001")
        .map(|f| f.path.as_path())
        .collect();
    assert_eq!(warned, [stray.as_path()]);
    assert!(findings.iter().all(|f| f.path != bound));
}
