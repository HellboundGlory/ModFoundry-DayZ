//! `mf changelog` prints the embedded changelog, optionally only newer releases.

use crate::binary::{mf, stderr, stdout};

#[test]
fn mf_changelog_prints_the_changelog() {
    let out = mf(&["changelog"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Changelog"), "{text}");
    assert!(
        text.contains(&format!("## {} - ", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
}

#[test]
fn mf_changelog_since_prints_only_newer_releases() {
    let out = mf(&["changelog", "--since", "0.0.1"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let newest = format!("## {} - ", env!("CARGO_PKG_VERSION"));
    assert!(text.starts_with(&newest), "{text}");
    assert!(!text.contains("# Changelog"), "{text}");
}

#[test]
fn mf_changelog_since_the_newest_says_nothing_changed() {
    let out = mf(&["changelog", "--since", "999.0.0"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "no changes after 999.0.0\n");
}

#[test]
fn mf_changelog_rejects_a_bad_version() {
    let out = mf(&["changelog", "--since", "banana"]);

    assert!(!out.status.success());
    assert!(stderr(&out).contains("version"), "{}", stderr(&out));
}
