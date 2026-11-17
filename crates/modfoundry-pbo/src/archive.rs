//! Random-access PBO reader: parses only the header up front and pulls a single
//! entry's payload on demand, so indexing a vanilla addon directory (files up to
//! ~1.2 GB) never loads a whole container into memory.

use crate::header::{Entry, PboHeader};
use crate::read::{read_header_file, unpack_entry};
use std::path::{Path, PathBuf};

/// An open PBO container: header plus the on-disk path payload reads seek into.
#[derive(Debug, Clone)]
pub struct PboArchive {
    path: PathBuf,
    header: PboHeader,
    /// Payload start of each entry; payloads are contiguous in entry order.
    offsets: Vec<u64>,
}

impl PboArchive {
    /// Open `path` by reading its header only; the payload is left on disk.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let (header, data_offset) = read_header_file(path)?;
        let offsets = header
            .entries
            .iter()
            .scan(data_offset as u64, |next, e| {
                let at = *next;
                *next += e.data_size as u64;
                Some(at)
            })
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            header,
            offsets,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> &PboHeader {
        &self.header
    }

    pub fn entries(&self) -> &[Entry] {
        &self.header.entries
    }

    fn entry_offset(&self, index: usize) -> Option<u64> {
        self.offsets.get(index).copied()
    }

    /// Read and unpack entry `index`'s payload (Cprs entries are decompressed).
    pub fn read_entry(&self, index: usize) -> anyhow::Result<Vec<u8>> {
        let entry = self.header.entries.get(index).ok_or_else(|| {
            anyhow::anyhow!(
                "entry {index} out of range ({} entries)",
                self.header.entries.len()
            )
        })?;
        let offset = self.entry_offset(index).expect("index was checked");
        let size = u64::from(entry.data_size);
        let mut raw = Vec::new();
        if size != 0 {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(&self.path)?;
            let len = file.metadata()?.len();
            if offset.checked_add(size).is_none_or(|end| end > len) {
                anyhow::bail!("payload for {} runs past end of file", entry.name_str());
            }
            file.seek(SeekFrom::Start(offset))?;
            raw = vec![0u8; entry.data_size as usize];
            file.read_exact(&mut raw)
                .map_err(|e| anyhow::anyhow!("reading entry `{}`: {e}", entry.name_str()))?;
        }
        unpack_entry(&raw, entry)
    }
}

#[cfg(test)]
mod tests {
    use crate::archive::PboArchive;
    use crate::pack::{pack_pbo, PackOptions, PboFileToWrite};
    use std::path::PathBuf;

    fn sample_bytes() -> Vec<u8> {
        let files = vec![
            PboFileToWrite {
                name: "config.cpp".into(),
                data: b"class CfgPatches {};".to_vec(),
                timestamp: 10,
            },
            PboFileToWrite {
                name: "Scripts\\1_Core\\constants.c".into(),
                data: b"class Constants {};".to_vec(),
                timestamp: 20,
            },
            PboFileToWrite {
                name: "Scripts\\5_Mission\\mission.c".into(),
                data: b"class Mission {};".to_vec(),
                timestamp: 30,
            },
        ];
        let props = vec![("product".to_owned(), "dayz".to_owned())];
        pack_pbo(&props, &files, &PackOptions::default()).unwrap()
    }

    fn temp_file(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mf-archive-{}-{name}.pbo", std::process::id()));
        std::fs::remove_file(&path).ok();
        path
    }

    #[test]
    fn opens_and_reads_each_entry_payload() {
        let path = temp_file("read");
        std::fs::write(&path, sample_bytes()).unwrap();

        let archive = PboArchive::open(&path).unwrap();
        assert_eq!(archive.header().property("product"), Some("dayz"));
        assert_eq!(archive.header().entries.len(), 3);
        assert_eq!(archive.read_entry(0).unwrap(), b"class CfgPatches {};");
        assert_eq!(archive.read_entry(1).unwrap(), b"class Constants {};");
        assert_eq!(archive.read_entry(2).unwrap(), b"class Mission {};");
        assert!(archive.read_entry(3).is_err());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn entry_offset_walks_the_declared_payload_sizes() {
        let path = temp_file("offsets");
        let bytes = sample_bytes();
        std::fs::write(&path, &bytes).unwrap();

        let archive = PboArchive::open(&path).unwrap();
        let (_, data_offset) = crate::header::parse_header(&bytes).unwrap();
        assert_eq!(archive.entry_offset(0), Some(data_offset as u64));
        assert_eq!(
            archive.entry_offset(1),
            Some(data_offset as u64 + b"class CfgPatches {};".len() as u64)
        );
        assert_eq!(archive.entry_offset(3), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reads_fail_when_the_payload_disappears_after_open() {
        let path = temp_file("vanished");
        std::fs::write(&path, sample_bytes()).unwrap();

        let archive = PboArchive::open(&path).unwrap();
        assert_eq!(archive.header().entries.len(), 3);
        std::fs::write(&path, b"").unwrap();
        assert!(archive.read_entry(2).is_err());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn read_entry_refuses_a_size_larger_than_the_file() {
        let path = temp_file("oversize");
        let bytes = sample_bytes();
        std::fs::write(&path, &bytes).unwrap();

        let archive = PboArchive::open(&path).unwrap();
        let (_, data_offset) = crate::header::parse_header(&bytes).unwrap();
        std::fs::write(&path, &bytes[..data_offset + 2]).unwrap();
        let err = archive.read_entry(0).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(err.contains("runs past end of file"), "{err}");
    }

    #[test]
    fn a_header_reads_even_when_its_payload_cannot_be_unpacked() {
        let path = temp_file("cprs-garbage");
        let mut bytes = sample_bytes();
        let name_at = bytes
            .windows(b"config.cpp\0".len())
            .position(|w| w == b"config.cpp\0")
            .expect("entry name in the packed header");
        let record = name_at + b"config.cpp\0".len();
        bytes[record..record + 4].copy_from_slice(&crate::header::PACKING_CPRS.to_le_bytes());
        bytes[record + 4..record + 8].copy_from_slice(&4096u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let archive = PboArchive::open(&path).expect("the header doesn't touch the payload");
        assert_eq!(
            archive.header().entries[0].packing,
            crate::header::PACKING_CPRS
        );
        assert!(archive.read_entry(0).is_err(), "garbage Cprs payload");
        assert!(
            crate::read::read_pbo(&bytes).is_err(),
            "a whole-file read hits the same payload"
        );
        std::fs::remove_file(&path).unwrap();
    }
}
