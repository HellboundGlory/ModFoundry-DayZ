//! The update notice in the server's `initialize` instructions, from a release install only.

#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const INITIALIZE: &str =
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
const NEWER: &str = "99.0.0";

fn notice() -> String {
    format!(
        "ModFoundry {NEWER} is out (this is {}): run mf update",
        env!("CARGO_PKG_VERSION")
    )
}

/// A folder holding a hard link to the server beside the install marker a release package ships.
struct Install {
    dir: PathBuf,
}

impl Install {
    fn new(label: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("mf-mcp-update-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::hard_link(
            env!("CARGO_BIN_EXE_modfoundry-mcp-server"),
            dir.join("server"),
        )
        .unwrap();
        std::fs::write(dir.join(".modfoundry-install"), "").unwrap();
        Self { dir }
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A scratch home holding the database and the cache, removed when it goes.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mf-mcp-home-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn save_check(&self, latest: &str) {
        let folder = self.dir.join("cache/modfoundry");
        std::fs::create_dir_all(&folder).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let state = format!(r#"{{"checked_at":{now},"latest":{latest}}}"#);
        std::fs::write(folder.join("update-check.json"), state).unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The `initialize` result of a release install whose cache is `home`.
fn initialize_result(install: &Install, home: &Home) -> Value {
    let mut child = Command::new(install.dir.join("server"))
        .env("MODFOUNDRY_DB", home.dir.join("modfoundry.sqlite"))
        .env("HOME", &home.dir)
        .env("XDG_CACHE_HOME", home.dir.join("cache"))
        .env("XDG_DATA_HOME", &home.dir)
        .env("MODFOUNDRY_DATA_DIR", home.dir.join("modfoundry"))
        .env_remove("MF_NO_UPDATE_CHECK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("server starts");
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(INITIALIZE.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let line = String::from_utf8(out.stdout).unwrap();
    let response: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    response["result"].clone()
}

#[test]
fn initialize_says_what_modfoundry_is() {
    let install = Install::new("fixed");
    let home = Home::new("fixed");
    home.save_check("null");
    let text = initialize_result(&install, &home)["instructions"]
        .as_str()
        .expect("instructions")
        .to_owned();
    assert!(text.starts_with("ModFoundry"), "{text}");
    assert!(text.contains("host.doctor"), "{text}");
    assert!(text.contains("host.update_check"), "{text}");
    assert!(!text.contains("is out"), "{text}");
}

#[test]
fn initialize_ends_with_the_saved_notice() {
    let install = Install::new("notice");
    let home = Home::new("notice");
    home.save_check(&format!("\"{NEWER}\""));
    let text = initialize_result(&install, &home)["instructions"]
        .as_str()
        .expect("instructions")
        .to_owned();
    assert!(text.ends_with(&format!("\n\n{}", notice())), "{text}");
}
