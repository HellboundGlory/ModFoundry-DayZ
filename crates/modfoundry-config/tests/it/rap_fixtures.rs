//! RaP (config.bin) parity fixtures.
//!
//! The `.bin`/`.txt` pairs under `data/rap/` are the official `CfgConvert.exe`
//! output (`-bin` from an authored config.cpp, `-txt` from that bin), so these
//! tests pin byte-level compatibility with the shipped DayZ tooling:
//! `to_text(read_rap(bin)) == txt` and `write_rap(read_rap(bin)) == bin`.

use modfoundry_config::ast::{ConfigFile, Member, Value};
use modfoundry_config::{read_rap, to_text, write_rap};

const FLAGSHIP_BIN: &[u8] = include_bytes!("../data/rap/flagship.bin");
const FLAGSHIP_TXT: &[u8] = include_bytes!("../data/rap/flagship.txt");
const QUIRKS_BIN: &[u8] = include_bytes!("../data/rap/quirks.bin");
const QUIRKS_TXT: &[u8] = include_bytes!("../data/rap/quirks.txt");

fn class<'a>(file: &'a ConfigFile, path: &[&str]) -> &'a modfoundry_config::ast::ConfigClass {
    let mut cur = file.find_class(path[0]).expect("top-level class");
    for name in &path[1..] {
        cur = cur.child_class(name).expect("nested class");
    }
    cur
}

#[test]
fn flagship_text_matches_cfgconvert() {
    let file = read_rap(FLAGSHIP_BIN).expect("reads");
    assert_eq!(to_text(&file).as_bytes(), FLAGSHIP_TXT);
}

#[test]
fn flagship_bytes_match_cfgconvert() {
    let file = read_rap(FLAGSHIP_BIN).expect("reads");
    assert_eq!(write_rap(&file).expect("writes"), FLAGSHIP_BIN);
}

#[test]
fn flagship_shapes() {
    let file = read_rap(FLAGSHIP_BIN).expect("reads");

    let forward = class(&file, &["CfgFixture", "Inherited"]);
    assert!(forward.forward && forward.members.is_empty());

    let plain = class(&file, &["CfgFixture", "Plain"]);
    let big = plain.property("bigInt").expect("bigInt");
    assert_eq!(big.value, Some(Value::Number("100000000000".into())));
    let short = plain.property("shortFloat").expect("shortFloat");
    assert_eq!(short.value, Some(Value::Float(2.0)));
    let text = plain.property("text").expect("text");
    assert_eq!(text.value, Some(Value::Str("plain".into())));
    let multi = plain.property("multiLine").expect("multiLine");
    assert_eq!(
        multi.value,
        Some(Value::Str("first line\nsecond line".into()))
    );

    let empty = plain.property("emptyArray").expect("emptyArray");
    assert!(empty.declared_array);
    assert_eq!(empty.value, Some(Value::Array(vec![])));

    let numbers = plain.property("numbers").expect("numbers");
    assert_eq!(
        numbers.value,
        Some(Value::Array(vec![
            Value::Number("1".into()),
            Value::Number("2".into()),
            Value::Number("3".into()),
        ]))
    );

    let appended = plain.property("appended").expect("appended");
    assert!(appended.array_append && appended.declared_array);
    assert_eq!(
        appended.value,
        Some(Value::Array(vec![
            Value::Number("7".into()),
            Value::Number("8".into()),
        ]))
    );

    let deep = plain.property("deep").expect("deep");
    assert_eq!(
        deep.value,
        Some(Value::Array(vec![Value::Array(vec![Value::Array(vec![
            Value::Number("1".into())
        ])])]))
    );

    let empty_class = class(&file, &["CfgFixture", "Plain", "Empty"]);
    assert!(empty_class.members.is_empty() && !empty_class.forward);

    let child = class(&file, &["CfgFixture", "Plain", "Child"]);
    assert_eq!(child.parents, vec!["Inherited".to_string()]);
    assert!(matches!(child.members[0], Member::Delete { .. }));
}

#[test]
fn quirks_text_matches_cfgconvert() {
    let file = read_rap(QUIRKS_BIN).expect("reads");
    assert_eq!(to_text(&file).as_bytes(), QUIRKS_TXT);
}

#[test]
fn quirks_bytes_match_cfgconvert() {
    let file = read_rap(QUIRKS_BIN).expect("reads");
    assert_eq!(write_rap(&file).expect("writes"), QUIRKS_BIN);
}

#[test]
fn quirks_enum_table_is_kept_out_of_text() {
    let file = read_rap(QUIRKS_BIN).expect("reads");
    assert_eq!(
        file.enums
            .iter()
            .map(|e| (e.name.as_str(), e.value))
            .collect::<Vec<_>>(),
        vec![
            ("destructno", 0),
            ("destructengine", 2),
            ("destructdefault", 6)
        ]
    );
    assert!(!to_text(&file).contains("destruct"));
}
