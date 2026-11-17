//! Contract tests for the shared frontend command bodies.
//!
//! These pin the operations both the CLI and MCP bind, so a bug fixed here is
//! fixed in every frontend. Scaffold→validate→lint round-trips plus
//! the raw `read_text` used by `vfs.read`.

use crate::common::{tmp, write};
use modfoundry_commands::{lint_target, read_text, scaffold, validate_file};

fn codes_of(diags: &[modfoundry_config::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

#[test]
fn read_text_returns_contents_and_errors_on_missing() {
    let file = tmp("read");
    let p = write(&file, "Scripts/3_Game/A.c", "class A {}\n");
    assert_eq!(read_text(&p).unwrap(), "class A {}\n");
    assert!(read_text(&file.join("nope.c")).is_err());
    let _ = std::fs::remove_dir_all(&file);
}

#[test]
fn scaffolded_tree_validates_and_lints_clean() {
    let dir = tmp("roundtrip");
    scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod")).expect("scaffold succeeds");
    assert!(validate_file(&dir).expect("validate").1.is_empty());
    assert!(lint_target(&dir).expect("lint").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn planted_bad_script_reports_mf_e1001() {
    let dir = tmp("badscript");
    scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod")).expect("scaffold succeeds");
    write(&dir, "Scripts/3_Game/Broken.c", "}");
    let findings = lint_target(&dir).expect("lint");
    assert!(findings.iter().any(|f| f.diagnostic.code == "MF-E1001"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn planted_bad_config_reports_mf_c310() {
    let dir = tmp("badconfig");
    scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod")).expect("scaffold succeeds");
    let cfg = dir.join("config.cpp");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("Scripts/4_World", "Scripts/3_Game")).unwrap();
    let diags = validate_file(&dir).expect("validate").1;
    assert!(codes_of(&diags).contains(&"MF-C310".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}
