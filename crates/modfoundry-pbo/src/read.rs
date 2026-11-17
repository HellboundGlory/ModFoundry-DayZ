//! Reading a PBO file's contents: header parse plus per-entry payload unpacking
//! (raw bytes directly, Cprs entries via [`crate::cprs::decompress`]).

use crate::cprs;
use crate::header::{parse_header, Entry, PboHeader, PACKING_CPRS, PACKING_RAW};
use std::path::Path;

/// Parse a PBO header and read every entry's payload from the same byte slice.
/// Returns the header plus one byte buffer per entry, in entry order.
pub fn read_pbo(bytes: &[u8]) -> anyhow::Result<(PboHeader, Vec<Vec<u8>>)> {
    let (header, data_offset) = parse_header(bytes)?;
    let mut out = Vec::with_capacity(header.entries.len());
    let mut pos = data_offset;
    for e in &header.entries {
        let end = pos
            .checked_add(e.data_size as usize)
            .ok_or_else(|| anyhow::anyhow!("payload size overflow for {}", e.name_str()))?;
        if end > bytes.len() {
            anyhow::bail!("payload for {} runs past end of file", e.name_str());
        }
        let payload = unpack_entry(&bytes[pos..end], e)?;
        out.push(payload);
        pos = end;
    }
    Ok((header, out))
}

/// Unpack a single entry's raw payload slice into its stored form.
pub fn unpack_entry(raw: &[u8], entry: &Entry) -> anyhow::Result<Vec<u8>> {
    match entry.packing {
        PACKING_RAW => Ok(raw.to_vec()),
        PACKING_CPRS => cprs::decompress(raw, entry.original_size as usize),
        other => anyhow::bail!(
            "entry `{}` uses unknown packing method {other:#x}",
            entry.name_str()
        ),
    }
}

/// Read a PBO file from disk and parse it (header + payloads).
pub fn read_pbo_file(path: &Path) -> anyhow::Result<(PboHeader, Vec<Vec<u8>>)> {
    let bytes = std::fs::read(path)?;
    read_pbo(&bytes)
}

/// Bytes read on the first attempt at a header; grown until the header ends.
const HEADER_PROBE_BYTES: u64 = 64 * 1024;

/// Parse only the header of a PBO file on disk, without reading its payload.
/// Returns the header and the offset where the payload region begins.
pub fn read_header_file(path: &Path) -> anyhow::Result<(PboHeader, usize)> {
    use std::io::Read;

    let file_len = std::fs::metadata(path)?.len();
    let mut probe = HEADER_PROBE_BYTES;
    loop {
        let take = probe.min(file_len);
        let mut buf = Vec::with_capacity(take as usize);
        std::fs::File::open(path)?
            .take(take)
            .read_to_end(&mut buf)?;
        // A short read means the file shrank since its length was taken.
        let short_read = (buf.len() as u64) < take;
        let bound = if short_read {
            buf.len()
        } else {
            file_len as usize
        };
        match crate::header::parse_header_bounded(&buf, bound) {
            Ok(parsed) => return Ok(parsed),
            Err(e) if !short_read && take < file_len && e.is::<crate::header::NeedsMoreBytes>() => {
                probe = probe.saturating_mul(4)
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{pack_pbo, PackOptions, PboFileToWrite};

    #[test]
    fn round_trips_raw_files() {
        let files = vec![
            PboFileToWrite {
                name: "config.cpp".into(),
                data: b"class CfgPatches {}".to_vec(),
                timestamp: 42,
            },
            PboFileToWrite {
                name: "scripts\\1_Core\\constants.c".into(),
                data: b"class Constants {}".to_vec(),
                timestamp: 99,
            },
        ];
        let properties = vec![
            ("product".to_owned(), "dayz".to_owned()),
            ("prefix".to_owned(), "MyMod".to_owned()),
            ("version".to_owned(), "1".to_owned()),
        ];
        let bytes = pack_pbo(&properties, &files, &PackOptions::default()).unwrap();
        let (hdr, payloads) = read_pbo(&bytes).unwrap();
        assert_eq!(hdr.property("product"), Some("dayz"));
        assert_eq!(hdr.property("prefix"), Some("MyMod"));
        assert_eq!(hdr.entries.len(), 2);
        assert_eq!(payloads[0], files[0].data);
        assert_eq!(payloads[1], files[1].data);
    }

    fn temp_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("mf-pbo-{}-{name}.pbo", std::process::id()));
        std::fs::remove_file(&path).ok();
        path
    }

    #[test]
    fn reads_a_header_that_is_larger_than_the_first_probe() {
        let files: Vec<PboFileToWrite> = (0..4000)
            .map(|i| PboFileToWrite {
                name: format!("scripts\\{i:04}\\entry_{i:04}.c"),
                data: vec![b'x'; 4],
                timestamp: 7,
            })
            .collect();
        let bytes = pack_pbo(&[], &files, &PackOptions::default()).unwrap();
        let path = temp_file("big-header");
        std::fs::write(&path, &bytes).unwrap();

        let (hdr, data_offset) = read_header_file(&path).unwrap();
        assert_eq!(hdr.entries.len(), 4000);
        assert_eq!(hdr.entries[3999].name_str(), "scripts\\3999\\entry_3999.c");
        assert!(data_offset > 64 * 1024, "header outgrew the initial probe");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn header_file_rejects_a_container_truncated_into_its_payload() {
        let files = vec![
            PboFileToWrite {
                name: "a.c".into(),
                data: vec![b'a'; 64],
                timestamp: 1,
            },
            PboFileToWrite {
                name: "b.c".into(),
                data: vec![b'b'; 64],
                timestamp: 2,
            },
        ];
        let bytes = pack_pbo(&[], &files, &PackOptions::default()).unwrap();
        let (_, data_offset) = crate::header::parse_header(&bytes).unwrap();
        let path = temp_file("truncated");
        std::fs::write(&path, &bytes[..data_offset]).unwrap();

        assert!(
            read_header_file(&path).is_err(),
            "the payload-budget guard must reject a container whose payload is gone"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn header_file_rejects_a_file_that_is_not_a_pbo() {
        let path = temp_file("not-a-pbo");
        std::fs::write(&path, b"#!/bin/sh\necho nope\n").unwrap();
        assert!(read_header_file(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
