//! `mf game-diff` end to end.

use crate::binary::{mf, stderr, stdout, Temp};

const SCRIPT_OLD: &str = "class A\n{\n\tvoid F() {}\n\n\tvoid G() {}\n}\n";
const SCRIPT_NEW: &str = "class A\n{\n\tvoid F() {}\n}\n";

fn two_folders(dir: &Temp) -> (String, String) {
    let old = dir.join("old");
    let new = dir.join("new");
    for (root, script) in [(&old, SCRIPT_OLD), (&new, SCRIPT_NEW)] {
        let file = root.join("scripts/3_Game/a.c");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, script).unwrap();
    }
    (
        old.to_string_lossy().into_owned(),
        new.to_string_lossy().into_owned(),
    )
}

#[test]
fn game_diff_exit_codes() {
    let dir = Temp::new("game-diff-cli");
    let (old, new) = two_folders(&dir);
    let snapshots = dir.join("snapshots");
    let snapshots = snapshots.to_str().unwrap();

    let out = mf(&["game-diff", &old, &new, "--snapshots", snapshots]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let summary = "1 script, 0 config and 0 layout changes; 0 breaks, 0 to check";
    assert!(stdout(&out).contains(summary), "got: {}", stdout(&out));

    let json = mf(&["game-diff", &old, &new, "--snapshots", snapshots, "--json"]);
    assert_eq!(json.status.code(), Some(0), "{}", stderr(&json));
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("JSON output");
    assert_eq!(value["summary"]["scripts"], 1, "{value}");

    let missing = dir.join("nope");
    let out = mf(&[
        "game-diff",
        missing.to_str().unwrap(),
        &new,
        "--snapshots",
        snapshots,
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("old side:"), "got: {}", stderr(&out));
}

#[test]
fn a_breaking_mod_exits_one_and_an_unreadable_mod_two() {
    let dir = Temp::new("game-diff-cli-mod");
    let (old, new) = two_folders(&dir);
    let snapshots = dir.join("snapshots");
    let snapshots = snapshots.to_str().unwrap();
    let mod_root = dir.join("MyMod");
    let file = mod_root.join("scripts/4_World/m.c");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "modded class A\n{\n\toverride void G() {}\n}\n").unwrap();
    let mod_root = mod_root.to_str().unwrap();

    let out = mf(&[
        "game-diff",
        &old,
        &new,
        "--snapshots",
        snapshots,
        "--against",
        mod_root,
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Hits in MyMod"), "got: {text}");
    assert!(
        text.contains("breaks  scripts/4_World/m.c:3  override removed  A.G()"),
        "got: {text}"
    );

    let missing = dir.join("NoMod");
    let missing = missing.to_str().unwrap();
    let out = mf(&[
        "game-diff",
        &old,
        &new,
        "--snapshots",
        snapshots,
        "--against",
        missing,
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stderr(&out).matches(missing).count(),
        1,
        "the mod folder is named once: {}",
        stderr(&out)
    );
}
