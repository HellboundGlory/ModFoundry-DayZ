//! `mf update` outside a release install refuses with the reason.

use crate::binary::{mf, stderr, stdout};

#[test]
fn mf_update_outside_a_release_install_refuses_with_the_reason() {
    let out = mf(&["update"]);

    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("not installed from a release package"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn mf_update_check_outside_a_release_install_prints_the_reason() {
    let out = mf(&["update", "--check"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("not installed from a release package"),
        "{}",
        stdout(&out)
    );
}
