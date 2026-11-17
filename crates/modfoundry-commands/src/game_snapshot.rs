//! Saves and loads game-build snapshots in mf's data folder.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use modfoundry_gamediff::snapshot::Snapshot;
use serde::{Deserialize, Serialize};

use crate::game_side::{read_side, GameSide, SideCounts};
use crate::search_index::{build_key, game_pbos, is_folder_name};

/// What saving a snapshot of an install gives back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSnapshot {
    pub build: String,
    pub path: PathBuf,
    pub counts: SideCounts,
    pub warnings: Vec<String>,
}

/// The folder snapshots are kept in, under mf's data folder.
pub fn snapshot_dir(data: &Path) -> PathBuf {
    data.join("game-snapshots")
}

/// The snapshot folder under the host's data folder, when it has one.
pub fn default_snapshot_dir() -> Option<PathBuf> {
    modfoundry_platform::app_data_dir().map(|data| snapshot_dir(&data))
}

const GZ: &str = ".json.gz";
const JSON: &str = ".json";

/// Writes `snapshot` gzip-compressed as `<build>.json.gz` in `dir`, replacing any snapshot of that build.
pub fn save_snapshot(dir: &Path, snapshot: &Snapshot) -> Result<PathBuf, String> {
    let build = &snapshot.build;
    if !is_folder_name(build) {
        return Err(format!("build {build} cannot name a snapshot file"));
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let file = dir.join(format!("{build}{GZ}"));
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let n = SAVES.fetch_add(1, Ordering::Relaxed);
    let part = dir.join(format!("{build}{GZ}.{}-{n}.part", std::process::id()));
    let text = snapshot.to_json()?;
    // Synced before the rename: the old build may be gone from disk, so a torn file cannot be replaced.
    let written = std::fs::File::create(&part)
        .and_then(|out| {
            let mut gz = GzEncoder::new(out, Compression::new(6));
            gz.write_all(text.as_bytes())?;
            gz.finish()?.sync_all()
        })
        .and_then(|()| std::fs::rename(&part, &file));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written.map_err(|err| format!("{}: {err}", file.display()))?;
    let _ = std::fs::remove_file(dir.join(format!("{build}{JSON}")));
    Ok(file)
}

/// Reads the install at `game` and saves its snapshot in `dir`, replacing one saved for that build.
pub fn snapshot_install(game: &Path, dir: &Path) -> Result<SavedSnapshot, String> {
    let (build, side) = read_install(game)?;
    save_side(build, side, dir)
}

fn read_install(game: &Path) -> Result<(String, GameSide), String> {
    let mut side = read_side(game)?;
    let build = side.build.take().ok_or_else(|| {
        format!(
            "{} is not a game install; a snapshot needs the build number of an install",
            game.display()
        )
    })?;
    Ok((build, side))
}

fn save_side(build: String, side: GameSide, dir: &Path) -> Result<SavedSnapshot, String> {
    let snapshot = Snapshot::new(build.clone(), side.scripts, side.configs, side.layouts);
    let path = save_snapshot(dir, &snapshot)?;
    Ok(SavedSnapshot {
        build,
        path,
        counts: side.counts,
        warnings: side.warnings,
    })
}

/// Saves a snapshot of `game` in `dir` when none is saved for its build; the line to report, if any.
///
/// An install read with warnings is not saved: a partial snapshot would block the full one for good.
pub fn auto_snapshot(game: &Path, dir: Option<&Path>) -> Option<String> {
    let dir = dir?;
    let pbos = game_pbos(game);
    if pbos.is_empty() {
        return None;
    }
    let build = build_key(game, &pbos);
    if saved_file(dir, &build).is_some() {
        return None;
    }
    let (build, side) = match read_install(game) {
        Ok(read) => read,
        Err(err) => return Some(format!("no snapshot of build {build} saved: {err}")),
    };
    if !side.warnings.is_empty() {
        return Some(format!(
            "no snapshot of build {build} saved: the install read with warnings ({}); run game.snapshot to save it anyway",
            side.warnings.join("; ")
        ));
    }
    Some(match save_side(build.clone(), side, dir) {
        Ok(saved) => format!(
            "saved a snapshot of build {build} to {}",
            saved.path.display()
        ),
        Err(err) => format!("no snapshot of build {build} saved: {err}"),
    })
}

/// The non-empty snapshot file saved for `build`, compressed first.
fn saved_file(dir: &Path, build: &str) -> Option<PathBuf> {
    [GZ, JSON]
        .iter()
        .map(|ext| dir.join(format!("{build}{ext}")))
        .find(|path| non_empty_file(path))
}

fn non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// The snapshot stored in `path`, gzip-compressed or plain JSON.
pub fn load_snapshot(path: &Path) -> Result<Snapshot, String> {
    let named = |err: String| format!("{}: {err}", path.display());
    let bytes = std::fs::read(path).map_err(|err| named(err.to_string()))?;
    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        GzDecoder::new(bytes.as_slice())
            .read_to_string(&mut text)
            .map_err(|err| named(err.to_string()))?;
        text
    } else {
        String::from_utf8(bytes).map_err(|err| named(err.to_string()))?
    };
    Snapshot::from_json(&text).map_err(named)
}

/// The builds `dir` holds a snapshot for, in name order.
pub fn saved_builds(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut builds: Vec<String> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if !non_empty_file(&path) {
                return None;
            }
            // Case-sensitive like the build lookup, which cannot find `.JSON` on Linux.
            let name = path.file_name()?.to_str()?;
            let stem = name.strip_suffix(GZ).or_else(|| name.strip_suffix(JSON))?;
            Some(stem.to_owned())
        })
        .collect();
    builds.sort();
    builds.dedup();
    builds
}

/// The snapshot a compare side names, or `None` when it is read another way.
pub fn snapshot_side(arg: &str, dir: &Path) -> Option<Result<Snapshot, String>> {
    let path = Path::new(arg);
    if path.is_file() && is_snapshot_path(path) {
        return Some(load_snapshot(path));
    }
    if path.exists() {
        return None;
    }
    if is_snapshot_path(path) {
        return Some(Err(format!("{arg} does not exist")));
    }
    if !is_folder_name(arg) {
        return None;
    }
    if let Some(saved) = saved_file(dir, arg) {
        return Some(load_snapshot(&saved));
    }
    let builds = saved_builds(dir);
    let list = if builds.is_empty() {
        "(none saved)".to_owned()
    } else {
        format!("(saved: {})", builds.join(", "))
    };
    Some(Err(format!("no snapshot saved for build {arg} {list}")))
}

/// A snapshot path ends `.json` or `.json.gz`, any case.
pub fn is_snapshot_path(path: &Path) -> bool {
    let name = path.to_string_lossy().to_ascii_lowercase();
    name.ends_with(GZ) || name.ends_with(JSON)
}
