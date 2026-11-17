//! Folder-layer scans: which spelling survives a case clash, and what a scan
//! reports about entries it could not read.

use modfoundry_vfs::layer::{CaseClash, DirectoryBackend};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mf-vfs-scan-{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"x").unwrap();
}

#[test]
fn names_that_differ_only_in_case_keep_the_first_by_sorted_path() {
    let dir = tmp("clash");
    write(&dir, "config.cpp");
    write(&dir, "Config.cpp");
    write(&dir, "sub/A.txt");
    write(&dir, "sub/a.txt");

    let (index, notes) = DirectoryBackend::scan_with_notes(&dir).unwrap();

    assert_eq!(index.resolve("config.cpp"), Some("Config.cpp"));
    assert_eq!(index.resolve("sub/a.txt"), Some("sub/A.txt"));
    assert_eq!(
        notes.clashes,
        vec![
            CaseClash {
                kept: "Config.cpp".into(),
                dropped: "config.cpp".into(),
            },
            CaseClash {
                kept: "sub/A.txt".into(),
                dropped: "sub/a.txt".into(),
            },
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_folders_that_differ_only_in_case_clash_by_their_files() {
    let dir = tmp("folder-clash");
    write(&dir, "data/x.paa");
    write(&dir, "Data/x.paa");

    let (index, notes) = DirectoryBackend::scan_with_notes(&dir).unwrap();

    assert_eq!(index.len(), 1);
    assert_eq!(index.resolve("data/x.paa"), Some("Data/x.paa"));
    assert_eq!(
        notes.clashes,
        vec![CaseClash {
            kept: "Data/x.paa".into(),
            dropped: "data/x.paa".into(),
        }]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
struct RestoreMode {
    path: PathBuf,
    mode: u32,
    root: PathBuf,
}

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_subfolder_is_skipped_and_the_rest_is_indexed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp("locked");
    write(&dir, "ok/a.c");
    write(&dir, "locked/b.c");
    let locked = dir.join("locked");
    let _restore = RestoreMode {
        path: locked.clone(),
        mode: 0o755,
        root: dir.clone(),
    };
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        eprintln!("skipped: the folder is still readable (running as root)");
        return;
    }

    let (index, notes) = DirectoryBackend::scan_with_notes(&dir).unwrap();

    assert_eq!(index.resolve("ok/a.c"), Some("ok/a.c"));
    assert_eq!(index.len(), 1);
    assert_eq!(notes.unreadable.len(), 1, "got {notes:?}");
    assert!(notes.unreadable[0].path.ends_with("locked"));
    assert!(!notes.unreadable[0].error.is_empty());
}

#[test]
fn the_root_itself_unreadable_is_an_error() {
    let dir = tmp("gone");
    let missing = dir.join("absent");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(DirectoryBackend::scan_with_notes(&missing).is_err());
}

#[test]
fn three_spellings_name_the_one_kept_in_every_clash() {
    for order in [["Ab", "aB", "ab"], ["ab", "aB", "Ab"]] {
        let dir = tmp("three");
        for name in order {
            write(&dir, name);
        }

        let (index, notes) = DirectoryBackend::scan_with_notes(&dir).unwrap();

        assert_eq!(index.resolve("ab"), Some("Ab"));
        assert_eq!(
            notes.clashes,
            vec![
                CaseClash {
                    kept: "Ab".into(),
                    dropped: "aB".into(),
                },
                CaseClash {
                    kept: "Ab".into(),
                    dropped: "ab".into(),
                },
            ],
            "created in order {order:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
