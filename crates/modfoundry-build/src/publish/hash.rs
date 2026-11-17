use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine};
use sha1::{Digest, Sha1};

/// The `bis_longHash` and `bis_shortHash` values Publisher writes for a mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModHash {
    pub long: [u8; 20],
    pub short: u32,
}

impl ModHash {
    /// Hashes of a mod from the trailers of its PBOs.
    pub fn from_trailers(trailers: &[[u8; 20]]) -> Self {
        let mut sorted = trailers.to_vec();
        sorted.sort_unstable();
        let text = hex_lower(sorted.iter().flatten());
        let long: [u8; 20] = Sha1::digest(text.as_bytes()).into();
        let mut short_input = vec![40];
        short_input.extend_from_slice(hex_lower(&long).as_bytes());
        Self {
            long,
            short: rs_hash(&short_input),
        }
    }

    /// The long hash as Base64.
    pub fn long_base64(&self) -> String {
        STANDARD.encode(self.long)
    }

    /// The short hash as Base64 of its little-endian bytes.
    pub fn short_base64(&self) -> String {
        STANDARD.encode(self.short.to_le_bytes())
    }
}

/// The trailer of every PBO directly inside a mod's `dta`, `addons` and `bin` folders.
pub fn pbo_trailers(mod_dir: &Path) -> io::Result<Vec<[u8; 20]>> {
    let mut trailers = Vec::new();
    for entry in fs::read_dir(mod_dir)? {
        let folder = entry?.path();
        if !is_hashed_folder(&folder) {
            continue;
        }
        for file in fs::read_dir(&folder)? {
            let path = file?.path();
            if path.is_file() && is_pbo(&path) {
                trailers.push(read_trailer(&path)?);
            }
        }
    }
    Ok(trailers)
}

fn is_hashed_folder(path: &Path) -> bool {
    path.is_dir()
        && path.file_name().is_some_and(|name| {
            ["dta", "addons", "bin"]
                .iter()
                .any(|folder| name.eq_ignore_ascii_case(folder))
        })
}

fn is_pbo(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pbo"))
}

fn read_trailer(path: &Path) -> io::Result<[u8; 20]> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 20 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is shorter than a PBO trailer", path.display()),
        ));
    }
    file.seek(SeekFrom::End(-20))?;
    let mut trailer = [0u8; 20];
    file.read_exact(&mut trailer)?;
    Ok(trailer)
}

fn hex_lower<'a>(bytes: impl IntoIterator<Item = &'a u8>) -> String {
    bytes
        .into_iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Robert Sedgewick's string hash, with the multiplier sequence Publisher uses.
fn rs_hash(bytes: &[u8]) -> u32 {
    let mut hash = 0u32;
    let mut a = 378_551u32;
    for &byte in bytes {
        hash = hash.wrapping_mul(a).wrapping_add(u32::from(byte));
        a = a.wrapping_mul(63_689);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::ErrorKind, path::PathBuf};

    fn trailer(hex: &str) -> [u8; 20] {
        let mut out = [0u8; 20];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn known_item_hashes_match_publisher() {
        let hash = ModHash::from_trailers(&[trailer("ad60cc7e63c90b7a1fad9da5048f4d91e20f0f62")]);
        assert_eq!(hash.long_base64(), "ipzZRHTSXAN4KCpW8Ui65MBpvv0=");
        assert_eq!(hash.short_base64(), "MfifOA==");
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-hash-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pbo_with_trailer(trailer: [u8; 20]) -> Vec<u8> {
        let mut bytes = vec![0u8; 10];
        bytes.extend_from_slice(&trailer);
        bytes
    }

    #[test]
    fn pbo_trailers_come_from_the_hashed_folders_only() {
        let dir = scratch_dir("trailers");
        let a = [0xaa; 20];
        let b = [0xbb; 20];
        fs::create_dir_all(dir.join("Addons")).unwrap();
        fs::create_dir_all(dir.join("BIN")).unwrap();
        fs::create_dir_all(dir.join("addons/sub")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("Addons/a.pbo"), pbo_with_trailer(a)).unwrap();
        fs::write(dir.join("BIN/b.PBO"), pbo_with_trailer(b)).unwrap();
        fs::write(dir.join("addons/sub/c.pbo"), pbo_with_trailer([0xcc; 20])).unwrap();
        fs::write(dir.join("other/d.pbo"), pbo_with_trailer([0xdd; 20])).unwrap();
        fs::write(dir.join("addons/readme.txt"), pbo_with_trailer([0xee; 20])).unwrap();

        let mut found = pbo_trailers(&dir).unwrap();
        found.sort();
        assert_eq!(found, vec![a, b]);
    }

    #[test]
    fn pbo_shorter_than_a_trailer_is_invalid_data() {
        let dir = scratch_dir("short");
        fs::create_dir_all(dir.join("addons")).unwrap();
        fs::write(dir.join("addons/short.pbo"), [0u8; 10]).unwrap();

        let err = pbo_trailers(&dir).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert!(err.to_string().contains("short.pbo"));
    }

    #[test]
    fn trailer_order_does_not_change_the_hash() {
        let first = trailer("ad60cc7e63c90b7a1fad9da5048f4d91e20f0f62");
        let second = trailer("0123456789abcdef0123456789abcdef01234567");
        assert_eq!(
            ModHash::from_trailers(&[first, second]),
            ModHash::from_trailers(&[second, first])
        );
    }
}
