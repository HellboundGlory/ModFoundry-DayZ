use modfoundry_gamediff::config::config_model;
use modfoundry_gamediff::layout::layout_model;
use modfoundry_gamediff::script::script_model;
use modfoundry_gamediff::snapshot::{Snapshot, SNAPSHOT_SCHEMA};

fn snapshot(build: &str) -> Snapshot {
    let scripts = script_model(&[(
        "scripts/3_Game/a.c".to_owned(),
        "class A : B\n{\n\tvoid F(int x) {}\n}\n".to_owned(),
    )]);
    let parsed = modfoundry_config::parser::parse("class CfgVehicles { class A { mass = 10; }; };");
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let configs = config_model(&[("config.cpp".to_owned(), parsed.file)]);
    let layouts = layout_model(&[(
        "gui/layouts/a.layout".to_owned(),
        "FrameWidgetClass Root {\n position 0 0\n}\n".to_owned(),
    )]);
    Snapshot::new(build.to_owned(), scripts, configs, layouts)
}

#[test]
fn a_snapshot_round_trips_and_starts_with_its_schema() {
    let snapshot = snapshot("1.30.1.2");

    let text = snapshot.to_json().unwrap();

    assert_eq!(snapshot.schema, SNAPSHOT_SCHEMA);
    assert!(
        text.starts_with("{\"schema\":1,\"build\":\"1.30.1.2\""),
        "{text}"
    );
    assert_eq!(Snapshot::from_json(&text).unwrap(), snapshot);
}

#[test]
fn snapshots_that_cannot_be_read_say_why() {
    let text = snapshot("1.30.1.2").to_json().unwrap();
    let newer = text.replacen("\"schema\":1", "\"schema\":2", 1);

    let cases: [(&str, &str); 4] = [
        ("not a game snapshot", "{}"),
        ("not a game snapshot", "[1]"),
        ("snapshot schema 2 is newer than this mf reads (1)", &newer),
        ("", "{\"schema\":1}"),
    ];

    for (expected, text) in cases {
        let error = Snapshot::from_json(text).unwrap_err();
        if expected.is_empty() {
            assert!(!error.is_empty(), "no message for {text}");
        } else {
            assert_eq!(error, expected, "{text}");
        }
    }
}
