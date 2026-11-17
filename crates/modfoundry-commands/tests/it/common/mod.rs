//! Shared helpers; host discovery is off so no test reads the machine's Steam or DayZ install.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use modfoundry_commands::registry::{self, Session};
use modfoundry_core::Core;
use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
use modfoundry_platform::set_host_discovery;

pub fn core() -> Core {
    core_with(&Session::new())
}

pub fn core_with(session: &Session) -> Core {
    set_host_discovery(false);
    registry::core_with(session)
}

/// Unique temp dir per call (label + pid + per-call counter) so parallel tests
/// in one binary never write the same path.
pub fn tmp(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mf-commands-{label}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

pub fn write(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

pub fn pack(path: &Path, props: &[(&str, &str)], files: &[(&str, &[u8])]) {
    let properties: Vec<(String, String)> = props
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let entries: Vec<PboFileToWrite> = files
        .iter()
        .map(|(n, d)| PboFileToWrite {
            name: (*n).to_string(),
            data: d.to_vec(),
            timestamp: 1,
        })
        .collect();
    let bytes = pack_pbo(&properties, &entries, &PackOptions::default()).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// `text` as a `config.bin`, the form every vanilla config takes.
pub fn config_bin(text: &str) -> Vec<u8> {
    let parsed = modfoundry_config::parser::parse(text);
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    modfoundry_config::write_rap(&parsed.file).unwrap()
}
