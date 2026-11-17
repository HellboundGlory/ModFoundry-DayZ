//! `MF-E2001` findings filtered through the `CfgMods` `files[]` bindings a
//! `config.cpp` above the file declares.

use modfoundry_enforce::lint::E_UNBOUND;
use modfoundry_enforce::Finding;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Drop the `MF-E2001` findings whose folder a `config.cpp` at or above it binds
/// in `CfgMods` `files[]`; every other finding keeps its place.
pub(crate) fn drop_bound(findings: Vec<Finding>) -> Vec<Finding> {
    let mut configs = HashMap::new();
    findings
        .into_iter()
        .filter(|finding| {
            finding.diagnostic.code != E_UNBOUND || !bound_above(&finding.path, &mut configs)
        })
        .collect()
}

/// True when a `config.cpp` at or above `file` binds its folder. Each folder's
/// config is read once per call.
fn bound_above(file: &Path, configs: &mut HashMap<PathBuf, Vec<String>>) -> bool {
    let file = lexical_absolute(file);
    let Some(folder) = file.parent() else {
        return false;
    };
    let mut dir = Some(folder);
    while let Some(here) = dir {
        let segments: Vec<String> = folder
            .strip_prefix(here)
            .unwrap_or(Path::new(""))
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        let entries = configs
            .entry(here.to_path_buf())
            .or_insert_with(|| files_at(here));
        if entries.iter().any(|entry| binds(entry, &segments)) {
            return true;
        }
        dir = here.parent();
    }
    false
}

/// `path` made absolute against the working folder with `.` and `..` folded
/// away, without touching the filesystem: a relative path's own parents stop
/// at the working folder, so configs above it would never be read.
fn lexical_absolute(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The `CfgMods` `files[]` entries of the `config.cpp` in `dir`, empty when
/// there is none or it cannot be read. A folder holding more than one match
/// gives the first by sorted name.
fn files_at(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<std::ffi::OsString> = read
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().eq_ignore_ascii_case("config.cpp"))
        .collect();
    names.sort();
    let Some(name) = names.first() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(dir.join(name)) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let parsed = modfoundry_config::parser::parse(&text);
    modfoundry_config::DayZConfig::from_file(&parsed.file)
        .mods
        .into_iter()
        .flat_map(|module| module.script_modules)
        .flat_map(|module| module.files)
        .collect()
}

/// True when a `files[]` `entry` compiles `folder`: the entry's tail matches the
/// folder's top segments and the entry still carries a prefix before them.
/// Folder segments are relative to the config that declares the entry.
fn binds(entry: &str, folder: &[&str]) -> bool {
    let normalized = entry.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/').trim_start_matches("./");
    let entry: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    (1..=folder.len()).any(|k| {
        entry.len() > k
            && entry[entry.len() - k..]
                .iter()
                .zip(&folder[..k])
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

#[cfg(test)]
mod tests {
    use super::{binds, lexical_absolute};
    use std::path::Path;

    #[test]
    fn a_relative_or_dotted_path_becomes_absolute_without_dots() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            lexical_absolute(Path::new("Scripts/Common/a.c")),
            cwd.join("Scripts/Common/a.c")
        );
        assert_eq!(
            lexical_absolute(Path::new("/x/y/../Common/./a.c")),
            Path::new("/x/Common/a.c")
        );
        assert_eq!(lexical_absolute(Path::new(".")), cwd);
    }

    #[test]
    fn an_entry_binds_its_folder_and_the_folders_below_it() {
        let cases: [(&str, &[&str], bool); 8] = [
            ("MyMod/Scripts/Common", &["Scripts", "Common"], true),
            ("MyMod/Scripts/Common", &["Scripts", "Common", "sub"], true),
            ("MyMod\\scripts\\common", &["Scripts", "Common"], true),
            ("/MyMod/Scripts/Common", &["Scripts", "Common"], true),
            ("MyMod/Scripts/Common", &["Scripts", "Other"], false),
            ("MyMod/Scripts/Common", &[], false),
            ("Scripts/Common", &["Scripts", "Common"], false),
            ("JM/CF/Scripts/3_Game", &["Scripts", "3_Game"], true),
        ];
        for (entry, folder, bound) in cases {
            assert_eq!(binds(entry, folder), bound, "{entry} {folder:?}");
        }
    }
}
