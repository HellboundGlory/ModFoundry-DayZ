//! Shared corpus reader for the config gates.
//!
//! Reads every `config.bin`/`config.cpp` entry at any depth from the PBOs of a
//! real install, so the config gates measure the same files. Opt-in: both
//! readers return empty (after a `skipped:` note) unless their env var is set.
//!
//! `MF_DAYZ_INSTALL` is a DayZ install dir; `MF_WORKSHOP_PATH` is a workshop
//! `content/221100` tree. Both are read-only.
#![allow(dead_code)]

use modfoundry_config::ast::ConfigFile;
use modfoundry_config::parser::parse;
use modfoundry_config::rap::read_rap;
use modfoundry_pbo::PboArchive;
use std::path::{Path, PathBuf};

/// One config entry decoded from a PBO.
pub struct CorpusConfig {
    pub label: String,
    pub is_bin: bool,
    /// The parsed tree, or `None` when `config.bin` failed to read.
    pub file: Option<ConfigFile>,
    /// `config.cpp` source text (lossy-decoded), or `None` for `config.bin`.
    pub text: Option<String>,
    /// Parse issues for a `config.cpp`.
    pub issues: usize,
    /// Reader/decompression failure text, when `file` is `None`.
    pub error: Option<String>,
}

/// Every config in the vanilla install's `Addons`, `dta` and DLC map folder PBOs.
pub fn vanilla_configs() -> Vec<CorpusConfig> {
    let Ok(root) = std::env::var("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let mut pbos = Vec::new();
    if let Some(addons) = find_dir(&root, "addons") {
        pbos.extend(pbo_files(&addons));
    }
    if let Some(dta) = find_dir(&root, "dta") {
        pbos.extend(pbo_files(&dta));
    }
    for map in map_addons_dirs(&root) {
        pbos.extend(pbo_files(&map));
    }
    pbos.sort();
    collect("vanilla", &pbos)
}

/// Every config in each workshop mod's `addons` PBOs.
pub fn workshop_configs() -> Vec<CorpusConfig> {
    let Ok(root) = std::env::var("MF_WORKSHOP_PATH") else {
        eprintln!("skipped: MF_WORKSHOP_PATH not set");
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let mut pbos = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let mod_dir = entry.path();
            if !mod_dir.is_dir() {
                continue;
            }
            if let Some(addons) = find_dir(&mod_dir, "addons") {
                pbos.extend(pbo_files(&addons));
            }
        }
    }
    pbos.sort();
    collect("workshop", &pbos)
}

fn collect(layer: &str, pbos: &[PathBuf]) -> Vec<CorpusConfig> {
    let mut out = Vec::new();
    for pbo in pbos {
        out.extend(read_configs(layer, pbo));
    }
    let bins = out.iter().filter(|c| c.is_bin).count();
    let cpps = out.iter().filter(|c| !c.is_bin).count();
    let failed = out.iter().filter(|c| c.error.is_some()).count();
    let clean = out.iter().filter(|c| !c.is_bin && c.issues == 0).count();
    eprintln!(
        "{layer}: {} PBOs -> {bins} config.bin ({failed} unreadable) + {cpps} config.cpp ({clean} clean)",
        pbos.len()
    );
    out
}

fn read_configs(layer: &str, pbo: &Path) -> Vec<CorpusConfig> {
    let Ok(archive) = PboArchive::open(pbo) else {
        eprintln!("pbo unreadable: {}", pbo.display());
        return Vec::new();
    };
    let pbo_name = pbo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().replace('\\', "/");
        let Some(is_bin) = config_kind(&name) else {
            continue;
        };
        let label = format!("{layer}:{pbo_name}:{name}");
        match archive.read_entry(index) {
            Ok(bytes) => out.push(decode(label, is_bin, &bytes)),
            Err(e) => out.push(failed(label, is_bin, &e.to_string())),
        }
    }
    out
}

fn failed(label: String, is_bin: bool, error: &str) -> CorpusConfig {
    CorpusConfig {
        label,
        is_bin,
        file: None,
        text: None,
        issues: 0,
        error: Some(error.to_owned()),
    }
}

fn decode(label: String, is_bin: bool, bytes: &[u8]) -> CorpusConfig {
    if is_bin {
        match read_rap(bytes) {
            Ok(file) => CorpusConfig {
                label,
                is_bin,
                file: Some(file),
                text: None,
                issues: 0,
                error: None,
            },
            Err(e) => failed(label, is_bin, &e.to_string()),
        }
    } else {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let parsed = parse(&text);
        CorpusConfig {
            label,
            is_bin,
            issues: parsed.issues.len(),
            file: Some(parsed.file),
            text: Some(text),
            error: None,
        }
    }
}

fn config_kind(name: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("config.bin") {
        Some(true)
    } else if lower.ends_with("config.cpp") {
        Some(false)
    } else {
        None
    }
}

fn find_dir(parent: &Path, lower: &str) -> Option<PathBuf> {
    std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .find(|e| {
            e.path().is_dir() && e.file_name().to_string_lossy().to_ascii_lowercase() == lower
        })
        .map(|e| e.path())
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

/// Each DLC map folder's `addons` (`sakhal/Addons`, `nasdara/addons`): a top-level folder,
/// not `@`-prefixed, whose `addons` holds a `*.pbo.dayz.bisign`.
fn map_addons_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with('@')
                        || name.eq_ignore_ascii_case("addons")
                        || name.eq_ignore_ascii_case("dta")
                })
        })
        .filter_map(|map| find_dir(&map, "addons"))
        .filter(|addons| {
            std::fs::read_dir(addons).is_ok_and(|files| {
                files.flatten().any(|file| {
                    file.file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .ends_with(".pbo.dayz.bisign")
                })
            })
        })
        .collect()
}
