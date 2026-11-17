//! The write API's contract: change-sets say what a mutation did, they are
//! invertible, and a refused set leaves the tree untouched.

use modfoundry_vfs::write::{ChangeOp, ChangeSet, FileChange, WriteTxn};
use modfoundry_vfs::{sha256_hex, VfsError, VirtualPath};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn tmp(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mf-vfsw-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn create_and_update_round_trip_through_undo() {
    let dir = tmp("round-trip");
    fs::write(dir.join("config.cpp"), b"old").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("config.cpp", b"new".to_vec()).unwrap();
    txn.write("Scripts/3_Game/MyMod.c", b"class MyMod {};".to_vec())
        .unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(fs::read(dir.join("config.cpp")).unwrap(), b"new");
    assert_eq!(
        fs::read(dir.join("Scripts/3_Game/MyMod.c")).unwrap(),
        b"class MyMod {};"
    );
    assert_eq!(
        set.files()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["mod:MyMod:config.cpp", "mod:MyMod:Scripts/3_Game/MyMod.c"]
    );
    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert_eq!(set.files()[0].before.as_deref(), Some(&b"old"[..]));
    assert_eq!(set.files()[1].op, ChangeOp::Create);
    assert!(set.files()[1].before.is_none());

    assert_eq!(set.undo().unwrap(), 2);
    assert_eq!(fs::read(dir.join("config.cpp")).unwrap(), b"old");
    assert!(!dir.join("Scripts/3_Game/MyMod.c").exists());
    assert!(
        !dir.join("Scripts").exists(),
        "directories the undo emptied are pruned: {:?}",
        names(&dir)
    );
}

#[test]
fn undo_of_a_path_written_twice_restores_the_original() {
    let dir = tmp("written-twice");
    fs::write(dir.join("A.c"), b"orig").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("A.c", b"one".to_vec()).unwrap();
    txn.write("A.c", b"two".to_vec()).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(fs::read(dir.join("A.c")).unwrap(), b"two");

    set.undo().unwrap();
    assert_eq!(fs::read(dir.join("A.c")).unwrap(), b"orig");
}

#[cfg(unix)]
#[test]
fn undo_reports_a_delete_that_fails() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("undelete");
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("Sub/New.c", b"class New {};".to_vec()).unwrap();
    let set = txn.finish().unwrap();

    let sub = dir.join("Sub");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::File::create(sub.join("probe")).is_ok() {
        // Root ignores the mode bits, so the delete cannot fail here.
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(sub.join("probe")).unwrap();
        eprintln!("skipped: the delete cannot fail when running as root");
        return;
    }

    let result = set.undo();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(result, Err(VfsError::Io(_))), "{result:?}");
    assert!(sub.join("New.c").is_file());
}

#[test]
fn dry_run_records_what_would_change_and_writes_nothing() {
    let dir = tmp("dry-run");
    fs::write(dir.join("config.cpp"), b"old").unwrap();

    let mut txn = WriteTxn::dry_run("mod:MyMod", &dir);
    txn.write("config.cpp", b"new".to_vec()).unwrap();
    txn.write("Scripts/3_Game/MyMod.c", b"class MyMod {};".to_vec())
        .unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(set.len(), 2);
    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert_eq!(set.files()[1].op, ChangeOp::Create);
    assert_eq!(fs::read(dir.join("config.cpp")).unwrap(), b"old");
    assert_eq!(names(&dir), vec!["config.cpp"]);
}

#[test]
fn a_case_conflict_refuses_the_whole_set_before_writing_anything() {
    let dir = tmp("case");
    fs::write(dir.join("Config.cpp"), b"original").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("config.cpp", b"clashing".to_vec()).unwrap();
    txn.write("Scripts/3_Game/Ok.c", b"fine".to_vec()).unwrap();
    let err = txn.finish().unwrap_err();

    assert!(matches!(err, VfsError::CaseConflict(_)), "{err}");
    assert_eq!(fs::read(dir.join("Config.cpp")).unwrap(), b"original");
    assert_eq!(
        names(&dir),
        vec!["Config.cpp"],
        "a refused set writes none of its files"
    );
}

#[test]
fn a_parent_that_is_a_file_is_refused_and_leaves_no_staging_file() {
    let dir = tmp("parent-file");
    fs::write(dir.join("Scripts"), b"not a directory").unwrap();
    fs::write(dir.join("keep.txt"), b"keep").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("Scripts/3_Game/X.c", b"x".to_vec()).unwrap();
    assert!(txn.finish().is_err());

    assert_eq!(fs::read(dir.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(names(&dir), vec!["Scripts", "keep.txt"]);
}

#[test]
fn a_change_set_survives_json_with_binary_content() {
    let dir = tmp("json");
    let bytes = vec![0u8, 0x9f, 0x92, b'\n', 0xff];

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("Textures/logo.paa", bytes.clone()).unwrap();
    let set = txn.finish().unwrap();

    let json = serde_json::to_string(&set).unwrap();
    let back: ChangeSet = serde_json::from_str(&json).unwrap();
    assert_eq!(back, set);
    assert_eq!(back.files()[0].before, None);
    assert_eq!(fs::read(dir.join("Textures/logo.paa")).unwrap(), bytes);
}

#[test]
fn a_change_set_recorded_elsewhere_cannot_write_outside_its_root() {
    let dir = tmp("escape");
    let outside = dir.join("outside.c");
    fs::write(&outside, b"untouched").unwrap();

    let entry = |path: &str| FileChange {
        path: VirtualPath::parse(path).unwrap(),
        op: ChangeOp::Create,
        before: None,
        after_sha256: None,
        skip_on_undo: false,
    };
    let set = |changes| ChangeSet {
        root: "mod:MyMod".into(),
        host_root: dir.clone(),
        changes,
    };

    // An entry belonging to another root.
    let foreign = set(vec![entry("game:DZ/AI/config.bin")]);
    assert!(matches!(foreign.undo(), Err(VfsError::OutsideRoot(_))));

    // A relative part that is not a plain relative path.
    let absolute = set(vec![entry("mod:MyMod:C:/Windows/system32/x.c")]);
    assert!(matches!(absolute.undo(), Err(VfsError::InvalidPath(_))));

    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    assert_eq!(names(&dir), vec!["outside.c"]);
}

#[test]
fn undo_refuses_a_change_set_whose_replaced_content_was_not_kept() {
    let dir = tmp("unrecorded");
    fs::write(dir.join("config.cpp"), b"old").unwrap();

    let mut txn = WriteTxn::unrecorded("mod:MyMod", &dir);
    txn.write("config.cpp", b"new".to_vec()).unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert!(set.files()[0].before.is_none());
    assert!(matches!(set.undo(), Err(VfsError::Incomplete(_))));
    assert_eq!(fs::read(dir.join("config.cpp")).unwrap(), b"new");
}

#[test]
fn an_empty_change_set_reports_nothing_to_do() {
    let dir = tmp("empty");
    let txn = WriteTxn::new("mod:MyMod", &dir);
    let set = txn.finish().unwrap();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    assert_eq!(set.undo().unwrap(), 0);
}

#[test]
fn a_write_that_must_not_be_retained_keeps_nothing_but_still_records_the_change() {
    let dir = tmp("unretained");
    fs::write(dir.join("MF.biprivatekey"), b"-----BEGIN PRIVATE KEY-----").unwrap();

    let mut txn = WriteTxn::new("mod:keys", &dir);
    txn.write_unretained("MF.biprivatekey", b"new key material".to_vec())
        .unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert!(
        set.files()[0].before.is_none(),
        "content that must not be persisted cannot be retained for the undo"
    );
    assert_eq!(
        fs::read(dir.join("MF.biprivatekey")).unwrap(),
        b"new key material"
    );
    assert!(
        !serde_json::to_string(&set).unwrap().contains("PRIVATE KEY"),
        "the replaced content leaked into the serialized change-set"
    );
    // Nothing to restore, so an undo refuses instead of guessing.
    assert!(matches!(set.undo(), Err(VfsError::Incomplete(_))));
}

#[cfg(unix)]
#[test]
fn an_unretained_write_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("owner-only");

    let mut txn = WriteTxn::new("mod:keys", &dir);
    txn.write_unretained("MF.biprivatekey", b"new key material".to_vec())
        .unwrap();
    txn.finish().unwrap();

    let mode = fs::metadata(dir.join("MF.biprivatekey"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn undo_refuses_when_a_file_changed_after_the_change_set_was_recorded() {
    let dir = tmp("stale");
    fs::write(dir.join("config.cpp"), b"one").unwrap();

    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write("config.cpp", b"two".to_vec()).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(fs::read(dir.join("config.cpp")).unwrap(), b"two");

    // The user keeps editing after the change-set was recorded (the stack lives
    // in the database, so this is the normal case, not the corner case).
    fs::write(dir.join("config.cpp"), b"three").unwrap();
    assert!(matches!(set.undo(), Err(VfsError::Stale(_))));
    assert_eq!(
        fs::read(dir.join("config.cpp")).unwrap(),
        b"three",
        "an undo must never clobber content it did not write"
    );
}

#[test]
fn undo_refuses_to_delete_a_created_file_that_was_edited_since() {
    let dir = tmp("stale-create");
    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write("Scripts/3_Game/New.c", b"class New {};".to_vec())
        .unwrap();
    let set = txn.finish().unwrap();

    fs::write(dir.join("Scripts/3_Game/New.c"), b"class New { int x; };").unwrap();
    assert!(matches!(set.undo(), Err(VfsError::Stale(_))));
    assert!(dir.join("Scripts/3_Game/New.c").is_file());
}

#[test]
fn undo_of_a_created_file_that_is_already_gone_skips_it_and_undoes_the_rest() {
    let dir = tmp("create-gone");
    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write("Scripts/A.c", b"class A {};".to_vec()).unwrap();
    txn.write("Scripts/B.c", b"class B {};".to_vec()).unwrap();
    let set = txn.finish().unwrap();

    fs::remove_file(dir.join("Scripts/B.c")).unwrap();
    assert_eq!(set.undo().unwrap(), 1);
    assert!(!dir.join("Scripts/A.c").exists());
    assert!(!dir.join("Scripts").exists(), "{:?}", names(&dir));

    let dir = tmp("create-gone-folder");
    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write("Sub/New.c", b"class New {};".to_vec()).unwrap();
    let set = txn.finish().unwrap();

    fs::remove_dir_all(dir.join("Sub")).unwrap();
    assert_eq!(set.undo().unwrap(), 0);
    assert!(dir.is_dir(), "the root itself is never pruned");
}

#[test]
fn a_refused_batch_stays_queued_so_finish_cannot_succeed_without_it() {
    let setup = |dir: &Path| {
        let mut txn = WriteTxn::new("mod:MyMod", dir);
        txn.write("first.txt", b"first".to_vec()).unwrap();
        txn.apply_pending().unwrap();
        assert!(dir.join("first.txt").is_file());

        fs::write(dir.join("Scripts"), b"not a directory").unwrap();
        txn.write("Scripts/X.c", b"x".to_vec()).unwrap();
        assert!(txn.apply_pending().is_err());
        assert!(!dir.join("Scripts/X.c").exists());
        txn
    };

    let dir = tmp("refused-queued");
    assert!(setup(&dir).finish().is_err());
    assert!(!dir.join("Scripts/X.c").exists());

    let dir = tmp("refused-queued-retry");
    let txn = setup(&dir);
    fs::remove_file(dir.join("Scripts")).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(
        set.files()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["mod:MyMod:first.txt", "mod:MyMod:Scripts/X.c"]
    );
    assert_eq!(set.undo().unwrap(), 2);
    assert!(!dir.join("first.txt").exists());
    assert!(!dir.join("Scripts").exists());
}

#[test]
fn a_stale_checked_write_stays_queued_after_apply_pending_refuses_it() {
    let dir = tmp("stale-queued");
    fs::write(dir.join("A.c"), b"old").unwrap();
    fs::write(dir.join("A.c"), b"newer").unwrap();

    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write_checked("A.c", b"mine".to_vec(), &sha256_hex(b"old"))
        .unwrap();
    assert!(matches!(txn.apply_pending(), Err(VfsError::Stale(_))));
    assert!(matches!(txn.finish(), Err(VfsError::Stale(_))));
    assert_eq!(fs::read(dir.join("A.c")).unwrap(), b"newer");
}

#[test]
fn a_checked_write_refuses_when_the_file_moved_on_since_it_was_read() {
    let dir = tmp("checked-stale");
    fs::write(dir.join("A.c"), b"old").unwrap();
    let digest = sha256_hex(b"old");

    // What the writer read, then somebody else's edit.
    fs::write(dir.join("A.c"), b"newer").unwrap();

    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write_checked("A.c", b"mine".to_vec(), &digest).unwrap();
    assert!(matches!(txn.finish(), Err(VfsError::Stale(_))));
    assert_eq!(
        fs::read(dir.join("A.c")).unwrap(),
        b"newer",
        "a stale base revision must never clobber the edit it did not see"
    );
}

#[test]
fn a_checked_write_applies_and_records_when_the_base_revision_matches() {
    let dir = tmp("checked-ok");
    fs::write(dir.join("A.c"), b"old").unwrap();

    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write_checked("A.c", b"mine".to_vec(), &sha256_hex(b"old"))
        .unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(fs::read(dir.join("A.c")).unwrap(), b"mine");
    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert_eq!(set.undo().unwrap(), 1);
    assert_eq!(fs::read(dir.join("A.c")).unwrap(), b"old");
}

#[test]
fn a_dry_run_checks_the_base_revision_too() {
    let dir = tmp("checked-preview");
    fs::write(dir.join("A.c"), b"current").unwrap();

    let mut txn = WriteTxn::dry_run("mod:X", &dir);
    txn.write_checked("A.c", b"mine".to_vec(), &sha256_hex(b"stale"))
        .unwrap();
    assert!(
        matches!(txn.finish(), Err(VfsError::Stale(_))),
        "a preview of a stale edit must report the staleness, not a clean plan"
    );
}

#[test]
fn a_checked_write_of_a_file_that_is_not_there_is_stale() {
    let dir = tmp("checked-missing");

    let mut txn = WriteTxn::new("mod:X", &dir);
    txn.write_checked("A.c", b"mine".to_vec(), &sha256_hex(b"old"))
        .unwrap();
    assert!(matches!(txn.finish(), Err(VfsError::Stale(_))));
    assert!(!dir.join("A.c").exists());
}

#[test]
fn an_existing_target_is_detected_without_reading_it() {
    let dir = tmp("metadata");
    fs::write(dir.join("A.c"), b"old").unwrap();
    fs::create_dir_all(dir.join("Scripts")).unwrap();

    let mut txn = WriteTxn::dry_run("mod:X", &dir);
    txn.write("A.c", b"new".to_vec()).unwrap();
    txn.write("Fresh.c", b"new".to_vec()).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(set.files()[0].op, ChangeOp::Update);
    assert_eq!(set.files()[1].op, ChangeOp::Create);

    let mut txn = WriteTxn::dry_run("mod:X", &dir);
    txn.write("Scripts", b"new".to_vec()).unwrap();
    assert!(
        matches!(txn.finish(), Err(VfsError::InvalidPath(_))),
        "a dry run refuses a directory as an apply would"
    );

    let mut txn = WriteTxn::dry_run("mod:X", &dir);
    txn.write("a.c", b"new".to_vec()).unwrap();
    assert!(
        matches!(txn.finish(), Err(VfsError::CaseConflict(_))),
        "a dry run refuses a case conflict as an apply would"
    );
}

#[test]
fn remove_deletes_the_file_and_undo_brings_it_back() {
    let dir = tmp("remove");
    fs::create_dir_all(dir.join("Sub")).unwrap();
    fs::write(dir.join("Sub/old.bisign"), b"stale signature").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("Sub/old.bisign").unwrap();
    let set = txn.finish().unwrap();

    assert!(!dir.join("Sub/old.bisign").exists());
    assert_eq!(set.files().len(), 1);
    assert_eq!(set.files()[0].op, ChangeOp::Delete);
    assert_eq!(
        set.files()[0].before.as_deref(),
        Some(&b"stale signature"[..])
    );

    assert_eq!(set.undo().unwrap(), 1);
    assert_eq!(
        fs::read(dir.join("Sub/old.bisign")).unwrap(),
        b"stale signature"
    );
}

#[test]
fn remove_of_a_missing_file_records_nothing() {
    let dir = tmp("remove-missing");

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("Gone.bisign").unwrap();
    let set = txn.finish().unwrap();

    assert!(set.is_empty(), "{:?}", set.files());
    assert_eq!(set.undo().unwrap(), 0);
}

#[test]
fn a_dry_run_remove_lists_the_file_and_keeps_it() {
    let dir = tmp("remove-dry-run");
    fs::write(dir.join("old.bisign"), b"stale signature").unwrap();

    let mut txn = WriteTxn::dry_run("mod:MyMod", &dir);
    txn.remove("old.bisign").unwrap();
    let set = txn.finish().unwrap();

    assert_eq!(set.len(), 1);
    assert_eq!(set.files()[0].op, ChangeOp::Delete);
    assert_eq!(
        fs::read(dir.join("old.bisign")).unwrap(),
        b"stale signature"
    );
}

#[test]
fn undo_of_a_remove_refuses_when_the_file_was_written_again() {
    let dir = tmp("remove-stale");
    fs::write(dir.join("old.bisign"), b"stale signature").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("old.bisign").unwrap();
    let set = txn.finish().unwrap();

    fs::write(dir.join("old.bisign"), b"fresh signature").unwrap();
    assert!(matches!(set.undo(), Err(VfsError::Stale(_))));
    assert_eq!(
        fs::read(dir.join("old.bisign")).unwrap(),
        b"fresh signature",
        "an undo must never replace a file somebody wrote after the removal"
    );
}

#[test]
fn remove_of_a_path_written_earlier_in_the_set_takes_it_out_again() {
    let dir = tmp("remove-new");
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("Sub/new.txt", b"x".to_vec()).unwrap();
    txn.remove("Sub/new.txt").unwrap();
    let set = txn.finish().unwrap();

    assert!(!dir.join("Sub/new.txt").exists());
    assert!(!dir.join("Sub").exists(), "{:?}", names(&dir));
    assert_eq!(set.len(), 2);
    assert_eq!(set.files()[0].op, ChangeOp::Create);
    assert_eq!(set.files()[1].op, ChangeOp::Delete);
    assert_eq!(set.files()[1].before.as_deref(), Some(&b"x"[..]));
    set.undo().unwrap();
    assert!(names(&dir).is_empty(), "{:?}", names(&dir));

    let dir = tmp("remove-replaced");
    fs::write(dir.join("old.txt"), b"old").unwrap();
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("old.txt", b"new".to_vec()).unwrap();
    txn.remove("old.txt").unwrap();
    let set = txn.finish().unwrap();

    assert!(!dir.join("old.txt").exists());
    set.undo().unwrap();
    assert_eq!(fs::read(dir.join("old.txt")).unwrap(), b"old");
}

#[test]
fn removing_a_path_twice_records_one_delete() {
    let dir = tmp("remove-twice");
    fs::write(dir.join("gone.bisign"), b"stale").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("gone.bisign").unwrap();
    txn.remove("gone.bisign").unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(set.len(), 1, "{:?}", set.files());
    assert!(!dir.join("gone.bisign").exists());

    let dir = tmp("remove-twice-dry");
    fs::write(dir.join("gone.bisign"), b"stale").unwrap();
    let mut txn = WriteTxn::dry_run("mod:MyMod", &dir);
    txn.remove("gone.bisign").unwrap();
    txn.remove("gone.bisign").unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(set.len(), 1, "{:?}", set.files());
    assert!(dir.join("gone.bisign").is_file());
}

#[test]
fn remove_matches_names_exactly() {
    let dir = tmp("remove-exact");
    fs::write(dir.join("Foo.bisign"), b"old").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("foo.bisign").unwrap();
    let set = txn.finish().unwrap();
    assert!(set.is_empty(), "{:?}", set.files());
    assert!(dir.join("Foo.bisign").is_file());

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("Bar.bisign", b"x".to_vec()).unwrap();
    txn.remove("bar.bisign").unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(set.len(), 1, "{:?}", set.files());
    assert_eq!(set.files()[0].op, ChangeOp::Create);
    assert!(dir.join("Bar.bisign").is_file());
}

#[test]
fn a_case_variant_may_replace_a_file_removed_earlier_in_the_set() {
    let dir = tmp("case-variant");
    fs::write(dir.join("Old.bisign"), b"old").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("Old.bisign").unwrap();
    txn.write("old.bisign", b"new".to_vec()).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(names(&dir), vec!["old.bisign"]);
    assert_eq!(fs::read(dir.join("old.bisign")).unwrap(), b"new");
    assert_eq!(set.undo().unwrap(), 2);
    assert_eq!(names(&dir), vec!["Old.bisign"]);
    assert_eq!(fs::read(dir.join("Old.bisign")).unwrap(), b"old");

    let dir = tmp("case-variant-dry");
    fs::write(dir.join("Old.bisign"), b"old").unwrap();
    let mut txn = WriteTxn::dry_run("mod:MyMod", &dir);
    txn.remove("Old.bisign").unwrap();
    txn.write("old.bisign", b"new".to_vec()).unwrap();
    txn.finish().unwrap();
    assert_eq!(names(&dir), vec!["Old.bisign"]);

    let dir = tmp("case-variant-late");
    fs::write(dir.join("Old.bisign"), b"old").unwrap();
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.write("old.bisign", b"new".to_vec()).unwrap();
    txn.remove("Old.bisign").unwrap();
    let err = txn.finish().unwrap_err();
    assert!(matches!(err, VfsError::CaseConflict(_)), "{err}");
    assert_eq!(names(&dir), vec!["Old.bisign"]);

    let dir = tmp("case-variant-other");
    fs::write(dir.join("Old.bisign"), b"old").unwrap();
    fs::write(dir.join("Other.bisign"), b"other").unwrap();
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("Other.bisign").unwrap();
    txn.write("old.bisign", b"new".to_vec()).unwrap();
    let err = txn.finish().unwrap_err();
    assert!(matches!(err, VfsError::CaseConflict(_)), "{err}");
}

#[test]
fn a_write_after_a_queued_remove_of_the_same_path_undoes_to_the_original() {
    let dir = tmp("remove-then-write");
    fs::write(dir.join("a.txt"), b"orig").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("a.txt").unwrap();
    txn.write("a.txt", b"again".to_vec()).unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"again");

    set.undo().unwrap();
    assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"orig");
}

#[cfg(unix)]
#[test]
fn undo_of_a_set_that_removed_a_symlink_restores_the_rest() {
    let dir = tmp("link-undo");
    let outside = tmp("link-undo-target");
    fs::write(dir.join("a.txt"), b"a").unwrap();
    fs::write(outside.join("secret.biprivatekey"), b"private").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.biprivatekey"), dir.join("link.bikey"))
        .unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("a.txt").unwrap();
    txn.remove("link.bikey").unwrap();
    let set = txn.finish().unwrap();

    let change = |path: &str| {
        set.files()
            .iter()
            .find(|f| f.path.as_str() == path)
            .unwrap()
            .clone()
    };
    let link = change("mod:MyMod:link.bikey");
    assert_eq!(link.op, ChangeOp::Delete);
    assert!(link.before.is_none());
    assert!(link.skip_on_undo);
    assert!(!change("mod:MyMod:a.txt").skip_on_undo);

    assert_eq!(set.undo().unwrap(), 1);
    assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"a");
    assert!(fs::symlink_metadata(dir.join("link.bikey")).is_err());
    assert_eq!(
        fs::read(outside.join("secret.biprivatekey")).unwrap(),
        b"private"
    );
}

#[test]
fn undo_still_refuses_a_removal_that_kept_nothing_by_accident() {
    let dir = tmp("remove-unkept");
    fs::write(dir.join("plain.txt"), b"plain").unwrap();

    let mut txn = WriteTxn::unrecorded("mod:MyMod", &dir);
    txn.remove("plain.txt").unwrap();
    let set = txn.finish().unwrap();

    assert!(!set.files()[0].skip_on_undo);
    assert!(set.files()[0].before.is_none());
    assert!(matches!(set.undo(), Err(VfsError::Incomplete(_))));
    assert!(!dir.join("plain.txt").exists());
}

#[test]
fn a_removed_private_key_is_never_kept_and_its_undo_refuses() {
    let dir = tmp("remove-keys");
    fs::create_dir_all(dir.join("keys")).unwrap();
    fs::write(dir.join("keys/MF.biprivatekey"), b"SECRET-KEY-BYTES").unwrap();
    fs::write(dir.join("keys/UPPER.BIPRIVATEKEY"), b"SECRET-KEY-BYTES").unwrap();
    fs::write(dir.join("keys/MF.bikey"), b"public").unwrap();
    fs::write(dir.join("note.txt"), b"note").unwrap();

    let mut txn = WriteTxn::new("mod:keys", &dir);
    for rel in [
        "keys/MF.biprivatekey",
        "keys/UPPER.BIPRIVATEKEY",
        "keys/MF.bikey",
        "note.txt",
    ] {
        txn.remove(rel).unwrap();
    }
    let set = txn.finish().unwrap();

    let change = |path: &str| {
        set.files()
            .iter()
            .find(|f| f.path.as_str() == path)
            .unwrap()
            .clone()
    };
    for path in [
        "mod:keys:keys/MF.biprivatekey",
        "mod:keys:keys/UPPER.BIPRIVATEKEY",
    ] {
        assert!(change(path).before.is_none(), "{path} kept its bytes");
        assert!(!change(path).skip_on_undo);
    }
    assert_eq!(
        change("mod:keys:keys/MF.bikey").before.as_deref(),
        Some(&b"public"[..])
    );
    assert!(
        !serde_json::to_string(&set)
            .unwrap()
            .contains("SECRET-KEY-BYTES"),
        "a private key leaked into the change-set"
    );
    assert!(matches!(set.undo(), Err(VfsError::Incomplete(_))));
    assert!(!dir.join("keys/MF.biprivatekey").exists());
    assert!(!dir.join("keys/MF.bikey").exists());

    let dir = tmp("remove-queued-key");
    let mut txn = WriteTxn::new("mod:keys", &dir);
    txn.write_unretained("new.biprivatekey", b"SECRET-KEY-BYTES")
        .unwrap();
    txn.remove("new.biprivatekey").unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(set.len(), 2);
    assert!(set.files()[1].before.is_none());
    assert!(!serde_json::to_string(&set)
        .unwrap()
        .contains("SECRET-KEY-BYTES"));
    assert!(!dir.join("new.biprivatekey").exists());
    assert!(matches!(set.undo(), Err(VfsError::Incomplete(_))));

    let dir = tmp("remove-ordinary-queued");
    let mut txn = WriteTxn::new("mod:keys", &dir);
    txn.write("loose.txt", b"SECRET-KEY-BYTES").unwrap();
    txn.remove("loose.txt").unwrap();
    let set = txn.finish().unwrap();
    assert_eq!(
        set.files()[1].before.as_deref(),
        Some(&b"SECRET-KEY-BYTES"[..])
    );
}

#[test]
fn skip_on_undo_round_trips_through_json_and_defaults_to_off() {
    let mut set = ChangeSet::new("mod:MyMod", PathBuf::from("/tmp/mod"));
    set.changes.push(FileChange {
        path: VirtualPath::parse("mod:MyMod:gone.txt").unwrap(),
        op: ChangeOp::Delete,
        before: Some(b"gone".to_vec()),
        after_sha256: None,
        skip_on_undo: false,
    });
    set.changes.push(FileChange {
        path: VirtualPath::parse("mod:MyMod:link.bikey").unwrap(),
        op: ChangeOp::Delete,
        before: None,
        after_sha256: None,
        skip_on_undo: true,
    });

    let json = serde_json::to_string(&set).unwrap();
    let plain = json.split("link.bikey").next().unwrap();
    assert!(!plain.contains("skip_on_undo"), "{json}");
    assert!(json.contains("\"skip_on_undo\":true"), "{json}");
    let back: ChangeSet = serde_json::from_str(&json).unwrap();
    assert_eq!(back, set);

    let old = r#"{"root":"mod:MyMod","host_root":"/tmp/mod","changes":[{"path":"mod:MyMod:old.txt","op":"delete","before":null,"after_sha256":null}]}"#;
    let read: ChangeSet = serde_json::from_str(old).unwrap();
    assert!(!read.files()[0].skip_on_undo);
}

#[test]
fn a_set_of_many_files_in_one_folder_applies_and_undoes() {
    let dir = tmp("many-files");
    fs::create_dir_all(dir.join("Data")).unwrap();
    fs::write(dir.join("Data/Zed.txt"), b"zed").unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    for i in 0..500 {
        txn.write(format!("Data/f{i:03}.txt"), b"x".to_vec())
            .unwrap();
    }
    for i in 0..50 {
        txn.write(format!("Data/Sub/g{i:02}.txt"), b"x".to_vec())
            .unwrap();
    }
    txn.write("Data/zed.txt", b"x".to_vec()).unwrap();
    let err = txn.finish().unwrap_err();
    assert!(matches!(err, VfsError::CaseConflict(_)), "{err}");
    assert_eq!(names(&dir.join("Data")), vec!["Zed.txt"]);

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    for i in 0..500 {
        txn.write(format!("Data/f{i:03}.txt"), b"x".to_vec())
            .unwrap();
    }
    for i in 0..50 {
        txn.write(format!("Data/Sub/g{i:02}.txt"), b"x".to_vec())
            .unwrap();
    }
    let set = txn.finish().unwrap();
    assert!(dir.join("Data/f499.txt").is_file());
    assert!(dir.join("Data/Sub/g49.txt").is_file());

    assert_eq!(set.undo().unwrap(), 550);
    assert_eq!(names(&dir.join("Data")), vec!["Zed.txt"]);
}

#[test]
#[ignore = "timing run, set MF_VFS_TIMING_FILES"]
fn timing_set_of_many_files_in_one_folder() {
    let count: usize = std::env::var("MF_VFS_TIMING_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200_000);
    let dir = tmp("timing");
    let paths: Vec<String> = (0..count).map(|i| format!("Data/f{i:06}.txt")).collect();

    let mut txn = WriteTxn::dry_run("mod:Timing", &dir);
    for path in &paths {
        txn.write(path, b"x".to_vec()).unwrap();
    }
    let started = Instant::now();
    txn.finish().unwrap();
    eprintln!("dry run of {count} files: {:?}", started.elapsed());

    let mut txn = WriteTxn::new("mod:Timing", &dir);
    for path in &paths {
        txn.write(path, b"x".to_vec()).unwrap();
    }
    let started = Instant::now();
    let set = txn.finish().unwrap();
    eprintln!("write of {count} files: {:?}", started.elapsed());
    assert!(dir.join(&paths[count - 1]).is_file());

    let started = Instant::now();
    assert_eq!(set.undo().unwrap(), count);
    eprintln!("undo of {count} files: {:?}", started.elapsed());
    assert!(!dir.join("Data").exists());
}

#[cfg(unix)]
#[test]
fn removing_a_symlink_never_keeps_the_bytes_it_points_at() {
    let dir = tmp("remove-link");
    let outside = tmp("remove-link-target");
    fs::write(outside.join("secret.biprivatekey"), b"private").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.biprivatekey"), dir.join("link.bikey"))
        .unwrap();

    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("link.bikey").unwrap();
    let set = txn.finish().unwrap();

    assert!(set.files()[0].before.is_none());
    assert!(fs::symlink_metadata(dir.join("link.bikey")).is_err());
    assert_eq!(
        fs::read(outside.join("secret.biprivatekey")).unwrap(),
        b"private"
    );
}

#[test]
fn a_private_key_never_enters_the_change_set_whatever_writes_or_removes_it() {
    type Calls = fn(&mut WriteTxn);
    let cases: [(&str, Calls); 4] = [
        ("dotfile key removed", |txn| {
            txn.remove(".biprivatekey").unwrap();
        }),
        ("key overwritten with a retaining write", |txn| {
            txn.write("k.biprivatekey", b"NEW").unwrap();
        }),
        ("key written then removed", |txn| {
            txn.write("fresh.biprivatekey", b"SECRET-KEY-BYTES")
                .unwrap();
            txn.remove("fresh.biprivatekey").unwrap();
        }),
        ("unretained file written then removed", |txn| {
            txn.write_unretained("secret.txt", b"SECRET-KEY-BYTES")
                .unwrap();
            txn.remove("secret.txt").unwrap();
        }),
    ];
    for (label, calls) in cases {
        let dir = tmp("key-never-kept");
        fs::write(dir.join(".biprivatekey"), b"SECRET-KEY-BYTES").unwrap();
        fs::write(dir.join("k.biprivatekey"), b"SECRET-KEY-BYTES").unwrap();
        let mut txn = WriteTxn::new("mod:keys", &dir);
        calls(&mut txn);
        let set = txn.finish().unwrap();
        assert!(
            set.files().iter().all(|f| f.before.is_none()),
            "{label}: bytes were kept"
        );
        assert!(
            !serde_json::to_string(&set)
                .unwrap()
                .contains("SECRET-KEY-BYTES"),
            "{label}: the key reached the JSON"
        );
    }
}

#[test]
fn a_write_after_a_removal_puts_back_the_case_conflict_that_removal_cleared() {
    let dir = tmp("readd-case");
    fs::write(dir.join("Old.bisign"), b"old").unwrap();
    let mut txn = WriteTxn::new("mod:MyMod", &dir);
    txn.remove("Old.bisign").unwrap();
    txn.write("Old.bisign", b"again").unwrap();
    txn.write("old.bisign", b"new").unwrap();
    assert!(matches!(txn.finish(), Err(VfsError::CaseConflict(_))));
    assert_eq!(names(&dir), ["Old.bisign"]);
}

#[test]
fn undo_checks_an_older_change_only_through_the_newest_one_at_its_path() {
    // The newest change at a path owns its staleness check, even when undo
    // skips it, so an older change there restores without a second check.
    let dir = tmp("skip-newest");
    fs::write(dir.join("a.txt"), b"current").unwrap();
    let mut set = ChangeSet::new("mod:MyMod", dir.clone());
    let path = VirtualPath::parse("mod:MyMod:a.txt").unwrap();
    set.changes.push(FileChange {
        path: path.clone(),
        op: ChangeOp::Update,
        before: Some(b"orig".to_vec()),
        after_sha256: Some(sha256_hex(b"older")),
        skip_on_undo: false,
    });
    set.changes.push(FileChange {
        path,
        op: ChangeOp::Delete,
        before: None,
        after_sha256: None,
        skip_on_undo: true,
    });
    assert_eq!(set.undo().unwrap(), 1);
    assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"orig");
}
