//! `$PBOPREFIX$` markers in a folder laid out as stored: the prefix a loose file is keyed under.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::walk::{rel_path, walk_files};

pub(crate) fn is_prefix_marker(name: &str) -> bool {
    name.eq_ignore_ascii_case("$PBOPREFIX$") || name.eq_ignore_ascii_case("$PBOPREFIX$.txt")
}

/// A marker's prefix: its first non-blank line with `/` separators, trimmed of white space and of slashes at both ends.
pub(crate) fn marker_prefix(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let line = text
        .trim_start_matches('\u{feff}')
        .lines()
        .find(|line| !line.trim().is_empty())?;
    let line = line.trim().replace('\\', "/");
    let prefix = line.trim_matches('/');
    (!prefix.is_empty()).then(|| prefix.to_string())
}

/// Keys the files of one folder tree by the path the game uses.
pub(crate) struct Markers {
    root: PathBuf,
    prefixes: HashMap<PathBuf, Option<String>>,
}

impl Markers {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            prefixes: HashMap::new(),
        }
    }

    pub(crate) fn virtual_rel(&mut self, path: &Path) -> String {
        let mut folder = path.parent();
        while let Some(dir) = folder.filter(|dir| dir.starts_with(&self.root)) {
            if let Some(prefix) = self.prefix_of(dir) {
                return format!("{prefix}/{}", rel_path(dir, path));
            }
            folder = dir.parent();
        }
        rel_path(&self.root, path)
    }

    fn prefix_of(&mut self, folder: &Path) -> Option<&str> {
        if !self.prefixes.contains_key(folder) {
            self.prefixes
                .insert(folder.to_path_buf(), marker_in(folder));
        }
        self.prefixes[folder].as_deref()
    }
}

/// The folders under `root` holding a marker with a prefix, each with that prefix.
pub(crate) fn marked_folders(root: &Path) -> Vec<(PathBuf, String)> {
    let mut markers = Markers::new(root);
    let mut folders: Vec<PathBuf> = walk_files(root, false, |_, name| is_prefix_marker(name))
        .into_iter()
        .filter_map(|marker| marker.parent().map(Path::to_path_buf))
        .collect();
    folders.dedup();
    folders
        .into_iter()
        .filter_map(|folder| {
            let prefix = markers.prefix_of(&folder)?.to_string();
            Some((folder, prefix))
        })
        .collect()
}

/// The files a key from `virtual_rel` may be: below each marked folder whose prefix starts it.
pub(crate) fn disk_candidates<'a>(
    marked: &'a [(PathBuf, String)],
    key: &'a str,
) -> impl Iterator<Item = PathBuf> + 'a {
    marked.iter().filter_map(move |(folder, prefix)| {
        let rest = key.strip_prefix(prefix.as_str())?.strip_prefix('/')?;
        Some(folder.join(rest))
    })
}

fn marker_in(folder: &Path) -> Option<String> {
    let mut markers: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| is_prefix_marker(&name.to_string_lossy()))
        })
        .collect();
    // `$PBOPREFIX$` before `$PBOPREFIX$.txt`; the path breaks a tie, since read order is not fixed.
    markers.sort_by_key(|path| (path.as_os_str().len(), path.clone()));
    markers.iter().find_map(|path| marker_prefix(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(label: &str, files: &[(&str, &str)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("mf-prefix-marker-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    #[test]
    fn a_marker_saved_with_a_byte_order_mark_reads_its_prefix() {
        let root = tree("bom", &[("$PBOPREFIX$", "\u{feff}My\\Mod\r\n")]);

        let prefix = marker_prefix(&root.join("$PBOPREFIX$"));

        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(prefix.as_deref(), Some("My/Mod"));
    }

    #[test]
    fn virtual_rel_keys_files_under_their_folder_prefix() {
        type Files<'a> = &'a [(&'a str, &'a str)];
        let cases: &[(Files, &str, &str)] = &[
            (&[("a/config.cpp", "")], "a/config.cpp", "a/config.cpp"),
            (
                &[("$PBOPREFIX$", "My\\Mod"), ("config.cpp", "")],
                "config.cpp",
                "My/Mod/config.cpp",
            ),
            (
                &[("a/$PBOPREFIX$", "My\\Mod"), ("a/Scripts/3_Game/x.c", "")],
                "a/Scripts/3_Game/x.c",
                "My/Mod/Scripts/3_Game/x.c",
            ),
            (
                &[("a/$PBOPREFIX$", "My\\Mod"), ("a/config.cpp", "")],
                "a/config.cpp",
                "My/Mod/config.cpp",
            ),
            (
                &[("a/$PBOPREFIX$", "My\\Mod"), ("b/config.cpp", "")],
                "b/config.cpp",
                "b/config.cpp",
            ),
            (
                &[
                    ("$PBOPREFIX$", "Outer"),
                    ("a/$PBOPREFIX$", "Inner"),
                    ("a/b/x.c", ""),
                    ("c.c", ""),
                ],
                "a/b/x.c",
                "Inner/b/x.c",
            ),
            (
                &[
                    ("$PBOPREFIX$", "Outer"),
                    ("a/$PBOPREFIX$", "Inner"),
                    ("a/b/x.c", ""),
                    ("c.c", ""),
                ],
                "c.c",
                "Outer/c.c",
            ),
            (
                &[
                    ("$PBOPREFIX$", "My\\Mod"),
                    ("a/$PBOPREFIX$", "\n\t\n"),
                    ("a/config.cpp", ""),
                ],
                "a/config.cpp",
                "My/Mod/a/config.cpp",
            ),
            (
                &[("$PBOPREFIX$.txt", "My\\Mod\n"), ("config.cpp", "")],
                "config.cpp",
                "My/Mod/config.cpp",
            ),
            (
                &[
                    ("$PBOPREFIX$", "First"),
                    ("$PBOPREFIX$.txt", "Second"),
                    ("config.cpp", ""),
                ],
                "config.cpp",
                "First/config.cpp",
            ),
            (
                &[("$PBOPREFIX$", "\\My\\Mod\\\n"), ("config.cpp", "")],
                "config.cpp",
                "My/Mod/config.cpp",
            ),
            (
                &[("$pboprefix$", "My"), ("config.cpp", "")],
                "config.cpp",
                "My/config.cpp",
            ),
            (
                &[
                    ("$pboprefix$", "Lower"),
                    ("$PBOPREFIX$", "Upper"),
                    ("config.cpp", ""),
                ],
                "config.cpp",
                "Upper/config.cpp",
            ),
        ];
        for (i, (files, path, expected)) in cases.iter().enumerate() {
            let root = tree(&format!("case-{i}"), files);
            let key = Markers::new(&root).virtual_rel(&root.join(path));
            let _ = std::fs::remove_dir_all(&root);
            assert_eq!(key, *expected, "case {i}: {files:?}");
        }
    }

    #[test]
    fn an_empty_marker_gives_way_to_the_other_name_in_the_same_folder() {
        let root = tree(
            "empty-beside-txt",
            &[
                ("$PBOPREFIX$", ""),
                ("$PBOPREFIX$.txt", "My\\Mod"),
                ("config.cpp", ""),
            ],
        );
        let key = Markers::new(&root).virtual_rel(&root.join("config.cpp"));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(key, "My/Mod/config.cpp");
    }
}
