//! The recursive directory walks shared by the CE, config and layout source
//! scanners.

use std::path::{Path, PathBuf};

const MAX_WALK_DEPTH: usize = 6;
/// Folders this deep and below are not read; mod prefixes nest and vanilla itself needs six levels.
const MAX_FOLDER_DEPTH: usize = 24;

/// Every file under `root` the `keep` test accepts, deterministically ordered.
pub(crate) fn walk_files(
    root: &Path,
    skip_top_builds: bool,
    keep: impl Fn(&std::fs::DirEntry, &str) -> bool,
) -> Vec<PathBuf> {
    walk_files_to(root, MAX_WALK_DEPTH, skip_top_builds, keep).0
}

/// The accepted files under `root`, and the folders at `max_depth` that were not entered, both ordered.
pub(crate) fn walk_files_to(
    root: &Path,
    max_depth: usize,
    skip_top_builds: bool,
    keep: impl Fn(&std::fs::DirEntry, &str) -> bool,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut walk = Walk {
        max_depth,
        skip_top_builds,
        keep: &keep,
        files: Vec::new(),
        skipped: Vec::new(),
    };
    walk.dir(root, 0);
    walk.files.sort();
    walk.skipped.sort();
    (walk.files, walk.skipped)
}

struct Walk<'a, Keep> {
    max_depth: usize,
    skip_top_builds: bool,
    keep: &'a Keep,
    files: Vec<PathBuf>,
    skipped: Vec<PathBuf>,
}

impl<Keep: Fn(&std::fs::DirEntry, &str) -> bool> Walk<'_, Keep> {
    fn dir(&mut self, dir: &Path, depth: usize) {
        if depth == self.max_depth {
            self.skipped.push(dir.to_path_buf());
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if name.starts_with('.') || (depth == 0 && self.skip_top_builds && name == "builds")
                {
                    continue;
                }
                self.dir(&entry.path(), depth + 1);
            } else if kind.is_file() && (self.keep)(&entry, &name) {
                self.files.push(entry.path());
            }
        }
    }
}

/// The stored path of a PBO entry: the prefix and the `/`-separated entry name.
pub(crate) fn entry_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

pub(crate) fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The files under `root` the `keep` test accepts, warning about folders nested past the depth cap.
pub(crate) fn walk_layout_tree(
    root: &Path,
    keep: impl Fn(&std::fs::DirEntry, &str) -> bool,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let (files, skipped) = walk_files_to(root, MAX_FOLDER_DEPTH, false, keep);
    if let Some(first) = skipped.first() {
        let warning = format!(
            "{} folder(s) under {} are nested deeper than {MAX_FOLDER_DEPTH} levels and were not read; first: {}",
            skipped.len(),
            root.display(),
            first.display()
        );
        // The layouts and the mod's loose files are often walked from the same root.
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    files
}
