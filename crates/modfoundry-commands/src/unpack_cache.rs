//! Unpacked copies of PBOs and mod folders, kept under a cache root and reused while unchanged.

use crate::pbo_ops::{pbos_of, unpack_under_prefix, PboUnpackArgs};
use anyhow::{ensure, Context, Result};
use modfoundry_vfs::sha256_hex;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A source's unpacked copy and what could not be unpacked from it.
pub struct Unpacked {
    pub dir: PathBuf,
    pub left_out: Option<String>,
}

/// The host cache folder for unpacked copies, if the platform has a cache folder.
pub fn default_root() -> Option<PathBuf> {
    modfoundry_platform::app_cache_dir().map(|base| base.join("modfoundry").join("unpacked"))
}

/// The unpacked copy of a PBO or a mod folder under `root`, made when missing or out of date.
pub fn unpacked(root: &Path, source: &Path) -> Result<Unpacked> {
    let pbos = fingerprint(source)?;
    ensure!(!pbos.is_empty(), "no PBOs under {}", source.display());
    let key = entry_key(source)?;
    let dir = root.join(&key);
    let stamp = root.join(format!("{key}.stamp"));
    fs::create_dir_all(root)?;
    // Held until this call returns: one caller makes the copy, the others wait and reuse it.
    let lock = fs::File::create(root.join(format!("{key}.lock")))?;
    lock.lock()?;
    if let Some(copy) = fresh(&dir, &stamp, &pbos) {
        return Ok(copy);
    }
    remove_if_exists(&stamp)?;
    remove_if_exists(&dir)?;
    remove_old_temps(root, &key)?;
    let temp = root.join(format!("{key}.tmp-{}", std::process::id()));
    let args = PboUnpackArgs {
        path: source.to_path_buf(),
        out: temp.clone(),
    };
    let done = match unpack_under_prefix(&args) {
        Ok(done) => done,
        Err(error) => {
            let _ = remove_if_exists(&temp);
            return Err(error);
        }
    };
    fs::rename(&temp, &dir)?;
    let left_out = done.left_out.unwrap_or_default().replace(['\n', '\r'], " ");
    fs::write(&stamp, stamp_text(&left_out, &pbos))?;
    Ok(Unpacked {
        dir,
        left_out: (!left_out.is_empty()).then_some(left_out),
    })
}

/// Removes the temp folders of this entry: under the lock none is in use, so each is
/// what a run that died left behind.
fn remove_old_temps(root: &Path, key: &str) -> std::io::Result<()> {
    let prefix = format!("{key}.tmp-");
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

/// `<name>-<hash>`: the source's file name, then a hash of its resolved path, so two
/// folders with one name get two copies.
fn entry_key(source: &Path) -> Result<String> {
    let resolved = crate::paths::resolved(source)?;
    let hash = sha256_hex(resolved.as_os_str().as_encoded_bytes());
    Ok(format!("{}-{}", safe_name(source), &hash[..12]))
}

fn safe_name(source: &Path) -> String {
    let name = source.file_name().unwrap_or_default();
    name.as_encoded_bytes()
        .iter()
        .map(|&byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
                char::from(byte)
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

fn fingerprint(source: &Path) -> Result<Vec<String>> {
    pbos_of(source)
        .iter()
        .map(|pbo| {
            let meta =
                fs::metadata(pbo).with_context(|| format!("cannot read {}", pbo.display()))?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            let name = pbo.file_name().unwrap_or_default().to_string_lossy();
            Ok(format!("{name}\t{}\t{modified}", meta.len()))
        })
        .collect()
}

fn stamp_text(left_out: &str, pbos: &[String]) -> String {
    let mut lines = vec!["2", left_out];
    lines.extend(pbos.iter().map(String::as_str));
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn fresh(dir: &Path, stamp: &Path, pbos: &[String]) -> Option<Unpacked> {
    if !dir.is_dir() {
        return None;
    }
    let text = fs::read_to_string(stamp).ok()?;
    let left_out = text.lines().nth(1)?;
    (stamp_text(left_out, pbos) == text).then(|| Unpacked {
        dir: dir.to_path_buf(),
        left_out: (!left_out.is_empty()).then(|| left_out.to_owned()),
    })
}

fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
