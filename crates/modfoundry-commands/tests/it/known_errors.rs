//! `diag.errors` with a baseline and a known-error file, and `diag.learn_error`.

use crate::common::{self, tmp, write};
use modfoundry_commands::known_errors::{KnownErrors, KNOWN_ERRORS_FILE};
use modfoundry_core::{Actor, Outcome, Severity};
use modfoundry_diag::MappedError;
use serde_json::{json, Value};
use std::path::Path;

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn error(message: &str, logged_file: Option<&str>) -> MappedError {
    MappedError::new(
        None,
        logged_file.map(str::to_owned),
        None,
        Severity::Error,
        message,
    )
}

fn catalogue(dir: &Path, entries: &str) -> std::path::PathBuf {
    write(dir, KNOWN_ERRORS_FILE, entries)
}

#[test]
fn a_pattern_a_regex_and_a_fingerprint_entry_each_mark_a_matching_error() {
    let dir = tmp("known-mark");
    let target = error(
        "Type 'LargeTentClutterCutter' must be inherited",
        Some("Mod/scripts/a.c"),
    );
    let path = catalogue(
        &dir,
        &json!([
            { "id": "aaaa1111", "pattern": "Can't find variable", "meaning": "a typo", "hint": "fix it" },
            { "id": "bbbb2222", "pattern": "re:^JSON ERROR", "meaning": "framework noise", "hint": "" },
            { "id": "cccc3333", "fingerprint": target.fingerprint, "meaning": "by identity", "hint": "h" },
        ])
        .to_string(),
    );

    let known = KnownErrors::load(Some(&path), None).unwrap();
    let mut errors = vec![
        target,
        error("JSON ERROR JM/CF/a.c:46", None),
        error("Can't find variable 'x'", None),
        error("something else", None),
    ];
    known.mark(&mut errors);

    assert_eq!(
        errors[0].known.as_ref().map(|k| k.id.as_str()),
        Some("cccc3333")
    );
    assert_eq!(errors[0].known.as_ref().unwrap().meaning, "by identity");
    assert_eq!(
        errors[1].known.as_ref().map(|k| k.id.as_str()),
        Some("bbbb2222")
    );
    assert_eq!(
        errors[2].known.as_ref().map(|k| k.id.as_str()),
        Some("aaaa1111")
    );
    assert!(errors[3].known.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_entry_in_file_order_wins() {
    let dir = tmp("known-first");
    let path = catalogue(
        &dir,
        &json!([
            { "id": "first000", "pattern": "boom", "meaning": "one", "hint": "" },
            { "id": "second00", "pattern": "boom too", "meaning": "two", "hint": "" },
        ])
        .to_string(),
    );
    let known = KnownErrors::load(Some(&path), None).unwrap();
    let mut errors = vec![error("boom too", None)];
    known.mark(&mut errors);
    assert_eq!(
        errors[0].known.as_ref().map(|k| k.id.as_str()),
        Some("first000")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_generic_error_is_never_marked_known() {
    let dir = tmp("known-generic");
    let path = catalogue(
        &dir,
        &json!([
            { "id": "aaaa1111", "pattern": "Virtual Machine Exception", "meaning": "m", "hint": "" },
            { "id": "bbbb2222", "pattern": "Stack trace", "meaning": "m", "hint": "" },
        ])
        .to_string(),
    );
    let known = KnownErrors::load(Some(&path), None).unwrap();
    let mut errors = vec![
        error("Virtual Machine Exception", Some("Mod/scripts/a.c")),
        error("-- Stack trace --", Some("Mod/scripts/a.c")),
    ];
    known.mark(&mut errors);
    assert!(errors[0].known.is_none());
    assert!(errors[1].known.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_and_no_location_both_give_an_empty_catalogue() {
    let dir = tmp("known-empty");
    std::fs::create_dir_all(&dir).unwrap();
    for (explicit, root) in [
        (None::<&Path>, None::<&Path>),
        (Some(dir.join("nope.json").as_path()), None),
        (None, Some(dir.as_path())),
    ] {
        let known = KnownErrors::load(explicit, root).unwrap();
        assert!(known.patterns().is_empty());
        assert!(known.fingerprints().is_empty());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_explicit_path_beats_the_projects_default() {
    let dir = tmp("known-explicit");
    let default = catalogue(
        &dir,
        &json!([{ "id": "default0", "pattern": "d", "meaning": "", "hint": "" }]).to_string(),
    );
    let other = write(
        &dir,
        "elsewhere/list.json",
        &json!([{ "id": "other000", "pattern": "o", "meaning": "", "hint": "" }]).to_string(),
    );

    let known = KnownErrors::load(Some(&other), Some(&dir)).unwrap();
    assert_eq!(known.patterns().len(), 1);
    assert_eq!(known.patterns()[0].source(), "o");
    assert_eq!(KnownErrors::path(None, Some(&dir)), Some(default));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diag_errors_marks_the_errors_a_baseline_log_also_holds() {
    let core = common::core();
    let logs = tmp("baseline-logs");
    write(
        &logs,
        "DayZServer_1.RPT",
        " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46\r\n\
         3:45:52.000 SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'x'\r\n",
    );
    let vanilla = tmp("baseline-vanilla");
    write(
        &vanilla,
        "DayZServer_0.RPT",
        " 3:40:00.001 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46\r\n",
    );

    let out = dispatch(
        &core,
        "diag.errors",
        json!({ "dir": logs, "baseline": vanilla }),
    );
    let Outcome::Data(v) = out else {
        panic!("expected Data, got {out:?}");
    };
    let errs = v.as_array().unwrap();
    assert_eq!(errs.len(), 2, "{v}");
    let cf = errs
        .iter()
        .find(|e| e["message"].as_str().unwrap().starts_with("JSON ERROR"))
        .unwrap();
    assert_eq!(cf["baseline"], json!(true), "{v}");
    let script = errs
        .iter()
        .find(|e| e["message"].as_str().unwrap().contains("Can't find"))
        .unwrap();
    assert_eq!(script["baseline"], json!(false), "{v}");
    for error in errs {
        let id = error["id"].as_str().unwrap();
        assert_eq!(id.len(), 8, "{id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(!error["fingerprint"].as_str().unwrap().is_empty());
        assert!(error["known"].is_null());
    }
    for dir in [&logs, &vanilla] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_baseline_folder_works_and_a_missing_baseline_path_is_refused() {
    let core = common::core();
    let logs = tmp("baseline-dir-logs");
    write(
        &logs,
        "DayZServer_1.RPT",
        " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/a.c:46\r\n",
    );
    let vanilla = tmp("baseline-dir-vanilla");
    write(
        &vanilla,
        "DayZServer_0.RPT",
        " 3:40:00.001 SCRIPT    (E): JSON ERROR JM/CF/a.c:46\r\n",
    );

    let Outcome::Data(v) = dispatch(
        &core,
        "diag.errors",
        json!({ "dir": logs, "baseline": vanilla }),
    ) else {
        panic!("expected Data");
    };
    assert_eq!(v[0]["baseline"], json!(true), "{v}");

    let missing = tmp("baseline-missing");
    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.errors",
            json!({ "dir": logs, "baseline": missing }),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(&missing.display().to_string()),
        "the refusal names the path: {err}"
    );
    for dir in [&logs, &vanilla] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn diag_errors_marks_an_error_the_known_file_explains() {
    let core = common::core();
    let logs = tmp("known-errors-logs");
    write(
        &logs,
        "DayZServer_1.RPT",
        " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46\r\n",
    );
    let known = write(
        &tmp("known-errors-file"),
        "known.json",
        &json!([
            { "id": "cf000000", "pattern": "JSON ERROR JM/CF", "meaning": "CF logs one on every boot", "hint": "ignore it" }
        ])
        .to_string(),
    );

    let Outcome::Data(v) = dispatch(
        &core,
        "diag.errors",
        json!({ "dir": logs, "known_errors": known }),
    ) else {
        panic!("expected Data");
    };
    assert_eq!(v[0]["known"]["id"], json!("cf000000"), "{v}");
    assert_eq!(v[0]["known"]["meaning"], json!("CF logs one on every boot"));
    assert_eq!(v[0]["known"]["hint"], json!("ignore it"));
    let _ = std::fs::remove_dir_all(&logs);
}

#[test]
fn a_broken_file_names_itself_and_the_bad_entry() {
    let dir = tmp("known-bad");
    let cases = [
        ("not json", "not json".to_string(), ""),
        (
            "both matchers",
            json!([{ "id": "both0000", "pattern": "p", "fingerprint": "f" }]).to_string(),
            "both0000",
        ),
        (
            "neither matcher",
            json!([{ "id": "none0000" }]).to_string(),
            "none0000",
        ),
        (
            "a bad regex",
            json!([{ "id": "regex000", "pattern": "re:[unclosed" }]).to_string(),
            "regex000",
        ),
        (
            "a duplicate id",
            json!([
                { "id": "dup00000", "pattern": "a" },
                { "id": "dup00000", "pattern": "b" },
            ])
            .to_string(),
            "dup00000",
        ),
    ];
    for (n, (label, text, id)) in cases.into_iter().enumerate() {
        let path = write(&dir, &format!("bad-{n}.json"), &text);
        let message = match KnownErrors::load(Some(&path), None) {
            Ok(_) => panic!("{label}: a broken catalogue must be refused"),
            Err(error) => format!("{error:#}"),
        };
        let start = path.display().to_string();
        assert!(message.starts_with(&start), "{label}: {message}");
        if !id.is_empty() {
            assert!(message.contains(id), "{label}: {message}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_error_without_the_new_fields_still_loads() {
    let old = json!({
        "file": null,
        "logged_file": "a.c",
        "line": 3,
        "severity": "error",
        "message": "boom",
    });
    let error: MappedError = serde_json::from_value(old).unwrap();
    assert_eq!(error.message, "boom");
    assert_eq!(error.id, "");
    assert!(!error.baseline);
    assert!(error.known.is_none());
}

/// A project with one logged script error, returning its root, its log folder and the error's id.
fn learnable(label: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let root = tmp(label);
    let logs = root.join("profiles");
    write(
        &logs,
        "DayZServer_1.RPT",
        " 3:45:52.000 SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'x'\r\n",
    );
    let Outcome::Data(v) = dispatch(&common::core(), "diag.errors", json!({ "dir": logs })) else {
        panic!("expected Data");
    };
    let id = v[0]["id"].as_str().unwrap().to_owned();
    (root, logs, id)
}

fn learn(id: &str, root: &Path, logs: &Path) -> Value {
    json!({
        "dir": logs,
        "source_root": root,
        "id": id,
        "meaning": "a broken script",
        "hint": "fix the typo",
    })
}

fn fingerprint_of(core: &modfoundry_core::Core, logs: &Path) -> String {
    let Outcome::Data(v) = dispatch(core, "diag.errors", json!({ "dir": logs })) else {
        panic!("expected Data");
    };
    v[0]["fingerprint"].as_str().unwrap().to_owned()
}

#[test]
fn a_preview_of_a_learned_error_returns_the_change_set_and_writes_nothing() {
    let (root, logs, id) = learnable("learn-preview");
    let mut args = learn(&id, &root, &logs);
    args["preview"] = json!(true);
    let out = dispatch(&common::core(), "diag.learn_error", args);
    let Outcome::Preview(plan) = out else {
        panic!("expected a plan, got {out:?}");
    };
    assert_eq!(plan["changes"][0]["op"], json!("create"), "{plan}");
    assert!(
        !root.join(KNOWN_ERRORS_FILE).exists(),
        "a preview writes nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn learning_an_error_writes_its_entry_saves_it_for_undo_and_the_next_read_explains_it() {
    let core = common::core();
    let (root, logs, id) = learnable("learn-apply");
    let file = root.join(KNOWN_ERRORS_FILE);
    assert!(
        !file.parent().unwrap().exists(),
        "the folder does not exist yet"
    );

    let Outcome::Changed {
        change_set: Some(change_set),
        summary,
        ..
    } = dispatch(&core, "diag.learn_error", learn(&id, &root, &logs))
    else {
        panic!("expected a recorded change");
    };
    assert!(summary.description.contains(&id), "{}", summary.description);

    let entries: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], json!(id));
    assert_eq!(entries[0]["meaning"], json!("a broken script"));
    assert_eq!(entries[0]["hint"], json!("fix the typo"));
    assert!(entries[0].get("pattern").is_none(), "{entries:?}");
    assert!(!entries[0]["fingerprint"].as_str().unwrap().is_empty());

    let Outcome::Data(v) = dispatch(
        &core,
        "diag.errors",
        json!({ "dir": logs, "source_root": root }),
    ) else {
        panic!("expected Data");
    };
    assert_eq!(v[0]["known"]["id"], json!(id), "{v}");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert!(!file.exists(), "undo removes the file the learn created");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn learning_the_same_error_again_is_refused_naming_the_existing_entry() {
    let core = common::core();
    let (root, logs, id) = learnable("learn-again");
    let file = root.join(KNOWN_ERRORS_FILE);
    write(
        &root.join("server"),
        "known-errors.json",
        &json!([{
            "id": "existing",
            "fingerprint": fingerprint_of(&core, &logs),
            "meaning": "",
            "hint": "",
        }])
        .to_string(),
    );

    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            learn(&id, &root, &logs),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("existing"), "{err}");
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(&std::fs::read_to_string(&file).unwrap())
            .unwrap()
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn learning_an_unknown_id_a_generic_error_or_with_no_location_is_refused() {
    let core = common::core();
    let (root, logs, _) = learnable("learn-refused");

    let unknown = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            learn("nope0000", &root, &logs),
        )
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("nope0000"), "{unknown}");

    let no_location = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            json!({ "dir": logs, "id": "nope0000" }),
        )
        .unwrap_err()
        .to_string();
    assert!(no_location.contains("known_errors"), "{no_location}");
    assert!(no_location.contains("source_root"), "{no_location}");

    let generic_logs = root.join("generic");
    write(
        &generic_logs,
        "DayZServer_1.RPT",
        " 3:45:52.000 SCRIPT    (E): Virtual Machine Exception\r\n\r\nStack trace:\r\nMyMod/scripts/a.c:12 Function F\r\n",
    );
    let Outcome::Data(v) = dispatch(&core, "diag.errors", json!({ "dir": generic_logs })) else {
        panic!("expected Data");
    };
    let id = v[0]["id"].as_str().unwrap().to_owned();
    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            learn(&id, &root, &generic_logs),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("Virtual Machine Exception"), "{err}");
    assert!(!root.join(KNOWN_ERRORS_FILE).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn learning_a_second_error_keeps_the_first_entry() {
    let core = common::core();
    let (root, logs, id) = learnable("learn-two");
    dispatch(&core, "diag.learn_error", learn(&id, &root, &logs));

    write(
        &logs,
        "DayZServer_2.RPT",
        " 3:45:53.000 SCRIPT    (E): @\"MyMod/scripts/3_Game/other.c,9\": Undefined function 'Y'\r\n",
    );
    let Outcome::Data(v) = dispatch(&core, "diag.errors", json!({ "dir": logs })) else {
        panic!("expected Data");
    };
    let second = v
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["message"]
                .as_str()
                .unwrap()
                .contains("Undefined function")
        })
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    dispatch(&core, "diag.learn_error", learn(&second, &root, &logs));
    let entries: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(root.join(KNOWN_ERRORS_FILE)).unwrap())
            .unwrap();
    let ids: Vec<&str> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [id.as_str(), second.as_str()]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn learning_an_id_a_hand_written_entry_already_uses_is_refused() {
    let core = common::core();
    let (root, logs, id) = learnable("learn-same-id");
    write(
        &root.join("server"),
        "known-errors.json",
        &json!([{ "id": id, "pattern": "something else", "meaning": "", "hint": "" }]).to_string(),
    );

    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            learn(&id, &root, &logs),
        )
        .unwrap_err()
        .to_string();

    assert!(err.contains(&format!("entry {id} already exists")), "{err}");
    assert!(KnownErrors::load(None, Some(&root)).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_empty_file_is_an_empty_catalogue_and_an_unreadable_one_is_an_error() {
    let dir = tmp("known-read");
    let empty = catalogue(&dir, "");
    assert!(KnownErrors::load(Some(&empty), None).is_ok());

    let folder = dir.join("not-a-file");
    std::fs::create_dir_all(&folder).unwrap();
    let err = KnownErrors::load(Some(&folder), None)
        .unwrap_err()
        .to_string();
    assert!(err.starts_with(&folder.display().to_string()), "{err}");
}

#[test]
fn learning_into_a_file_that_cannot_be_read_is_refused_and_writes_nothing() {
    let core = common::core();
    let (root, logs, id) = learnable("learn-unreadable");
    std::fs::create_dir_all(root.join(KNOWN_ERRORS_FILE)).unwrap();

    let err = core
        .dispatch(
            Actor::CliSession,
            "diag.learn_error",
            learn(&id, &root, &logs),
        )
        .unwrap_err()
        .to_string();

    assert!(err.contains("known-errors.json"), "{err}");
    assert!(root.join(KNOWN_ERRORS_FILE).is_dir());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_entry_with_a_key_of_its_own_still_loads() {
    let dir = tmp("known-extra-key");
    let path = catalogue(
        &dir,
        &json!([{ "id": "aaaa1111", "pattern": "JSON ERROR", "meaning": "", "hint": "", "owner": "cf" }])
            .to_string(),
    );
    assert!(KnownErrors::load(Some(&path), None).is_ok());
}
