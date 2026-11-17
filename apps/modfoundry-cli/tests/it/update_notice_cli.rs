//! The update notice: one stderr line from a release install, and the last line of `mf doctor`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::binary::{mf, mf_command_at, mf_in, stderr, stdout, Temp};

const NEWER: &str = "99.0.0";

fn notice() -> String {
    format!(
        "ModFoundry {NEWER} is out (this is {}): run mf update",
        env!("CARGO_PKG_VERSION")
    )
}

/// A folder holding a hard link to `mf` beside the install marker a release package ships.
struct Install {
    dir: PathBuf,
}

impl Install {
    fn new(label: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("mf-update-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::hard_link(env!("CARGO_BIN_EXE_mf"), dir.join("mf")).unwrap();
        std::fs::write(dir.join(".modfoundry-install"), "").unwrap();
        Self { dir }
    }

    fn program(&self) -> PathBuf {
        self.dir.join("mf")
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Saves a check made just now that found `latest`, in the cache folder `mf` reads under `db`.
fn save_check(db: &Temp, latest: &str) {
    let folder = db.join("home/cache/modfoundry");
    std::fs::create_dir_all(&folder).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let state = format!(r#"{{"checked_at":{now},"latest":"{latest}"}}"#);
    std::fs::write(folder.join("update-check.json"), state).unwrap();
}

fn run_release(install: &Install, db: &Temp, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = mf_command_at(&install.program(), &db.join("modfoundry.sqlite"));
    cmd.env_remove("MF_NO_UPDATE_CHECK").args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("mf runs")
}

#[test]
fn a_release_install_prints_the_saved_notice_on_stderr() {
    let install = Install::new("notice");
    let db = Temp::new("update-notice");
    save_check(&db, NEWER);
    let out = run_release(&install, &db, &["formats"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), format!("{}\n", notice()));
    let plain = Temp::new("update-plain");
    let plain_out = mf_in(&plain.join("modfoundry.sqlite"), &["formats"]);
    assert_eq!(stdout(&out), stdout(&plain_out));
}

#[test]
fn the_switch_turns_the_notice_off() {
    let install = Install::new("switch");
    let db = Temp::new("update-switch");
    save_check(&db, NEWER);
    let out = run_release(&install, &db, &["formats"], &[("MF_NO_UPDATE_CHECK", "1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn a_build_outside_a_release_install_prints_nothing() {
    let db = Temp::new("update-source");
    save_check(&db, NEWER);
    let out = mf_in(&db.join("modfoundry.sqlite"), &["formats"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
}

#[test]
fn doctor_ends_with_the_notice() {
    let install = Install::new("doctor");
    let db = Temp::new("update-doctor");
    save_check(&db, NEWER);
    let out = run_release(&install, &db, &["doctor"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).lines().last(), Some(notice().as_str()));
}

#[test]
fn doctor_and_update_help_name_the_switch() {
    for command in ["doctor", "update"] {
        let out = mf(&[command, "--help"]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(
            stdout(&out).contains("MF_NO_UPDATE_CHECK=1"),
            "{command}: {}",
            stdout(&out)
        );
    }
}
