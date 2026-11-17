use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The path canonicalized, else made absolute when it does not exist.
pub(crate) fn resolved(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path).or_else(|_| std::path::absolute(path))
}

/// The path canonicalized, else made absolute, else as given.
pub(crate) fn resolved_or_given(path: &Path) -> PathBuf {
    resolved(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The path made absolute, else as given.
pub(crate) fn absolute_or_given(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The path made absolute with `.` and `..` folded by name, no link followed.
pub(crate) fn folded(path: &Path) -> PathBuf {
    walk_parts(path, |at| at)
}

/// The path with every part that exists resolved, left to right, so a link is followed before
/// a later `..` climbs out of it; a cache that is not made yet can still sit behind a link.
pub(crate) fn resolved_through_links(path: &Path) -> PathBuf {
    walk_parts(path, |at| fs::canonicalize(&at).unwrap_or(at))
}

fn walk_parts(path: &Path, settle: impl Fn(PathBuf) -> PathBuf) -> PathBuf {
    let mut at = PathBuf::new();
    for part in absolute_or_given(path).components() {
        match part {
            Component::ParentDir => {
                at.pop();
            }
            Component::CurDir => {}
            part => {
                at.push(part);
                at = settle(at);
            }
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-paths-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn resolved_follows_a_link_to_an_existing_path() {
        let dir = scratch("resolved-link");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("file.txt"), "").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolved(&link.join("file.txt")).unwrap(),
            fs::canonicalize(real.join("file.txt")).unwrap()
        );
    }

    #[test]
    fn resolved_makes_a_missing_relative_path_absolute() {
        let name = "mf-paths-missing-relative";
        assert_eq!(
            resolved(Path::new(name)).unwrap(),
            std::env::current_dir().unwrap().join(name)
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolved_through_links_keeps_a_missing_tail_behind_a_link() {
        let dir = scratch("tail-behind-link");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let real = fs::canonicalize(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolved_through_links(&link.join("cache/new")),
            real.join("cache/new")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolved_through_links_applies_dotdot_after_a_missing_folder() {
        let dir = scratch("dotdot-after-missing");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let real = fs::canonicalize(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolved_through_links(&link.join("missing/../x")),
            real.join("x")
        );
    }
}
