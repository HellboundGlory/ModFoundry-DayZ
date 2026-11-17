use std::fs;
use std::path::Path;

use modfoundry_config::mod_display_name;
use modfoundry_sign::Bisign;

use super::checks::{build_files, has_extension};
use super::hash::{pbo_trailers, ModHash};
use super::tags::ModFacts;
use crate::build::source_root_file;

const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;

/// The facts about a built mod folder that its Workshop key-value tags need.
pub fn mod_facts(mod_dir: &Path) -> anyhow::Result<ModFacts> {
    let hash = ModHash::from_trailers(&pbo_trailers(mod_dir)?);
    let mod_cpp = source_root_file(mod_dir, "mod.cpp")
        .map(fs::read_to_string)
        .transpose()?;
    let mut facts = ModFacts {
        long_hash: hash.long_base64(),
        short_hash: hash.short_base64(),
        display_name: mod_display_name(mod_cpp.as_deref(), None),
        ..ModFacts::default()
    };
    let meta_cpp = source_root_file(mod_dir, "meta.cpp");
    for file in build_files(mod_dir)? {
        let path = mod_dir.join(&file);
        if meta_cpp.as_ref() != Some(&path) {
            facts.size += fs::metadata(&path)?.len();
        }
        if has_extension(&file, "bisign") {
            if let Ok(bisign) = Bisign::parse(&fs::read(&path)?) {
                facts.signers.insert(bisign.key.authority);
            }
        }
        if has_extension(&file, "dll") {
            match pe_machine(&fs::read(&path)?) {
                Some(IMAGE_FILE_MACHINE_I386) => facts.win32 = true,
                Some(IMAGE_FILE_MACHINE_AMD64) => facts.win64 = true,
                _ => {}
            }
        }
    }
    Ok(facts)
}

fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let off = u32::from_le_bytes(bytes.get(0x3C..0x40)?.try_into().ok()?) as usize;
    let header = bytes.get(off..)?;
    if header.get(..4)? != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes(header.get(4..6)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_sign::{KeyBlob, SignVersion};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TRAILER_HEX: &str = "ad60cc7e63c90b7a1fad9da5048f4d91e20f0f62";
    const LONG_HASH: &str = "ipzZRHTSXAN4KCpW8Ui65MBpvv0=";
    const SHORT_HASH: &str = "MfifOA==";

    static SCRATCH: AtomicUsize = AtomicUsize::new(0);

    fn scratch_dir(label: &str) -> PathBuf {
        let n = SCRATCH.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mf-facts-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn gardening_pbo() -> Vec<u8> {
        let mut bytes = vec![0u8; 10];
        bytes.extend(
            (0..20).map(|i| u8::from_str_radix(&TRAILER_HEX[2 * i..2 * i + 2], 16).unwrap()),
        );
        bytes
    }

    #[test]
    fn hashes_come_from_the_pbo_trailers() {
        let dir = scratch_dir("hashes");
        fs::create_dir_all(dir.join("addons")).unwrap();
        fs::write(dir.join("addons/GardeningPlus.pbo"), gardening_pbo()).unwrap();

        let facts = mod_facts(&dir).unwrap();
        assert_eq!(facts.long_hash, LONG_HASH);
        assert_eq!(facts.short_hash, SHORT_HASH);
    }

    fn bisign(authority: &str) -> Vec<u8> {
        Bisign {
            key: KeyBlob {
                authority: authority.to_owned(),
                private: false,
                bits: 1024,
                exponent: 65537,
                components: vec![vec![0x5a; 128]],
            },
            version: SignVersion::V3,
            signatures: [vec![0x11; 128], vec![0x22; 128], vec![0x33; 128]],
        }
        .to_bytes()
    }

    const MOD_CPP: &str = "name = \"Gardening Plus\";";

    fn write_gardening_mod(dir: &Path) -> u64 {
        fs::create_dir_all(dir.join("addons")).unwrap();
        fs::create_dir_all(dir.join("keys")).unwrap();
        let pbo = gardening_pbo();
        let sign = bisign("IceBlade");
        fs::write(dir.join("addons/GardeningPlus.pbo"), &pbo).unwrap();
        fs::write(dir.join("addons/GardeningPlus.pbo.IceBlade.bisign"), &sign).unwrap();
        fs::write(dir.join("keys/IceBlade.bikey"), b"key").unwrap();
        fs::write(dir.join("mod.cpp"), MOD_CPP).unwrap();
        (pbo.len() + sign.len() + b"key".len() + MOD_CPP.len()) as u64
    }

    #[test]
    fn signers_come_from_the_bisign_files() {
        let dir = scratch_dir("signers");
        write_gardening_mod(&dir);

        assert_eq!(
            mod_facts(&dir).unwrap().signers,
            BTreeSet::from(["IceBlade".to_owned()])
        );
    }

    #[test]
    fn size_is_the_length_of_every_file() {
        let dir = scratch_dir("size");
        let expected = write_gardening_mod(&dir);

        assert_eq!(mod_facts(&dir).unwrap().size, expected);
    }

    #[test]
    fn a_root_meta_cpp_is_not_counted_in_size() {
        let dir = scratch_dir("meta");
        let expected = write_gardening_mod(&dir);
        fs::write(dir.join("Meta.CPP"), "class CfgMods {};").unwrap();

        assert_eq!(mod_facts(&dir).unwrap().size, expected);
    }

    #[test]
    fn display_name_comes_from_the_root_mod_cpp() {
        let dir = scratch_dir("name");
        write_gardening_mod(&dir);

        assert_eq!(
            mod_facts(&dir).unwrap().display_name,
            Some("Gardening Plus".to_owned())
        );
    }

    fn pe_header(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x80];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        bytes[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    #[test]
    fn pe_machine_is_read_from_a_well_formed_header_only() {
        let mut no_pe_signature = pe_header(0x8664);
        no_pe_signature[0x40..0x44].copy_from_slice(b"NE\0\0");
        let mut no_mz = pe_header(0x8664);
        no_mz[..2].copy_from_slice(b"ZM");
        let cases: [(&str, Vec<u8>, Option<u16>); 5] = [
            (
                "64-bit",
                pe_header(IMAGE_FILE_MACHINE_AMD64),
                Some(IMAGE_FILE_MACHINE_AMD64),
            ),
            (
                "32-bit",
                pe_header(IMAGE_FILE_MACHINE_I386),
                Some(IMAGE_FILE_MACHINE_I386),
            ),
            ("too short", b"MZ".to_vec(), None),
            ("no PE signature", no_pe_signature, None),
            ("no MZ", no_mz, None),
        ];
        for (label, bytes, expected) in cases {
            assert_eq!(pe_machine(&bytes), expected, "{label}");
        }
    }

    #[test]
    fn dlls_set_win32_and_win64_from_their_pe_machine() {
        let dir = scratch_dir("dll");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(
            dir.join("bin/Client.DLL"),
            pe_header(IMAGE_FILE_MACHINE_AMD64),
        )
        .unwrap();
        fs::write(dir.join("bin/Old.dll"), pe_header(IMAGE_FILE_MACHINE_I386)).unwrap();

        let facts = mod_facts(&dir).unwrap();
        assert!(facts.win32);
        assert!(facts.win64);
    }
}
