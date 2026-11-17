//! `host.changelog` through the registry.

use crate::common;
use modfoundry_core::{Actor, CommandError, Outcome};
use serde_json::{json, Value};

fn dispatch(args: Value) -> Result<Outcome, CommandError> {
    common::core().dispatch(Actor::CliSession, "host.changelog", args)
}

fn data(args: Value) -> Value {
    match dispatch(args).unwrap() {
        Outcome::Data(answer) => answer,
        other => panic!("expected Data, got {other:?}"),
    }
}

#[test]
fn host_changelog_answers_the_embedded_file() {
    let answer = data(json!({}));

    assert!(answer["text"].as_str().unwrap().starts_with("# Changelog"));
    assert!(
        answer["versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version == env!("CARGO_PKG_VERSION")),
        "{answer}"
    );
    assert!(answer["since"].is_null(), "{answer}");
}

#[test]
fn host_changelog_since_a_far_future_version_answers_nothing() {
    let answer = data(json!({ "since": "999.0.0" }));

    assert_eq!(answer["text"], "");
    assert_eq!(answer["versions"], json!([]));
    assert_eq!(answer["since"], "999.0.0");
}

#[test]
fn host_changelog_since_an_old_version_answers_the_newest_sections() {
    let answer = data(json!({ "since": "0.0.1" }));

    let text = answer["text"].as_str().unwrap();
    let newest = format!("## {} - ", env!("CARGO_PKG_VERSION"));
    assert!(text.starts_with(&newest), "{text}");
    assert!(!text.contains("# Changelog"), "{text}");
}

#[test]
fn host_changelog_rejects_a_bad_since() {
    for since in [json!("banana"), json!(7)] {
        let error = dispatch(json!({ "since": since })).expect_err("a bad since");
        assert!(
            matches!(error, CommandError::InvalidArguments(_)),
            "{since}: {error:?}"
        );
    }
}
