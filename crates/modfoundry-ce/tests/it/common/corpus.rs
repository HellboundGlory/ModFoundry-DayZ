//! Shared CE corpus reader: the vanilla mission XML packed inside the
//! game's `worlds_*_ce.pbo` files and the loose XML workshop mods ship.
//!
//! Opt-in: both readers return empty (after a `skipped:` note) unless their env
//! var is set. `MF_DAYZ_INSTALL` is a DayZ install dir; `MF_WORKSHOP_PATH` is a
//! workshop `content/221100` tree. Both are read-only.
#![allow(dead_code)]

use modfoundry_pbo::PboArchive;
use std::path::{Path, PathBuf};

/// Largest loose workshop XML read; PBOs have no such limit.
const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
/// Deepest workshop path component count (relative to the mod folder).
const MAX_DEPTH: usize = 6;

/// One XML file read from the corpus.
pub struct CorpusFile {
    pub label: String,
    /// The map a vanilla mission belongs to; `None` for workshop files.
    pub world: Option<String>,
    /// Entry path inside the PBO, or the path relative to the mod folder.
    pub rel: String,
    pub text: String,
}

/// Every XML entry in the install's `worlds_*_ce.pbo` files.
pub fn vanilla() -> Vec<CorpusFile> {
    let Ok(root) = std::env::var("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let mut out = Vec::new();
    for addons in addons_dirs(&root) {
        for pbo in pbo_files(&addons) {
            let Some(world) = ce_world(&pbo) else {
                continue;
            };
            out.extend(read_pbo(&world, &pbo));
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Every loose `*.xml` file under each workshop mod folder.
pub fn workshop() -> Vec<CorpusFile> {
    let Ok(root) = std::env::var("MF_WORKSHOP_PATH") else {
        eprintln!("skipped: MF_WORKSHOP_PATH not set");
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let mut out = Vec::new();
    let Ok(mods) = std::fs::read_dir(&root) else {
        return out;
    };
    for mod_dir in mods.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let id = mod_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut found = Vec::new();
        walk_xml(&mod_dir, "", 1, &mut found);
        for (path, rel) in found {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() > MAX_XML_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            out.push(CorpusFile {
                label: format!("workshop:{id}:{rel}"),
                world: None,
                rel,
                text: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn read_pbo(world: &str, pbo: &Path) -> Vec<CorpusFile> {
    let Ok(archive) = PboArchive::open(pbo) else {
        eprintln!("pbo unreadable: {}", pbo.display());
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let rel = entry.name_str().replace('\\', "/").to_ascii_lowercase();
        if !rel.ends_with(".xml") {
            continue;
        }
        let Ok(bytes) = archive.read_entry(index) else {
            continue;
        };
        out.push(CorpusFile {
            label: format!("vanilla:{world}:{rel}"),
            world: Some(world.to_owned()),
            rel,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    out
}

fn addons_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for depth1 in child_dirs(root) {
        if is_named(&depth1, "addons") {
            out.push(depth1.clone());
        }
        for depth2 in child_dirs(&depth1) {
            if is_named(&depth2, "addons") {
                out.push(depth2);
            }
        }
    }
    out
}

fn child_dirs(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn is_named(path: &Path, lower: &str) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(lower))
}

fn ce_world(pbo: &Path) -> Option<String> {
    let name = pbo.file_name()?.to_string_lossy().to_ascii_lowercase();
    let world = name.strip_prefix("worlds_")?.strip_suffix("_ce.pbo")?;
    (!world.is_empty()).then(|| world.to_owned())
}

fn pbo_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("pbo"))
        })
        .collect()
}

fn walk_xml(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                walk_xml(&path, &rel, depth + 1, out);
            }
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("xml"))
        {
            out.push((path, rel));
        }
    }
}
