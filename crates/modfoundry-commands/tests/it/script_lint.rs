use std::path::PathBuf;

use modfoundry_commands::project_settings::{project_id, PREFS_PROJECT_KEY};
use modfoundry_commands::registry::Session;
use modfoundry_core::{Actor, CommandError, Outcome};
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::{json, Value};

use crate::common;

const MISSING_RETURN: &str = "class Helper\n{\n}\n\nclass B\n{\n\tint F()\n\t{\n\t\tint a = 1;\n\t\tif (a > 0)\n\t\t\treturn 1;\n\t}\n\n\tvoid Helper()\n\t{\n\t}\n\n\tvoid G()\n\t{\n\t\tHelper();\n\t}\n}\n";

fn lint(args: Value) -> Result<Value, CommandError> {
    let Outcome::Data(value) = common::core().dispatch(Actor::CliSession, "script.lint", args)?
    else {
        panic!("script.lint returns data");
    };
    Ok(value)
}

/// A mod with a missing return, a game folder whose packed scripts hold one base class, and a cache folder.
fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let game = common::tmp(&format!("{label}-game"));
    common::pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("1_Core\\base.c", b"class PrBase\n{\n}\n")],
    );
    let root = common::tmp(&format!("{label}-mod"));
    common::write(&root, "Scripts/4_World/b.c", MISSING_RETURN);
    (root, game, common::tmp(&format!("{label}-cache")))
}

fn codes(value: &Value) -> Vec<&str> {
    value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect()
}

#[test]
fn a_covered_guess_gives_way_to_the_game_finding_on_its_file() {
    let (root, game, cache) = fixture("lint-covered");

    let value = lint(json!({
        "path": root,
        "game": game,
        "dependencies": [],
        "cache": cache,
    }))
    .unwrap();

    assert!(codes(&value).contains(&"MF-E4001"), "{value}");
    assert!(!codes(&value).contains(&"MF-E1003"), "{value}");
}

#[test]
fn without_known_dependencies_the_game_check_is_skipped_and_the_guesses_stay() {
    let (root, game, cache) = fixture("lint-unknown-deps");

    let value = lint(json!({
        "path": root,
        "game": game,
        "cache": cache,
    }))
    .unwrap();

    assert_eq!(
        value["game"]["skipped"],
        "the mod's dependencies are not known; save them with mf deps add (MCP: project.edit_dependencies), or pass --dependency or --no-dependencies",
        "{value}"
    );
    assert!(codes(&value).contains(&"MF-E1003"), "{value}");
    assert!(!codes(&value).contains(&"MF-E4001"), "{value}");
}

#[test]
fn a_finding_that_only_some_builds_print_names_those_builds() {
    let (_, game, cache) = fixture("lint-builds");
    let root = common::tmp("lint-builds-mod");
    common::write(
        &root,
        "Scripts/3_Game/bad.c",
        "#ifdef DIAG\nvoid F()\n{\n\tint a = b;\n}\n#endif\n",
    );

    let value = lint(json!({
        "path": root,
        "game": game,
        "dependencies": [],
        "cache": cache,
    }))
    .unwrap();

    let messages: Vec<&str> = value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["message"].as_str().unwrap())
        .collect();
    assert!(
        messages.contains(&"Can't find variable 'b' [diag-server, diag-client]"),
        "{value}"
    );
}

#[test]
fn a_failed_index_skips_the_game_check_and_names_the_index_error() {
    let (root, game, _) = fixture("lint-no-index");
    let gone = common::tmp("lint-no-index-gone").join("@Gone");
    let db = common::tmp("lint-no-index-db").join("agents.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&root)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "gone",
                "name": "Gone",
                "role": "Both",
                "use_as": "Dependency",
                "enabled": true,
                "source": {"Folder": {"path": gone}},
            }]}}),
        )
        .unwrap();

    let session = Session::new().with_db(db);
    let Outcome::Data(value) = common::core_with(&session)
        .dispatch(
            Actor::CliSession,
            "script.lint",
            json!({ "path": root, "game": game }),
        )
        .unwrap()
    else {
        panic!("script.lint returns data");
    };

    assert!(
        value["game"]["skipped"]
            .as_str()
            .unwrap()
            .starts_with("no script index:"),
        "{value}"
    );
    assert!(value["index_error"].is_string(), "{value}");
}
