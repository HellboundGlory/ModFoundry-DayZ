//! Deterministic writer for plaintext DayZ PBOs.
//!
//! Output packs entries uncompressed (`packing == 0`, engine-accepted — the
//! vanilla/workshop corpus is overwhelmingly uncompressed and vanilla samples
//! are all uncompressed). Byte-stability holds: the
//! writer is pure over its inputs, so identical properties + files + timestamps
//! yield byte-identical PBOs.
//!
//! A BI PBO ends with a `\0` byte then a 20-byte SHA-1 of the header+payloads
//! (verified against armake2, the Arma 3/DayZ-compatible Rust packer, and the
//! official `DSSignFile`). Real engine tools refuse PBOs without this trailer.

use crate::header::{parse_header, PACKING_VERS};
use anyhow::Result;

/// One file to place inside a packed PBO. `name` uses `\` separators (engine
/// form, e.g. `config.cpp` or `scripts\1_Core\constants.c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PboFileToWrite {
    pub name: String,
    pub data: Vec<u8>,
    /// 32-bit unix timestamp recorded in the entry (determinism requires the
    /// caller to supply a fixed value, e.g. from `SOURCE_DATE_EPOCH` or the
    /// source mtime).
    pub timestamp: u32,
}

/// Writer options. Empty today; kept as a named struct so future settings
/// (e.g. Cprs compression) don't churn callers.
#[derive(Debug, Clone, Default)]
pub struct PackOptions {}

/// Serialize a set of files and properties into a complete PBO byte buffer.
///
/// `properties` and `files` are written in the order given, so callers control
/// determinism. Returns the PBO bytes.
pub fn pack_pbo(
    properties: &[(String, String)],
    files: &[PboFileToWrite],
    _options: &PackOptions,
) -> Result<Vec<u8>> {
    use sha1::{Digest, Sha1};

    let mut header = Vec::new();

    // Property entry: empty name + record with packing == "Vers".
    header.push(0);
    header.extend_from_slice(&PACKING_VERS.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());

    for (k, v) in properties {
        write_name(&mut header, k, "property key")?;
        write_asciiz(&mut header, v)?;
    }
    header.push(0);

    for f in files {
        write_name(&mut header, &f.name, "entry name")?;
        let size = size_field(&f.name, f.data.len())?;
        header.extend_from_slice(&0u32.to_le_bytes()); // packing: raw
        header.extend_from_slice(&size.to_le_bytes()); // original_size
        header.extend_from_slice(&0u32.to_le_bytes()); // reserved
        header.extend_from_slice(&f.timestamp.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes()); // data_size
    }

    // Empty-name terminator entry (with its zero record) — data begins after it.
    header.push(0);
    header.extend_from_slice(&[0u8; 20]);

    let payload_len: usize = files.iter().map(|f| f.data.len()).sum();
    let mut out = Vec::with_capacity(header.len() + payload_len + 21);
    out.extend_from_slice(&header);
    let mut hasher = Sha1::new();
    hasher.update(&header);
    for f in files {
        out.extend_from_slice(&f.data);
        hasher.update(&f.data);
    }

    // BI PBO trailer: a `\0` byte then the 20-byte SHA-1 of header+payloads.
    // armake2/engine/DSSignFile expect this (verified empirically); omitting it
    // makes a valid-looking PBO that the official tools refuse.
    out.push(0);
    out.extend_from_slice(&hasher.finalize());

    // Sanity: the writer round-trips its own header.
    let (hdr, data_offset) = parse_header(&out)?;
    anyhow::ensure!(
        hdr.entries.len() == files.len(),
        "packed header lists {} entries, expected {}",
        hdr.entries.len(),
        files.len()
    );
    anyhow::ensure!(
        out.len() == data_offset + payload_len + 21,
        "packed size {} does not match the header",
        out.len()
    );

    Ok(out)
}

/// Append `s` and a NUL. Rejects embedded NULs (would corrupt framing).
fn write_asciiz(out: &mut Vec<u8>, s: &str) -> Result<()> {
    if s.as_bytes().contains(&0) {
        anyhow::bail!("string contains NUL byte: {s:?}");
    }
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    Ok(())
}

/// An empty name breaks the header: an empty entry name reads as its terminator,
/// an empty property key ends the property list early.
fn write_name(out: &mut Vec<u8>, s: &str, what: &str) -> Result<()> {
    if s.is_empty() {
        anyhow::bail!("empty {what}");
    }
    write_asciiz(out, s)
}

fn size_field(name: &str, len: usize) -> Result<u32> {
    u32::try_from(len)
        .map_err(|_| anyhow::anyhow!("{name} is {len} bytes; a PBO entry holds at most 4 GiB"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};

    fn sample_files() -> Vec<PboFileToWrite> {
        vec![
            PboFileToWrite {
                name: "config.cpp".into(),
                data: b"class CfgPatches {}".to_vec(),
                timestamp: 10,
            },
            PboFileToWrite {
                name: "scripts\\1_Core\\constants.c".into(),
                data: b"class Constants {}".to_vec(),
                timestamp: 20,
            },
        ]
    }

    fn sample_props() -> Vec<(String, String)> {
        vec![
            ("product".to_owned(), "dayz".to_owned()),
            ("prefix".to_owned(), "MyMod".to_owned()),
        ]
    }

    #[test]
    fn is_byte_deterministic() {
        let a = pack_pbo(&sample_props(), &sample_files(), &PackOptions {}).unwrap();
        let b = pack_pbo(&sample_props(), &sample_files(), &PackOptions {}).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn output_parses_with_expected_shape() {
        let files = sample_files();
        let bytes = pack_pbo(&sample_props(), &files, &PackOptions {}).unwrap();
        let (hdr, data_offset) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("prefix"), Some("MyMod"));
        assert_eq!(hdr.entries.len(), 2);
        assert_eq!(hdr.entries[0].name_str(), "config.cpp");
        assert_eq!(hdr.entries[0].timestamp, 10);
        assert_eq!(hdr.entries[0].data_size, files[0].data.len() as u32);
        assert_eq!(hdr.entries[1].timestamp, 20);
        // payload region holds both files contiguously; BI trailer follows.
        let payload_end = data_offset + files[0].data.len() + files[1].data.len();
        assert_eq!(
            &bytes[data_offset..payload_end],
            [files[0].data.as_slice(), files[1].data.as_slice()].concat()
        );
        assert_eq!(bytes.len(), payload_end + 21, "\0 + 20-byte SHA-1 trailer");
    }

    #[test]
    fn rejects_strings_that_break_header_framing() {
        let mut unnamed = sample_files();
        unnamed[0].name = String::new();
        let cases = vec![
            (
                "NUL in property key",
                vec![("bad\0key".to_owned(), "v".to_owned())],
                sample_files(),
            ),
            (
                "empty property key",
                vec![(String::new(), "v".to_owned())],
                sample_files(),
            ),
            ("empty file name", sample_props(), unnamed),
        ];
        for (label, props, files) in cases {
            assert!(
                pack_pbo(&props, &files, &PackOptions {}).is_err(),
                "{label} was accepted"
            );
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn an_entry_over_4_gib_is_refused() {
        let err = size_field("big.paa", u32::MAX as usize + 1).unwrap_err();
        assert!(err.to_string().contains("big.paa"));
    }

    #[test]
    fn output_has_bi_sha1_trailer() {
        let bytes = pack_pbo(&sample_props(), &sample_files(), &PackOptions {}).unwrap();
        // Last 21 bytes = `\0` + SHA-1 over everything before them.
        assert_eq!(bytes[bytes.len() - 21], 0);
        let body = &bytes[..bytes.len() - 21];
        let digest = Sha1::digest(body);
        assert_eq!(&bytes[bytes.len() - 20..], digest.as_slice());
    }
}
