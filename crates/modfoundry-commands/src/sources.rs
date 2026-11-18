use std::path::{Path, PathBuf};

use modfoundry_pbo::PboArchive;

/// The display name a mod declares in `mod.cpp`/`meta.cpp`; `None` for a `.pbo`
/// or a directory without either file.
pub(crate) fn mod_display_name(root: &Path) -> Option<String> {
    if root.is_file() {
        return None;
    }
    let read = |name: &str| {
        std::fs::read(root.join(name))
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    };
    modfoundry_config::mod_display_name(read("mod.cpp").as_deref(), read("meta.cpp").as_deref())
}

pub(crate) fn game_dir(requested: Option<&PathBuf>) -> Option<PathBuf> {
    requested
        .cloned()
        .or_else(|| modfoundry_platform::find_dayz_installs().0)
}

pub(crate) fn find_child(dir: &Path, name: &str, want_dir: bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| {
            entry
                .file_type()
                .map(|kind| {
                    if want_dir {
                        kind.is_dir()
                    } else {
                        kind.is_file()
                    }
                })
                .unwrap_or(false)
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
}

pub(crate) fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Each DLC map's `addons` folder, by map name.
pub(crate) fn map_addon_dirs(game: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(game)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            !path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.eq_ignore_ascii_case("addons")
                    || name.eq_ignore_ascii_case("dta")
                    || name.starts_with('@')
            })
        })
        .filter_map(|map| find_child(&map, "addons", true))
        .filter(|map_addons| holds_signed_pbo(map_addons))
        .collect();
    dirs.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    dirs
}

/// The vanilla config folders: `addons`, then `dta`, then each DLC map's `addons`, by map name.
pub(crate) fn vanilla_addon_dirs(game: &Path) -> Vec<PathBuf> {
    find_child(game, "addons", true)
        .into_iter()
        .chain(find_child(game, "dta", true))
        .chain(map_addon_dirs(game))
        .collect()
}

/// Whether `addons` holds a PBO with a `.dayz.bisign` beside it, the name Bohemia's key gives a signature.
fn holds_signed_pbo(addons: &Path) -> bool {
    std::fs::read_dir(addons)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".pbo.dayz.bisign")
        })
}

pub(crate) fn open_pbo(pbo: &Path, warnings: &mut Vec<String>) -> Option<PboArchive> {
    match PboArchive::open(pbo) {
        Ok(archive) => Some(archive),
        Err(err) => {
            warnings.push(format!("{}: {err}", pbo.display()));
            None
        }
    }
}
