//! `mf conflict-check` end to end.

use crate::binary::{mf, stderr, stdout, Temp};

const SHARED: &str = "class Shared\n{\n}\n";
const MODDED_ONLY: &str = "modded class PlayerBase\n{\n}\n";
const WITHOUT_SUPER: &str =
    "modded class PlayerBase\n{\n\toverride void OnConnect()\n\t{\n\t}\n}\n";

fn two_mods(dir: &Temp, script: &str) -> (String, String) {
    let a = dir.join("A");
    let b = dir.join("B");
    for root in [&a, &b] {
        std::fs::create_dir_all(root.join("Scripts/4_World")).unwrap();
        std::fs::write(root.join("Scripts/4_World/m.c"), script).unwrap();
    }
    (
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
    )
}

#[test]
fn a_shared_class_exits_one_and_prints_the_finding_with_its_places() {
    let dir = Temp::new("conflict-cli-shared");
    let (a, b) = two_mods(&dir, SHARED);

    let out = mf(&["conflict-check", &a, &b]);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(stderr(&out), "", "no warnings");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0], "load order: A, B",
        "the report names the order used: {text}"
    );
    assert!(lines[1].starts_with("error MF-M101 "), "got: {text}");
    assert!(
        lines[1].contains("Shared is declared by A, B; only one mod can declare it"),
        "got: {text}"
    );
    assert_eq!(lines[2], "  A: Scripts/4_World/m.c:1", "{text}");
    assert_eq!(lines[3], "  B: Scripts/4_World/m.c:1", "{text}");
    assert_eq!(
        lines.last(),
        Some(&"1 errors, 0 warnings, 0 hints across 2 mods"),
        "{text}"
    );
}

#[test]
fn a_shared_modded_class_alone_is_a_hint_and_exits_zero() {
    let dir = Temp::new("conflict-cli-hint");
    let (a, b) = two_mods(&dir, MODDED_ONLY);

    let out = mf(&["conflict-check", &a, &b]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}\n{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("hint MF-M102 class PlayerBase is modded by A, B"),
        "got: {}",
        stdout(&out)
    );
}

#[test]
fn a_cut_off_override_is_an_error_naming_the_mod_it_cuts_off() {
    let dir = Temp::new("conflict-cli-cut");
    let a = dir.join("A");
    let b = dir.join("B");
    for (root, script) in [
        (
            &a,
            "modded class PlayerBase\n{\n\toverride void OnConnect() { super.OnConnect(); }\n}\n",
        ),
        (&b, WITHOUT_SUPER),
    ] {
        std::fs::create_dir_all(root.join("Scripts/4_World")).unwrap();
        std::fs::write(root.join("Scripts/4_World/m.c"), script).unwrap();
    }

    let out = mf(&["conflict-check", a.to_str().unwrap(), b.to_str().unwrap()]);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "error MF-M104 B overrides PlayerBase.OnConnect() without calling super.OnConnect, \
             which cuts off A"
        ),
        "got: {}",
        stdout(&out)
    );
}

#[test]
fn json_prints_the_report() {
    let dir = Temp::new("conflict-cli-json");
    let (a, b) = two_mods(&dir, SHARED);

    let out = mf(&["conflict-check", &a, &b, "--json"]);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let value: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON output");
    assert_eq!(value["findings"][0]["code"], "MF-M101", "{value}");
    assert_eq!(
        value["load_order"],
        serde_json::json!(["A", "B"]),
        "{value}"
    );
}

#[test]
fn a_lone_mod_and_a_missing_set_file_exit_two() {
    let dir = Temp::new("conflict-cli-refused");
    let (a, _) = two_mods(&dir, SHARED);

    let one = mf(&["conflict-check", &a]);
    assert_eq!(one.status.code(), Some(2), "{}", stdout(&one));
    assert!(
        stderr(&one).contains("two or more mods"),
        "got: {}",
        stderr(&one)
    );

    let no_sets = dir.join("project");
    std::fs::create_dir_all(&no_sets).unwrap();
    let out = mf(&[
        "conflict-check",
        "--set",
        "x",
        "--root",
        no_sets.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("launch-sets.json is missing"),
        "got: {}",
        stderr(&out)
    );
}

#[cfg(unix)]
#[test]
fn a_mod_folder_argument_that_is_not_utf8_is_checked_without_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let dir = Temp::new("conflict-cli-non-utf8");
    let odd = dir.join("").join(std::ffi::OsStr::from_bytes(b"A\xff"));
    std::fs::create_dir_all(odd.join("Scripts/4_World")).unwrap();
    std::fs::write(odd.join("Scripts/4_World/m.c"), SHARED).unwrap();
    let (_, b) = two_mods(&dir, SHARED);
    let db = dir.join("mf.db");

    let out = crate::binary::mf_command(&db)
        .arg("conflict-check")
        .arg(&odd)
        .arg(&b)
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}
