//! `mf publish` and `mf publish-items` against a script standing in for the Steam helper.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::binary::{stderr, stdout, Temp};

/// The helper answers the account request every publish starts with.
const ACCOUNT: &str = r#"*'"request":"account"'*) echo '{"event":"account","name":"builder","steam_id":76561198000000001}' ;;"#;

/// A folder holding a hard link to `mf` and the helper script it finds beside it.
struct Rig {
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A rig whose helper answers the requests matching `arms` (the body of its `case`).
fn rig(label: &str, arms: &str) -> Rig {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mf-publish-{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_mf"));
    let link = dir.join("mf");
    std::fs::hard_link(binary, &link).unwrap_or_else(|e| {
        panic!(
            "could not link {} to {}: {e}",
            binary.display(),
            link.display()
        )
    });
    let helper = dir.join("modfoundry-steam");
    std::fs::write(
        &helper,
        format!(
            "#!/bin/sh\n\
             [ \"$1\" = \"--probe\" ] && exit 0\n\
             here=$(dirname \"$0\")\n\
             read line\n\
             echo \"$line\" >> \"$here/requests.log\"\n\
             case \"$line\" in\n\
             {ACCOUNT}\n\
             {arms}\n\
             esac\n"
        ),
    )
    .unwrap();
    set_executable(&helper);
    wait_until_runnable(&helper);
    Rig { dir }
}

fn set_executable(program: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(program, PermissionsExt::from_mode(0o755)).unwrap();
}

/// Runs a just-written helper until exec stops failing with "Text file busy": a fork elsewhere in
/// the test binary can hold the file open, and `mf` would then fail to start its helper.
fn wait_until_runnable(program: &Path) {
    for _ in 0..200 {
        match Command::new(program).arg("--probe").output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", program.display());
}

/// The requests the helper was sent, in order.
fn requests(rig: &Rig) -> Vec<String> {
    std::fs::read_to_string(rig.dir.join("requests.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `mf` from the rig, pointed at its own database and home.
fn mf_with(rig: &Rig, db: &Path) -> Command {
    crate::binary::mf_command_at(&rig.dir.join("mf"), db)
}

/// The key the publishes sign their builds with, made once per test run.
fn keys() -> &'static Path {
    static KEYS: OnceLock<PathBuf> = OnceLock::new();
    KEYS.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("mf-publish-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        modfoundry_commands::keygen("MF", &dir, None).unwrap();
        dir
    })
}

fn publish_flags(keys: &Path) -> [String; 8] {
    [
        "--name".into(),
        "MyMod".into(),
        "--profile".into(),
        "pub".into(),
        "--authority".into(),
        "MF".into(),
        "--keys-dir".into(),
        keys.display().to_string(),
    ]
}

/// A scaffolded `MyMod` with the given `workshop/item.json`; the `Temp` keeps it alive.
fn ready_mod(label: &str, item_json: &str) -> (Temp, PathBuf) {
    let base = Temp::new(label);
    let dir = base.join("MyMod");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::create_dir_all(dir.join("workshop")).unwrap();
    std::fs::write(dir.join("workshop/item.json"), item_json).unwrap();
    (base, dir)
}

fn saved_item(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("workshop/item.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Runs `mf publish` on `dir` and sends SIGINT once the helper touches its `started` marker.
fn publish_and_interrupt(rig: &Rig, db: &Temp, dir: &Path) -> Output {
    let started = rig.dir.join("started");
    let mut child = mf_with(rig, &db.join("modfoundry.sqlite"))
        .arg("publish")
        .arg(dir)
        .args(publish_flags(keys()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mf starts");

    wait_for(&started, &mut child, "the upload never started");
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill runs");
    wait_for_exit(&mut child);

    child.wait_with_output().unwrap()
}

/// Polls until `marker` exists, killing and failing after 20 s.
fn wait_for(marker: &Path, child: &mut Child, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("{message}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Polls until `child` exits, killing and failing after 20 s.
fn wait_for_exit(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("mf did not stop after Ctrl+C");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn ctrl_c_during_the_upload_deletes_the_new_item() {
    let rig = rig(
        "cancel",
        r#"*'"request":"create"'*) echo '{"event":"created","id":55,"needs_legal_agreement":false}' ;;
*'"request":"update"'*) touch "$here/started"; exec sleep 30 ;;
*'"request":"delete"'*) echo '{"event":"deleted","id":55}' ;;"#,
    );
    let (_base, dir) = ready_mod(
        "publish-cancel",
        r#"{"title":"Field Kit","visibility":"private"}"#,
    );
    let db = Temp::new("publish-cancel-db");

    let out = publish_and_interrupt(&rig, &db, &dir);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("cancelling the upload"), "{err}");
    assert!(
        err.contains("the upload failed: cancelled; the new item 55 was deleted"),
        "{err}"
    );
    assert_eq!(
        requests(&rig).last().map(String::as_str),
        Some(r#"{"request":"delete","id":55}"#)
    );
    let saved = saved_item(&dir);
    assert!(saved.get("id").is_none(), "{saved}");
}

#[test]
fn a_second_ctrl_c_stops_mf_in_a_step_that_cannot_be_cancelled() {
    let rig = rig(
        "create-stall",
        r#"*'"request":"create"'*) touch "$here/started"; exec sleep 30 ;;"#,
    );
    let (_base, dir) = ready_mod(
        "publish-create-stall",
        r#"{"title":"Field Kit","visibility":"private"}"#,
    );
    let db = Temp::new("publish-create-stall-db");
    let mut child = mf_with(&rig, &db.join("modfoundry.sqlite"))
        .arg("publish")
        .arg(&dir)
        .args(publish_flags(keys()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mf starts");
    wait_for(
        &rig.dir.join("started"),
        &mut child,
        "the item was never asked for",
    );

    for _ in 0..2 {
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("kill runs");
        std::thread::sleep(Duration::from_millis(300));
    }
    wait_for_exit(&mut child);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130), "{}", stderr(&out));
    assert!(stderr(&out).contains("interrupted"), "{}", stderr(&out));
}

#[test]
fn ctrl_c_after_the_helper_submitted_keeps_the_upload() {
    let rig = rig(
        "submitted",
        r#"*'"request":"update"'*) echo '{"event":"submitted","id":7,"needs_legal_agreement":false}'
touch "$here/started"; exec sleep 30 ;;"#,
    );
    let (_base, dir) = ready_mod("publish-submitted", r#"{"id":7,"title":"Field Kit"}"#);
    let db = Temp::new("publish-submitted-db");

    let out = publish_and_interrupt(&rig, &db, &dir);

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("published \"Field Kit\" -> "), "{text}");
    assert!(text.contains("(updated)"), "{text}");
    let sent = requests(&rig);
    assert!(
        !sent
            .iter()
            .any(|line| line.contains(r#""request":"delete""#)),
        "{sent:?}"
    );
}

#[test]
fn publish_prints_the_result_lines_for_an_updated_item() {
    let rig = rig(
        "result",
        r#"*'"request":"update"'*) echo '{"event":"progress","stage":"uploading_content","done":1,"total":2}'
echo '{"event":"submitted","id":7,"needs_legal_agreement":false}' ;;"#,
    );
    let (_base, dir) = ready_mod("publish-result", r#"{"id":7,"title":"Field Kit"}"#);
    let db = Temp::new("publish-result-db");

    let out = mf_with(&rig, &db.join("modfoundry.sqlite"))
        .arg("publish")
        .arg(&dir)
        .args(publish_flags(keys()))
        .output()
        .expect("mf runs");

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(
            "published \"Field Kit\" -> https://steamcommunity.com/sharedfiles/filedetails/?id=7 (updated)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("under builder (76561198000000001)\n"),
        "{text}"
    );
    assert!(!text.contains("legal agreement"), "{text}");
    let sent = requests(&rig);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0], r#"{"request":"account"}"#);
    assert!(
        sent[1].starts_with(r#"{"request":"update","id":7"#),
        "{sent:?}"
    );
}

#[test]
fn publish_of_a_new_item_prints_new_item_and_the_legal_agreement_line() {
    let rig = rig(
        "new-item",
        r#"*'"request":"create"'*) echo '{"event":"created","id":42,"needs_legal_agreement":true}' ;;
*'"request":"update"'*) echo '{"event":"submitted","id":42,"needs_legal_agreement":false}' ;;"#,
    );
    let (_base, dir) = ready_mod(
        "publish-new-item",
        r#"{"title":"Field Kit","visibility":"private"}"#,
    );
    let db = Temp::new("publish-new-item-db");

    let out = mf_with(&rig, &db.join("modfoundry.sqlite"))
        .arg("publish")
        .arg(&dir)
        .args(publish_flags(keys()))
        .output()
        .expect("mf runs");

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("?id=42 (new item)"), "{text}");
    assert!(
        text.contains(
            "accept the Workshop legal agreement before the item shows: https://steamcommunity.com/sharedfiles/workshoplegalagreement\n"
        ),
        "{text}"
    );
    assert_eq!(saved_item(&dir)["id"], 42);
    let sent = requests(&rig);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert_eq!(sent[0], r#"{"request":"account"}"#);
    assert_eq!(sent[1], r#"{"request":"create"}"#);
    assert!(
        sent[2].starts_with(r#"{"request":"update","id":42"#),
        "{sent:?}"
    );
}

#[test]
fn publish_items_lists_the_account_and_each_item() {
    let rig = rig(
        "items",
        r#"*'"request":"my_items"'*) echo '{"event":"items","account":{"name":"builder","steam_id":76561198000000001},"items":[{"id":3000000001,"title":"Field Kit","description":"","visibility":"public","tags":[],"kv_tags":[],"updated":1},{"id":3000000002,"title":"Other","description":"","visibility":"friends_only","tags":[],"kv_tags":[],"updated":1}]}' ;;"#,
    );
    let db = Temp::new("publish-items-db");

    let out = mf_with(&rig, &db.join("modfoundry.sqlite"))
        .arg("publish-items")
        .output()
        .expect("mf runs");

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "account builder (76561198000000001)\n\
         3000000001  public  Field Kit\n\
         3000000002  friends_only  Other\n"
    );
}

#[test]
fn publish_items_reports_a_helper_failure() {
    let rig = rig(
        "items-failed",
        r#"*'"request":"my_items"'*) echo '{"event":"failed","message":"Steam is not running"}' ;;"#,
    );
    let db = Temp::new("publish-items-failed-db");

    let out = mf_with(&rig, &db.join("modfoundry.sqlite"))
        .arg("publish-items")
        .output()
        .expect("mf runs");

    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Steam is not running"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}
