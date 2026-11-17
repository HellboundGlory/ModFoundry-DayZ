//! `mf search` and `mf search index`: their answers, their refusals and their JSON form.

use std::path::Path;

use crate::binary::{mf, mf_in, stderr, stdout, Temp};
use crate::layout_cli::pack;

/// A game folder holding one script PBO with one script in it.
fn fake_game(dir: &Path) -> std::path::PathBuf {
    let game = dir.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[(
            "3_Game/vehicle.c",
            b"class Vehicle\n{\n    void Start()\n    {\n    }\n}\n",
        )],
    );
    game
}

#[test]
fn a_question_with_no_index_asks_for_one() {
    let dir = Temp::new("search-cli-no-index");
    let game = fake_game(dir.path());

    let out = mf(&[
        "search",
        "how",
        "--game",
        game.to_str().unwrap(),
        "--model",
        dir.join("model").to_str().unwrap(),
    ]);

    assert_ne!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("run `mf search index`"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn an_unknown_kind_is_refused_and_named() {
    let dir = Temp::new("search-cli-kind");
    let game = fake_game(dir.path());

    let out = mf(&[
        "search",
        "how",
        "--kind",
        "bogus",
        "--game",
        game.to_str().unwrap(),
    ]);

    assert_ne!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(stderr(&out).contains("bogus"), "got: {}", stderr(&out));
}

#[test]
fn a_question_or_a_sub_verb_is_required() {
    let out = mf(&["search"]);

    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("a question is required, or `mf search index`"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn indexing_a_missing_game_fails() {
    let dir = Temp::new("search-cli-missing-game");

    let out = mf(&[
        "search",
        "index",
        "--game",
        dir.join("nope").to_str().unwrap(),
    ]);

    assert_ne!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("not a folder"),
        "got: {}",
        stderr(&out)
    );
}

/// An indexed file's control characters never reach the terminal.
#[test]
#[ignore]
fn a_hit_with_control_characters_prints_without_them() {
    let Some(model) = std::env::var_os("MF_SEARCH_MODEL") else {
        eprintln!("skipped: MF_SEARCH_MODEL is not set");
        return;
    };
    let dir = Temp::new("search-cli-control");
    let game = dir.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[(
            "3_Game/noisy.c",
            b"class Noisy\n{\n    void Run()\n    {\n        Print(\"\x1b[31mred\x07\");\n    }\n}\n",
        )],
    );
    let model = std::path::PathBuf::from(model);
    let game_arg = game.to_str().unwrap();
    let model_arg = model.to_str().unwrap();
    let base = Temp::new("search-cli-control-db");
    let db = base.join("modfoundry.sqlite");

    let indexed = mf_in(
        &db,
        &["search", "index", "--game", game_arg, "--model", model_arg],
    );
    assert_eq!(indexed.status.code(), Some(0), "{}", stderr(&indexed));

    let out = mf_in(
        &db,
        &[
            "search",
            "noisy run",
            "--game",
            game_arg,
            "--model",
            model_arg,
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Noisy"), "got: {text}");
    assert!(
        !text.contains('\u{1b}') && !text.contains('\u{7}'),
        "control characters reached the terminal: {text:?}"
    );
    assert!(text.contains("red"), "the text between them stays: {text}");
}

/// Builds an index over a fake game and finds its one script again.
#[test]
#[ignore]
fn an_indexed_script_is_found_by_its_question() {
    let Some(model) = std::env::var_os("MF_SEARCH_MODEL") else {
        eprintln!("skipped: MF_SEARCH_MODEL is not set");
        return;
    };
    let dir = Temp::new("search-cli-index");
    let game = fake_game(dir.path());
    let model = std::path::PathBuf::from(model);
    let game_arg = game.to_str().unwrap();
    let model_arg = model.to_str().unwrap();
    // One database for both runs, so the second finds the first's index in the same cache.
    let base = Temp::new("search-cli-index-db");
    let db = base.join("modfoundry.sqlite");

    let indexed = mf_in(
        &db,
        &["search", "index", "--game", game_arg, "--model", model_arg],
    );
    assert_eq!(indexed.status.code(), Some(0), "{}", stderr(&indexed));
    let summary = stdout(&indexed);
    assert!(
        summary.contains("indexed 1 files, 2 pieces from 1 sources in "),
        "got: {summary}"
    );
    assert!(summary.contains("index: "), "got: {summary}");
    assert!(summary.contains("/index.sqlite (build "), "got: {summary}");

    let out = mf_in(
        &db,
        &[
            "search",
            "how a vehicle starts",
            "--game",
            game_arg,
            "--model",
            model_arg,
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("1. scripts/3_Game/vehicle.c:"),
        "got: {text}"
    );

    let as_json = mf_in(
        &db,
        &[
            "search",
            "how a vehicle starts",
            "--game",
            game_arg,
            "--model",
            model_arg,
            "--json",
        ],
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&as_json)).expect("JSON output");
    assert!(
        value["hits"]
            .as_array()
            .is_some_and(|hits| !hits.is_empty()),
        "got: {value}"
    );
}
