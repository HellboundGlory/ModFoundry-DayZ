//! PBO-backed VFS layer: a vanilla/workshop/workspace PBO mounts
//! read-only, its entries indexed as `prefix/entry` virtual paths.
//!
//! Entry names come from an untrusted container: the corpus carries non-UTF-8
//! bytes, `..` traversal, Windows device names (`LPT5.{GUID}`), wildcards, BOMs
//! and whitespace-only names. Every name is
//! sanitized before it can become a virtual path, and the ones that cannot be
//! are excluded and counted — never repaired into a collision.

use crate::layer::{Backend, CaseFoldIndex, Layer, LayerKind};
use crate::path::VirtualPath;
use crate::VfsError;
use modfoundry_pbo::PboArchive;
use std::collections::HashMap;
use std::path::Path;

/// Windows reserved device stems: a path component whose stem is one of these is
/// not addressable (the host would open the device, not the file).
const RESERVED_STEMS: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters a path component must never carry: control bytes, Windows-reserved
/// punctuation (`:` is also the drive/ADS separator), and the BOM that would make
/// the same file addressable under two spellings.
fn is_forbidden(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | ':') || c == '\u{feff}'
}

/// Normalize one container entry name into a `a/b/c` virtual relative path, or
/// `None` when it can never be addressed safely.
pub fn sanitize_entry_name(raw: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(raw).ok()?;
    if name.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(name.len());
    for component in name.replace('\\', "/").split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return None;
        }
        if component.ends_with('.') || component.trim_end() != component {
            return None;
        }
        if component.chars().any(is_forbidden) {
            return None;
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or(component)
            .to_ascii_uppercase();
        if RESERVED_STEMS.contains(&stem.as_str()) {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(component);
    }
    Some(out)
}

/// Normalize a container `prefix`, dropping the separators at either end.
pub fn sanitize_prefix(prefix: &str) -> Option<String> {
    sanitize_entry_name(prefix.trim_matches(['\\', '/']).as_bytes())
}

/// The `prefix/name` virtual path of a container entry, or `None` when its name is unusable.
pub fn entry_vpath(prefix: Option<&str>, raw_name: &[u8]) -> Option<String> {
    let name = sanitize_entry_name(raw_name)?;
    Some(match prefix {
        Some(prefix) => format!("{prefix}/{name}"),
        None => name,
    })
}

/// A read-only PBO layer: the container header, the `prefix/entry` index the VFS
/// resolves against, and the entry lookup used to fetch payloads on demand.
#[derive(Debug, Clone)]
pub struct PboBackend {
    archive: PboArchive,
    prefix: Option<String>,
    index: CaseFoldIndex,
    entry_of: HashMap<String, usize>,
    skipped: usize,
    collisions: usize,
}

impl PboBackend {
    /// Mount the container at `path`. `.ebo` (BI's protected bank) is rejected by
    /// extension — it opens like a PBO but its entry table is encrypted.
    pub fn open(path: &Path) -> Result<Self, VfsError> {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ebo"))
        {
            return Err(VfsError::UnsupportedContainer(path.to_path_buf()));
        }
        let archive = PboArchive::open(path)
            .map_err(|e| VfsError::Container(format!("{}: {e:#}", path.display())))?;
        let prefix = match archive.header().property("prefix") {
            Some(prefix) => Some(sanitize_prefix(prefix).ok_or_else(|| {
                VfsError::InvalidPath(format!("{}: PBO prefix {prefix:?}", path.display()))
            })?),
            None => None,
        };
        let mut backend = Self {
            archive,
            prefix,
            index: CaseFoldIndex::default(),
            entry_of: HashMap::new(),
            skipped: 0,
            collisions: 0,
        };
        backend.reindex();
        Ok(backend)
    }

    fn reindex(&mut self) {
        let mut index = CaseFoldIndex::default();
        let mut entry_of = HashMap::new();
        let mut skipped = 0;
        let mut collisions = 0;
        for (position, entry) in self.archive.entries().iter().enumerate() {
            let Some(rel) = entry_vpath(self.prefix.as_deref(), &entry.name) else {
                skipped += 1;
                continue;
            };
            let lowered = rel.to_ascii_lowercase();
            if index.insert(&rel).is_some() {
                collisions += 1;
            }
            entry_of.insert(lowered, position);
        }
        self.index = index;
        self.entry_of = entry_of;
        self.skipped = skipped;
        self.collisions = collisions;
    }

    /// The engine `prefix` property the container maps under, if any.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    pub fn index(&self) -> &CaseFoldIndex {
        &self.index
    }

    /// Entries dropped because their name could not become a virtual path.
    pub fn skipped_names(&self) -> usize {
        self.skipped
    }

    /// Entries that only differ by case from another entry (VFS resolution folds
    /// case, so the last one wins).
    pub fn case_collisions(&self) -> usize {
        self.collisions
    }

    /// A mountable layer carrying this container's index.
    pub fn layer(self, id: impl Into<String>, kind: LayerKind) -> Layer {
        Layer {
            id: crate::layer::LayerId::new(id),
            kind,
            backend: Backend::Pbo {
                host_path: self.archive.path().to_path_buf(),
            },
            index: Some(self.index),
        }
    }

    /// Read entry `rel` (case-insensitively) out of the container.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, VfsError> {
        let rel = VirtualPath::clean(rel).map_err(VfsError::InvalidPath)?;
        let index = self
            .entry_of
            .get(&rel.to_ascii_lowercase())
            .copied()
            .ok_or_else(|| VfsError::NotFound(rel.clone()))?;
        self.archive
            .read_entry(index)
            .map_err(|e| VfsError::Container(format!("{}: {e:#}", self.archive.path().display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
    use std::path::{Path, PathBuf};

    fn write_pbo(name: &str, prefix: Option<&str>, files: &[(&str, &[u8])]) -> PathBuf {
        let entries: Vec<PboFileToWrite> = files
            .iter()
            .map(|(n, d)| PboFileToWrite {
                name: (*n).to_string(),
                data: d.to_vec(),
                timestamp: 5,
            })
            .collect();
        let props: Vec<(String, String)> = prefix
            .map(|p| vec![("prefix".to_owned(), p.to_owned())])
            .unwrap_or_default();
        let bytes = pack_pbo(&props, &entries, &PackOptions::default()).unwrap();
        let path =
            std::env::temp_dir().join(format!("mf-vfspbo-{}-{name}.pbo", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Rewrite the first occurrence of `from` in a packed container. Lets a test
    /// inject name bytes the writer (which takes `String`) cannot produce.
    fn patch_bytes(path: &Path, from: &[u8], to: &[u8]) {
        let mut bytes = std::fs::read(path).unwrap();
        let at = bytes
            .windows(from.len())
            .position(|w| w == from)
            .expect("patch target present");
        bytes[at..at + to.len()].copy_from_slice(to);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn sanitizes_entry_names_into_virtual_relative_paths() {
        assert_eq!(
            sanitize_entry_name(b"Scripts\\3_Game\\Foo.c").as_deref(),
            Some("Scripts/3_Game/Foo.c")
        );
        assert_eq!(
            sanitize_entry_name(b"config.bin").as_deref(),
            Some("config.bin")
        );
        assert_eq!(
            sanitize_entry_name("DZ\u{5c}AI".as_bytes()).as_deref(),
            Some("DZ/AI")
        );
    }

    #[test]
    fn rejects_names_that_must_never_become_a_path() {
        let hostile: &[&[u8]] = &[
            b"..\\escape.c".as_slice(),
            b"a/../../etc/passwd".as_slice(),
            b"/absolute.c".as_slice(),
            b"C:\\windows\\system32\\evil.dll".as_slice(),
            b"stream.txt:zone".as_slice(),
            b"wild*card.c".as_slice(),
            b"quest?ion.c".as_slice(),
            b"pipe|.c".as_slice(),
            b"quote\".c".as_slice(),
            b"angle<.c".as_slice(),
            b"\xef\xbb\xbfbom.c".as_slice(),
            b"\t\t".as_slice(),
            b"trailing. ".as_slice(),
            b"".as_slice(),
            b"nul\x01.c".as_slice(),
            b"\xd1ooler_as.paa".as_slice(),
        ];
        for name in hostile {
            assert_eq!(
                sanitize_entry_name(name),
                None,
                "should be rejected: {:?}",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn rejects_every_reserved_device_stem_in_every_spelling() {
        let stems = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for stem in stems {
            for spelling in [
                stem.to_string(),
                stem.to_ascii_lowercase(),
                format!("{stem}.c"),
                format!("{stem}.{{GUID}}"),
                format!("Scripts\\{stem}\\x.c"),
            ] {
                assert_eq!(
                    sanitize_entry_name(spelling.as_bytes()),
                    None,
                    "should be rejected: {spelling}"
                );
            }
        }
    }

    #[test]
    fn keeps_names_that_merely_look_like_devices() {
        assert_eq!(
            sanitize_entry_name(b"console.c").as_deref(),
            Some("console.c")
        );
        assert_eq!(sanitize_entry_name(b"COM10.c").as_deref(), Some("COM10.c"));
        assert_eq!(
            sanitize_entry_name(b"auxiliary.paa").as_deref(),
            Some("auxiliary.paa")
        );
    }

    #[test]
    fn reads_entries_under_the_container_prefix_case_insensitively() {
        let path = write_pbo(
            "prefix",
            Some("DZ\\AI"),
            &[
                ("config.bin", b"bin"),
                ("Scripts\\3_Game\\Foo.c", b"script"),
            ],
        );
        let backend = PboBackend::open(&path).unwrap();
        assert_eq!(backend.read("DZ/AI/config.bin").unwrap(), b"bin");
        assert_eq!(
            backend.read("dz/ai/scripts/3_game/foo.c").unwrap(),
            b"script"
        );
        assert!(matches!(
            backend.read("dz/ai/nope.c"),
            Err(VfsError::NotFound(_))
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_prefixless_container_indexes_bare_entry_names() {
        let path = write_pbo("noprefix", None, &[("config.cpp", b"class {}")]);
        let backend = PboBackend::open(&path).unwrap();
        assert_eq!(backend.read("config.cpp").unwrap(), b"class {}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hostile_entry_names_are_excluded_and_counted() {
        let path = write_pbo(
            "hostile",
            Some("DZ\\AI"),
            &[
                ("ok.c", b"ok"),
                ("..\\escape.c", b"no"),
                ("CON", b"no"),
                ("wild*card.c", b"no"),
            ],
        );
        let backend = PboBackend::open(&path).unwrap();
        assert_eq!(backend.index().len(), 1);
        assert_eq!(backend.skipped_names(), 3);
        assert_eq!(backend.read("dz/ai/ok.c").unwrap(), b"ok");
        assert!(matches!(
            backend.read("dz/ai/escape.c"),
            Err(VfsError::NotFound(_))
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn non_utf8_entry_names_are_excluded_not_lossily_decoded() {
        let path = write_pbo("nonutf8", None, &[("x_1.paa", b"paa")]);
        patch_bytes(&path, b"x_1.paa", b"x_\xf1.paa");

        let backend = PboBackend::open(&path).unwrap();
        assert_eq!(backend.index().len(), 0);
        assert_eq!(backend.skipped_names(), 1);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_unusable_prefix_fails_the_mount() {
        let path = write_pbo("badprefix", Some("..\\root"), &[("config.bin", b"x")]);
        assert!(matches!(
            PboBackend::open(&path),
            Err(VfsError::InvalidPath(_))
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_prefix_with_an_outer_separator_mounts_under_the_trimmed_name() {
        for (name, prefix) in [("trailing", "MyMod\\"), ("leading", "\\MyMod")] {
            let path = write_pbo(name, Some(prefix), &[("config.cpp", b"class {}")]);
            let backend = PboBackend::open(&path).unwrap();
            assert_eq!(backend.read("MyMod/config.cpp").unwrap(), b"class {}");
            std::fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn ebo_is_reported_as_an_unsupported_protected_container() {
        let path = std::env::temp_dir().join(format!("mf-vfspbo-{}.EBO", std::process::id()));
        std::fs::write(&path, b"\0Vers-body-not-a-pbo").unwrap();
        assert!(matches!(
            PboBackend::open(&path),
            Err(VfsError::UnsupportedContainer(_))
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_container_that_is_not_a_pbo_reports_a_read_failure() {
        let path = std::env::temp_dir().join(format!("mf-vfspbo-{}-junk.pbo", std::process::id()));
        std::fs::write(&path, b"definitely not a container").unwrap();
        assert!(matches!(
            PboBackend::open(&path),
            Err(VfsError::Container(_))
        ));
        std::fs::remove_file(&path).unwrap();
    }
}
