//! Opt-in check of `.edds` decoding against the vanilla `dta/gui.pbo`. Skips unless `MF_DAYZ_INSTALL` is set, and
//! only reads the install.

use modfoundry_pbo::PboArchive;
use modfoundry_texture::edds::{decode_edds, encode_edds};
use modfoundry_texture::mips::mip_chain;
use modfoundry_texture::RgbaImage;
use std::path::PathBuf;

const HEADER_LEN: usize = 128;

#[test]
fn vanilla_gui_edds_decode_and_round_trip() {
    let Some(install) = std::env::var_os("MF_DAYZ_INSTALL").map(PathBuf::from) else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let archive = PboArchive::open(&install.join("dta/gui.pbo")).unwrap();
    let (mut decoded, mut compared, mut skipped) = (0, 0, 0);
    let mut worst: Option<(f64, String)> = None;

    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().into_owned();
        if !name.to_ascii_lowercase().ends_with(".edds") {
            continue;
        }
        let bytes = archive.read_entry(index).unwrap();
        if bytes.get(0x54..0x58) != Some(&[0; 4][..]) {
            skipped += 1;
            continue;
        }

        let levels = decode_edds(&bytes).unwrap_or_else(|err| panic!("{name}: {err}"));
        decoded += 1;
        let encoded = encode_edds(&levels).unwrap();
        assert!(
            decode_edds(&encoded).unwrap() == levels,
            "{name}: does not round-trip"
        );
        if bytes.get(0x68..0x6C) == Some(&[0, 0, 0, 0xFF][..]) {
            assert!(
                encoded[..HEADER_LEN] == bytes[..HEADER_LEN],
                "{name}: header differs"
            );
        }
        if let Some(difference) = level_one_difference(&levels) {
            compared += 1;
            if worst.as_ref().is_none_or(|(worst, _)| difference > *worst) {
                worst = Some((difference, name));
            }
        }
    }

    eprintln!("decoded {decoded}, compared {compared}, skipped {skipped}");
    if let Some((difference, name)) = &worst {
        eprintln!("worst level 1 mean absolute difference {difference:.2}: {name}");
    }
    assert!(decoded >= 100, "only {decoded} files decoded");
    assert!(worst.is_none_or(|(difference, _)| difference < 16.0));
}

/// Mean absolute byte difference between level 1 and the 2x box filter of level 0, for power-of-two files.
fn level_one_difference(levels: &[RgbaImage]) -> Option<f64> {
    let base = &levels[0];
    if levels.len() < 2 || !base.width.is_power_of_two() || !base.height.is_power_of_two() {
        return None;
    }
    let expected = &mip_chain(base.clone(), 2)[1].pixels;
    let actual = &levels[1].pixels;
    let total: u64 = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    Some(total as f64 / actual.len() as f64)
}
