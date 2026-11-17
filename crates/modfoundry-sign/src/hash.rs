//! Deriving the three SHA-1 hashes a `.bisign` signs from a packed PBO.

use anyhow::{bail, Result};
use sha1::{Digest, Sha1};

use crate::tools::SignVersion;

/// v3 hashes only these extensions' contents (case-insensitive). DayZ's set,
/// which differs from Arma 3's.
pub const V3_EXTENSIONS: &[&str] = &["c", "h", "hpp", "ext", "inc", "cfg", "bikb"];

/// v2 hashes everything except these extensions.
pub const V2_EXCLUDED_EXTENSIONS: &[&str] = &[
    "paa", "jpg", "p3d", "tga", "rvmat", "lip", "ogg", "wss", "png", "rtm", "pac", "fxy", "wrp",
];

/// The PBO content a signature covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PboDigest {
    pub prefix: Option<String>,
    pub checksum: [u8; 20],
    /// Entry name (raw on-disk bytes, engine form, `\` separators) and payload,
    /// in header order.
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

impl PboDigest {
    /// Build from the bytes of a packed PBO (header + payloads + `\0` + SHA-1).
    pub fn from_pbo_bytes(pbo: &[u8]) -> Result<Self> {
        if pbo.len() < 21 || pbo[pbo.len() - 21] != 0 {
            bail!("PBO is missing its `\\0` + 20-byte SHA-1 trailer");
        }
        let (header, payloads) = modfoundry_pbo::read_pbo(pbo)?;
        let mut checksum = [0u8; 20];
        checksum.copy_from_slice(&pbo[pbo.len() - 20..]);
        let prefix = header.property("prefix").map(str::to_owned);
        Ok(PboDigest {
            prefix,
            checksum,
            entries: header
                .entries
                .into_iter()
                .map(|e| e.name)
                .zip(payloads)
                .collect(),
        })
    }

    /// SHA-1 over the sorted, lowercased names of non-empty entries.
    ///
    /// Names are hashed as their raw on-disk bytes: the engine addresses files
    /// by byte, and name obfuscators write names that are not valid UTF-8
    /// (6224 in the local corpus), so decoding first would silently reproduce
    /// the wrong digest. Lowercasing is ASCII-only for the same reason.
    pub fn names_hash(&self) -> [u8; 20] {
        let mut names: Vec<Vec<u8>> = self
            .entries
            .iter()
            .filter(|(_, data)| !data.is_empty())
            .map(|(name, _)| name.iter().map(u8::to_ascii_lowercase).collect())
            .collect();
        names.sort();
        let mut h = Sha1::new();
        for name in &names {
            h.update(name);
        }
        h.finalize().into()
    }

    /// SHA-1 over the concatenated payloads selected by the version's extension
    /// rule, or over the "no files" token when nothing is selected.
    pub fn file_hash(&self, version: SignVersion) -> [u8; 20] {
        let mut h = Sha1::new();
        let mut selected = false;
        for (name, data) in &self.entries {
            if extension_included(name, version) {
                h.update(data);
                selected = true;
            }
        }
        if !selected {
            h.update(match version {
                SignVersion::V2 => b"nothing".as_slice(),
                SignVersion::V3 => b"gnihton".as_slice(),
            });
        }
        h.finalize().into()
    }

    /// `[hash1, hash2, hash3]` — the three values RSA-signed into a bisign.
    pub fn hashes(&self, version: SignVersion) -> [[u8; 20]; 3] {
        let names = self.names_hash();
        let prefix = normalized_prefix(self.prefix.as_deref());

        let mut h2 = Sha1::new();
        h2.update(self.checksum);
        h2.update(names);
        h2.update(&prefix);

        let mut h3 = Sha1::new();
        h3.update(self.file_hash(version));
        h3.update(names);
        h3.update(&prefix);

        [self.checksum, h2.finalize().into(), h3.finalize().into()]
    }
}

fn extension_included(name: &[u8], version: SignVersion) -> bool {
    let ext: Vec<u8> = name
        .rsplit(|b| matches!(b, b'.' | b'\\' | b'/'))
        .next()
        .unwrap_or_default()
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect();
    let is = |candidate: &str| ext == candidate.as_bytes();
    match version {
        SignVersion::V3 => V3_EXTENSIONS.iter().any(|e| is(e)),
        SignVersion::V2 => !V2_EXCLUDED_EXTENSIONS.iter().any(|e| is(e)),
    }
}

fn normalized_prefix(prefix: Option<&str>) -> Vec<u8> {
    match prefix {
        None => Vec::new(),
        Some(p) if p.ends_with('\\') => p.as_bytes().to_vec(),
        Some(p) => {
            let mut v = p.as_bytes().to_vec();
            v.push(b'\\');
            v
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};

    /// Build a [`PboDigest`] from in-memory entries.
    pub fn digest(files: &[(&str, &[u8])], prefix: Option<&str>) -> PboDigest {
        let files: Vec<PboFileToWrite> = files
            .iter()
            .map(|(n, d)| PboFileToWrite {
                name: (*n).to_owned(),
                data: d.to_vec(),
                timestamp: 0,
            })
            .collect();
        let props: Vec<(String, String)> = match prefix {
            Some(p) => vec![("prefix".to_owned(), p.to_owned())],
            None => Vec::new(),
        };
        PboDigest::from_pbo_bytes(&pack_pbo(&props, &files, &PackOptions::default()).unwrap())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::digest;
    use super::*;

    fn sha1(parts: &[&[u8]]) -> [u8; 20] {
        let mut h = Sha1::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().into()
    }

    /// A minimal valid PBO carrying one entry whose name is raw bytes (the
    /// corpus has 6224 such names; `pack_pbo` can only emit text).
    fn pbo_with_raw_name(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut h = Vec::new();
        h.push(0);
        h.extend_from_slice(&modfoundry_pbo::PACKING_VERS.to_le_bytes());
        h.extend_from_slice(&[0u8; 16]);
        h.push(0); // empty property block
        h.extend_from_slice(name);
        h.push(0);
        h.extend_from_slice(&modfoundry_pbo::PACKING_RAW.to_le_bytes());
        h.extend_from_slice(&[0u8; 8]); // original_size, reserved
        h.extend_from_slice(&7u32.to_le_bytes()); // timestamp
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.push(0); // terminator name
        h.extend_from_slice(&[0u8; 20]);
        h.extend_from_slice(data);
        h.push(0);
        h.extend_from_slice(&[0u8; 20]); // trailer (unused by names_hash)
        h
    }

    #[test]
    fn names_hash_hashes_the_raw_bytes_of_a_non_utf8_name() {
        // The engine hashes name *bytes*. 0xC4 is not valid UTF-8 alone, so a
        // decoded-string hash would substitute U+00C4 -> U+00E4 -> `c3 a4` and
        // silently disagree with `DSSignFile` for every affected mod.
        let pbo = pbo_with_raw_name(&[0xC4, 0x41, b'.', b'c'], b"x");
        let d = PboDigest::from_pbo_bytes(&pbo).unwrap();
        // ASCII-only lowercasing: 0x41 -> 0x61, 0xC4 left alone.
        assert_eq!(d.names_hash(), sha1(&[&[0xC4, 0x61, b'.', b'c']]));
    }

    #[test]
    fn file_hash_reads_the_extension_from_raw_name_bytes() {
        let pbo = pbo_with_raw_name(&[0xC4, b'.', b'c'], b"AAAA");
        let d = PboDigest::from_pbo_bytes(&pbo).unwrap();
        assert_eq!(d.file_hash(SignVersion::V3), sha1(&[b"AAAA"]));
    }

    #[test]
    fn names_hash_lowercases_sorts_and_skips_empty_entries() {
        let d = digest(
            &[
                ("Scripts\\B.c", b"x"),
                ("Scripts\\a.c", b"y"),
                ("data\\empty.bin", b""),
            ],
            Some("pref"),
        );
        assert_eq!(d.names_hash(), sha1(&[b"scripts\\a.c", b"scripts\\b.c"]));
    }

    #[test]
    fn v3_file_hash_covers_only_the_whitelist_extensions() {
        let d = digest(
            &[
                ("Scripts\\a.c", b"AAAA"),
                ("Scripts\\b.cpp", b"BBBB"),
                ("Scripts\\c.h", b"CCCC"),
                ("Scripts\\d.sqf", b"DDDD"),
                ("config.bin", b"EEEE"),
            ],
            None,
        );
        assert_eq!(d.file_hash(SignVersion::V3), sha1(&[b"AAAA", b"CCCC"]));
    }

    #[test]
    fn v3_file_hash_uses_gnihton_when_no_eligible_files() {
        let d = digest(&[("config.bin", b"EEEE")], None);
        assert_eq!(d.file_hash(SignVersion::V3), sha1(&[b"gnihton"]));
    }

    #[test]
    fn v2_file_hash_excludes_binary_assets_but_keeps_config_and_sqf() {
        let d = digest(
            &[
                ("Scripts\\a.c", b"AAAA"),
                ("config.cpp", b"BBBB"),
                ("data\\t.paa", b"PPPP"),
                ("data\\m.p3d", b"MMMM"),
                ("Scripts\\d.sqf", b"DDDD"),
            ],
            None,
        );
        assert_eq!(
            d.file_hash(SignVersion::V2),
            sha1(&[b"AAAA", b"BBBB", b"DDDD"])
        );
    }

    #[test]
    fn v2_file_hash_uses_nothing_when_all_files_are_excluded() {
        let d = digest(&[("data\\t.paa", b"P")], None);
        assert_eq!(d.file_hash(SignVersion::V2), sha1(&[b"nothing"]));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        let d = digest(&[("Scripts\\A.C", b"AAAA")], None);
        assert_eq!(d.file_hash(SignVersion::V3), sha1(&[b"AAAA"]));
    }

    #[test]
    fn hashes_chain_checksum_names_and_normalized_prefix() {
        let d = digest(&[("Scripts\\a.c", b"AAAA")], Some("mymod"));
        let names = d.names_hash();
        let [h1, h2, h3] = d.hashes(SignVersion::V3);
        assert_eq!(h1, d.checksum);
        assert_eq!(h2, sha1(&[&h1, &names, b"mymod\\"]));
        assert_eq!(h3, sha1(&[&sha1(&[b"AAAA"]), &names, b"mymod\\"]));
    }

    #[test]
    fn prefix_already_ending_in_backslash_is_not_doubled() {
        let d = digest(&[("a.c", b"x")], Some("mymod\\"));
        let names = d.names_hash();
        let [_, h2, _] = d.hashes(SignVersion::V3);
        assert_eq!(h2, sha1(&[&d.checksum, &names, b"mymod\\"]));
    }
}
