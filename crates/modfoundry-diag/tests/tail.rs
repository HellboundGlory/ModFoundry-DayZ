//! Contract tests for `LogTail` incremental log reading.

use modfoundry_diag::{find_logs, LogTail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

/// Unique temp dir per call (label + pid + per-call counter) so parallel tests
/// in one binary never write the same path.
fn tmp(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mf-diag-tail-{label}-{}-{n}", std::process::id()))
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn append(path: &Path, contents: &[u8]) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(contents).unwrap();
}

fn set_modified(path: &Path, at: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

fn millis(at: i64) -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_millis(at as u64)
}

#[test]
fn tail_missing_dir_yields_no_records() {
    let dir = tmp("missing");
    assert!(find_logs(&dir).is_empty());
    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 0);
}

#[test]
fn tail_complete_lines_parse_once_and_are_not_reported_twice() {
    let dir = tmp("twice");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(
        &rpt,
        b"15:52:04.285  SCRIPT    (W): first line\r\n15:52:05.000  SCRIPT    (E): second line\r\n",
    );

    let mut tail = LogTail::new(&dir);
    let got = tail.poll();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|r| r.file == rpt));
    assert_eq!(got[0].record.message, "first line");
    assert_eq!(got[1].record.message, "second line");
    assert_eq!(tail.poll().len(), 0);
}

#[test]
fn tail_partial_line_is_held_until_its_newline_arrives() {
    let dir = tmp("partial");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): hel");

    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 0);

    append(&rpt, b"lo world\r\n");
    let got = tail.poll();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].record.message, "hello world");
}

#[test]
fn tail_new_files_and_appended_lines_come_back_in_file_then_line_order() {
    let dir = tmp("newfile");
    let rpt = dir.join("DayZServer_x64.RPT");
    let script = dir.join("script.log");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): rpt one\r\n");

    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 1);

    append(&rpt, b"15:52:05.000  SCRIPT    (W): rpt two\r\n");
    write(
        &script,
        b" SCRIPT    (W): script one\r\n SCRIPT    (E): script two\r\n",
    );

    let got = tail.poll();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].file, rpt);
    assert_eq!(got[0].record.message, "rpt two");
    assert_eq!(got[1].file, script);
    assert_eq!(got[1].record.message, "script one");
    assert_eq!(got[2].record.message, "script two");
}

#[test]
fn tail_shrunken_file_resets_its_offset() {
    let dir = tmp("truncate");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(
        &rpt,
        b"15:52:04.285  SCRIPT    (W): old line\r\n15:52:05.000  SCRIPT    (W): older\r\n",
    );

    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 2);

    write(&rpt, b"16:00:00.000  SCRIPT    (W): fresh line\r\n");
    let got = tail.poll();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].record.message, "fresh line");
}

#[test]
fn a_rewritten_log_is_read_from_its_first_byte_when_it_outgrows_the_old_offset() {
    let dir = tmp("rewrite");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): old line\r\n");

    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 1);

    write(
        &rpt,
        b"16:00:00.000  SCRIPT    (W): fresh line\r\n16:00:01.000  SCRIPT    (E): another\r\n",
    );
    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(messages, ["fresh line", "another"]);
}

#[test]
fn a_rewrite_drops_the_old_partial_line() {
    let dir = tmp("rewrite-partial");
    let log = dir.join("script.log");
    write(&log, b"SCRIPT (E): old one\nunfinished");
    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 1);

    write(
        &log,
        b"ENGINE (E): new one that is much longer than the old\n",
    );
    let records = tail.poll();

    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record.message,
        "new one that is much longer than the old"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unchanged_log_is_not_opened_again() {
    let dir = tmp("unchanged");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): old line\r\n");
    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), 1);

    let written_at = std::fs::metadata(&rpt).unwrap().modified().unwrap();
    write(&rpt, b"15:52:04.285  SCRIPT    (W): new line\r\n");
    set_modified(&rpt, written_at);

    let got = tail.poll();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(got.len(), 0);
}

#[test]
fn a_tail_started_after_a_short_log_reads_only_what_it_gains() {
    let dir = tmp("growth");
    let rpt = dir.join("old.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (E): held before\r\n");

    let mut tail = LogTail::after_existing(&dir);
    let mut gained = String::new();
    for n in 0..20 {
        gained.push_str(&format!("15:52:05.000  SCRIPT    (E): line {n}\r\n"));
    }
    append(&rpt, gained.as_bytes());

    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(messages.len(), 20);
    assert_eq!(messages[0], "line 0");
    assert_eq!(messages[19], "line 19");
}

#[test]
fn tail_invalid_utf8_bytes_do_not_error() {
    let dir = tmp("badutf8");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): caf\xe9 latte\r\n");

    let mut tail = LogTail::new(&dir);
    let got = tail.poll();
    assert_eq!(got.len(), 1);
    assert!(got[0].record.message.contains("caf"));
    assert!(got[0].record.message.contains("latte"));
    assert!(got[0].record.message.contains('\u{fffd}'));
}

#[test]
fn tail_multibyte_character_split_across_polls_decodes_whole() {
    let dir = tmp("split-utf8");
    let log = dir.join("DayZServer_x64.RPT");
    let line = "15:52:04.285  SCRIPT    (W): caf\u{e9} ready\r\n".as_bytes();
    let cut = line.iter().position(|&b| b == 0xC3).unwrap() + 1;
    write(&log, &line[..cut]);
    let mut tail = LogTail::new(&dir);
    assert!(tail.poll().is_empty());
    append(&log, &line[cut..]);
    let records = tail.poll();
    assert_eq!(records.len(), 1);
    assert!(
        records[0].record.message.contains("caf\u{e9}"),
        "{:?}",
        records[0].record.message
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_log_does_not_lose_the_records_of_the_others() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("unreadable");
    let rpt = dir.join("a.RPT");
    write(&rpt, b"15:52:04.285  SCRIPT    (W): kept line\r\n");
    let log = dir.join("z.log");
    write(&log, b"");
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut tail = LogTail::new(&dir);
    let got = tail.poll();

    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].file, rpt);
    assert_eq!(got[0].record.message, "kept line");
}

#[test]
fn a_rewrite_with_the_same_banner_is_read_from_the_start_when_it_outgrows_the_old_offset() {
    let dir = tmp("same-banner");
    let rpt = dir.join("DayZServer_x64.RPT");
    let mut banner = String::new();
    while banner.len() < 300 {
        banner.push_str("15:52:04.285  SCRIPT    (W): a banner line the server writes\r\n");
    }
    write(&rpt, banner.as_bytes());
    append(&rpt, b"15:52:05.000  SCRIPT    (W): old line\r\n");

    let mut tail = LogTail::new(&dir);
    assert_eq!(tail.poll().len(), banner.matches("\r\n").count() + 1);

    let mut rewritten = banner.clone();
    for n in 0..4 {
        rewritten.push_str(&format!(
            "16:00:0{n}.000  SCRIPT    (E): fresh line {n}\r\n"
        ));
    }
    write(&rpt, rewritten.as_bytes());
    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    let fresh: Vec<&String> = messages
        .iter()
        .filter(|m| m.starts_with("fresh line"))
        .collect();
    assert_eq!(fresh.len(), 4, "{messages:?}");
    for (n, message) in fresh.iter().enumerate() {
        assert_eq!(*message, &format!("fresh line {n}"));
    }
}

#[cfg(unix)]
#[test]
fn a_log_unreadable_when_a_tail_started_is_read_whole_once_it_can_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("unreadable-start");
    let rpt = dir.join("DayZServer_x64.RPT");
    write(
        &rpt,
        b"15:52:04.285  SCRIPT    (E): held before\r\n15:52:05.000  SCRIPT    (E): also held\r\n",
    );
    std::fs::set_permissions(&rpt, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&rpt).is_ok() {
        return; // root or CAP_DAC_OVERRIDE reads it anyway
    }

    let mut tail = LogTail::after_existing(&dir);
    assert!(tail.poll().is_empty());

    std::fs::set_permissions(&rpt, std::fs::Permissions::from_mode(0o644)).unwrap();
    write(
        &rpt,
        b"16:00:00.000  SCRIPT    (W): first fresh line\r\n16:00:01.000  SCRIPT    (W): second fresh line\r\n16:00:02.000  SCRIPT    (W): third fresh line\r\n",
    );
    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(
        messages,
        ["first fresh line", "second fresh line", "third fresh line"]
    );
}

#[test]
fn after_existing_skips_what_the_files_held_before() {
    let dir = tmp("after-existing");
    let old = dir.join("old.RPT");
    write(&old, b"15:52:04.285  SCRIPT    (E): held before\r\n");

    let mut tail = LogTail::after_existing(&dir);
    append(&old, b"15:52:05.000  SCRIPT    (E): written since\r\n");
    write(
        &dir.join("script_new.log"),
        b"15:52:06.000  SCRIPT    (W): new file\r\n",
    );

    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(messages, ["written since", "new file"]);
}

#[test]
fn since_skips_a_log_modified_before_the_cutoff_and_reads_one_modified_at_it() {
    let dir = tmp("since");
    let cutoff: i64 = 1_000_500;
    let old = dir.join("old.RPT");
    write(&old, b"15:52:04.285  SCRIPT    (E): written before\r\n");
    set_modified(&old, millis(cutoff - 1));
    let new = dir.join("script_new.log");
    write(&new, b"15:52:06.000  SCRIPT    (W): new file\r\n");
    set_modified(&new, millis(cutoff));

    let mut tail = LogTail::since(&dir, cutoff);
    let messages: Vec<String> = tail.poll().into_iter().map(|r| r.record.message).collect();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(messages, ["new file"]);
}
