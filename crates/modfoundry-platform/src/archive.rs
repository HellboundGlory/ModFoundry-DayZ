//! Unpacks a release tarball with the system `tar`, refusing any entry that could land outside its top folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Unpacks `archive` into `into` after checking every entry is a plain file or folder under `top`.
pub fn unpack_tar_gz(archive: &Path, into: &Path, top: &str) -> Result<PathBuf, String> {
    for (kind, name) in listing(archive)? {
        check_entry(kind, &name, top)?;
    }
    fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .arg("--no-same-owner")
        .output()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tar could not unpack {}: {}",
            archive.display(),
            stderr.trim()
        ));
    }
    Ok(into.join(top))
}

/// The type letter and exact path of every entry; both listings name the entries in the same order.
fn listing(archive: &Path) -> Result<Vec<(char, String)>, String> {
    let verbose = list(archive, "-tvzf")?;
    let names = list(archive, "-tzf")?;
    if verbose.len() != names.len() {
        return Err(format!("unreadable listing of {}", archive.display()));
    }
    let kinds = verbose
        .iter()
        .map(|line| line.chars().next().unwrap_or('?'));
    Ok(kinds.zip(names).collect())
}

fn list(archive: &Path, flags: &str) -> Result<Vec<String>, String> {
    let output = Command::new("tar")
        .arg(flags)
        .arg(archive)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tar could not list {}: {}",
            archive.display(),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

fn check_entry(kind: char, name: &str, top: &str) -> Result<(), String> {
    let reason = if !matches!(kind, '-' | 'd') {
        "not a plain file or folder"
    } else if name.starts_with('/') {
        "absolute path"
    } else if name.split('/').any(|part| part == "..") {
        "has a .. part"
    } else if !is_under(name, top) {
        "outside the top folder"
    } else {
        return Ok(());
    };
    Err(format!("refused {name}: {reason}"))
}

fn is_under(name: &str, top: &str) -> bool {
    name.strip_prefix(top)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Marks a file runnable by everyone and writable by its owner.
#[cfg(unix)]
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

/// Marks a file runnable by everyone and writable by its owner.
#[cfg(not(unix))]
pub fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: &str = "ModFoundry-2.3.0-linux-x86_64";

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mf-archive-{name}-{}", std::process::id()))
    }

    fn release_tree(root: &Path) {
        let top = root.join("src").join(TOP);
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("mf"), "mf 2.3.0\n").unwrap();
    }

    /// Packs `members` from `root/src` with extra `tar` `flags` into `root/release.tar.gz`.
    fn tar_gz(root: &Path, flags: &[&str], members: &[&str]) -> PathBuf {
        let archive = root.join("release.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .args(flags)
            .args(members)
            .current_dir(root.join("src"))
            .status()
            .unwrap();
        assert!(status.success(), "tar could not pack the test archive");
        archive
    }

    #[test]
    fn a_release_tarball_unpacks_under_its_top_folder() {
        let _spawning = crate::spawn_lock();
        let root = scratch("good");
        release_tree(&root);
        let archive = tar_gz(&root, &[], &[TOP]);

        let into = root.join("unpacked");
        let top = unpack_tar_gz(&archive, &into, TOP).unwrap();

        assert_eq!(top, into.join(TOP));
        assert_eq!(fs::read_to_string(top.join("mf")).unwrap(), "mf 2.3.0\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_entry_outside_the_top_folder_is_refused_before_anything_is_extracted() {
        let _spawning = crate::spawn_lock();
        let root = scratch("refused");
        release_tree(&root);
        let evil = root.join("src").join("evil");
        fs::write(&evil, "x").unwrap();
        let absolute = evil.display().to_string();
        let owned = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cases = [
            (
                "top/../evil",
                owned(&["--transform=s,^evil,top/../evil,"]),
                owned(&["evil"]),
            ),
            (
                "../evil",
                owned(&["-P", "--transform=s,^evil,../evil,"]),
                owned(&["evil"]),
            ),
            (absolute.as_str(), owned(&["-P"]), vec![absolute.clone()]),
            (
                "other/evil",
                owned(&["--transform=s,^evil,other/evil,"]),
                owned(&["evil"]),
            ),
        ];
        for (index, (entry, flags, members)) in cases.iter().enumerate() {
            let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
            let members: Vec<&str> = members.iter().map(String::as_str).collect();
            let archive = tar_gz(&root, &flags, &members);
            let into = root.join(format!("unpacked-{index}"));

            let error = unpack_tar_gz(&archive, &into, TOP).unwrap_err();

            assert!(error.contains(&format!("refused {entry}")), "{error}");
            assert!(
                fs::read_dir(&into).map_or(true, |mut dir| dir.next().is_none()),
                "nothing may be extracted for {entry}"
            );
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_top_folder_is_refused() {
        let _spawning = crate::spawn_lock();
        let root = scratch("link");
        release_tree(&root);
        let link = root.join("src").join(TOP).join("link");
        std::os::unix::fs::symlink("/nonexistent-target", link).unwrap();
        let archive = tar_gz(&root, &[], &[TOP]);

        let into = root.join("unpacked");
        let error = unpack_tar_gz(&archive, &into, TOP).unwrap_err();

        assert!(error.contains(&format!("refused {TOP}/link")), "{error}");
        assert!(fs::read_dir(&into).map_or(true, |mut dir| dir.next().is_none()));
        fs::remove_dir_all(&root).unwrap();
    }
}
