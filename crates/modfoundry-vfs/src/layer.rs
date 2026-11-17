//! Layer model: backends (where bytes come from) and overlays (priority stacks).

use crate::path::VirtualPath;
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Kind of a VFS layer; drives defaults for read/write policy and UI grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    /// Vanilla game addons (`game:` root backends).
    VanillaGame,
    /// Vanilla engine/dta content (Workbench/game `dta`).
    VanillaDta,
    /// Subscribed Steam Workshop mod (read-only reference).
    WorkshopMod,
    /// The user's workspace mod source tree (primary write target).
    WorkspaceSource,
    /// Build staging output (write target during builds).
    Staging,
    /// Mission files (Central Economy XML etc.).
    Mission,
    /// Synthetic (generated) content.
    Virtual,
}

/// Opaque, stable identity of one mounted layer (survives re-indexing).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerId(String);

impl LayerId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl std::fmt::Display for LayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a layer's bytes actually come from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Backend {
    /// Plain host directory (workspace source, staging, workshop unpacked dirs).
    Directory { host_root: PathBuf },
    /// A PBO archive read through modfoundry-pbo (mounted read-only).
    Pbo { host_path: PathBuf },
    /// Synthetic content produced by the core (e.g. generated configs).
    Virtual { provider: String },
}

/// One mounted layer: kind + identity + backend + optional case-fold index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub kind: LayerKind,
    pub backend: Backend,
    /// Case-folded path index, maintained by the indexer; `None` until scanned.
    #[serde(skip)]
    pub index: Option<CaseFoldIndex>,
}

/// Case-folded file map for a scanned layer (path → host-relative location).
#[derive(Debug, Clone, Default)]
pub struct CaseFoldIndex {
    /// lowercased virtual rel path → actual (case-preserved) rel path.
    map: HashMap<String, String>,
}

impl CaseFoldIndex {
    /// Index `actual_rel`, returning the case-preserved path it displaced (a
    /// case-fold collision inside one layer).
    pub fn insert(&mut self, actual_rel: &str) -> Option<String> {
        self.map
            .insert(actual_rel.to_ascii_lowercase(), actual_rel.to_string())
    }
    pub fn resolve(&self, lower: &str) -> Option<&str> {
        self.map.get(lower).map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// Every indexed path in its case-preserved spelling.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.map.values().map(String::as_str)
    }
    /// Every `(lowercase, case-preserved)` pair in the index.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map
            .iter()
            .map(|(lower, actual)| (lower.as_str(), actual.as_str()))
    }
}

/// Result of resolving a virtual path across an overlay.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: VirtualPath,
    /// Winning layer.
    pub layer: LayerId,
    /// Layers that also contain the path but lost on priority.
    pub shadowed_by: Vec<LayerId>,
}

/// Provenance answer for an overlay lookup: which layer provided it.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub layer: LayerId,
}

/// One visible path in an overlay: its winning spelling, layer and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedEntry {
    pub rel: String,
    pub layer: LayerId,
    pub kind: LayerKind,
    /// Layers that also hold this path but lost on priority, highest first.
    pub shadowed_by: Vec<LayerId>,
}

/// Ordered overlay: a root plus an ordered stack of layers (index 0 = lowest
/// priority = shadowed first). Resolution returns the highest-priority match and
/// reports the shadowed lower-priority matches (conflict detection).
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub root: String,
    pub layers: Vec<Layer>,
}

impl Overlay {
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            layers: Vec::new(),
        }
    }

    pub fn push(&mut self, layer: Layer) {
        self.layers.push(layer);
    }

    /// Resolve a *relative* path against the stack. Returns the winning layer and
    /// the shadowed layers, or `None` when absent everywhere.
    pub fn resolve(&self, rel: &str) -> Option<Resolved> {
        let (layer, _, shadowed_by) = self.winner(&rel.to_ascii_lowercase())?;
        Some(Resolved {
            path: VirtualPath::parse(&format!("{}:{rel}", self.root)).expect("rel was clean"),
            layer: layer.id.clone(),
            shadowed_by,
        })
    }

    /// The layer that wins `lower` (an already lowercased relative path), its
    /// case-preserved spelling there, and the ids of the layers it shadows.
    fn winner(&self, lower: &str) -> Option<(&Layer, &str, Vec<LayerId>)> {
        let mut found: Option<(&Layer, &str)> = None;
        let mut shadowed = Vec::new();
        // layers are ordered lowest→highest; iterate reversed so the first hit is
        // the winner, later (lower) hits are shadowed.
        for layer in self.layers.iter().rev() {
            let Some(spelling) = layer.index.as_ref().and_then(|idx| idx.resolve(lower)) else {
                continue;
            };
            if found.is_none() {
                found = Some((layer, spelling));
            } else {
                shadowed.push(layer.id.clone());
            }
        }
        let (layer, spelling) = found?;
        Some((layer, spelling, shadowed))
    }

    /// List every visible path in the overlay with its winning layer, sorted by
    /// `rel` lowercased. Unindexed layers contribute nothing.
    pub fn entries(&self) -> Vec<ListedEntry> {
        let mut by_lower: BTreeMap<&str, ListedEntry> = BTreeMap::new();
        // Layers are lowest to highest; reverse so the first hit wins.
        for layer in self.layers.iter().rev() {
            let Some(idx) = &layer.index else {
                continue;
            };
            for (lower, actual) in idx.iter() {
                match by_lower.entry(lower) {
                    Entry::Vacant(slot) => {
                        slot.insert(ListedEntry {
                            rel: actual.to_string(),
                            layer: layer.id.clone(),
                            kind: layer.kind,
                            shadowed_by: Vec::new(),
                        });
                    }
                    Entry::Occupied(mut slot) => slot.get_mut().shadowed_by.push(layer.id.clone()),
                }
            }
        }
        by_lower.into_values().collect()
    }

    /// Read `rel`'s bytes from the layer that won resolution, dispatching on that
    /// layer's backend.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, crate::VfsError> {
        let (layer, spelling, _) = self
            .winner(&rel.to_ascii_lowercase())
            .ok_or_else(|| crate::VfsError::NotFound(format!("{}:{rel}", self.root)))?;
        match &layer.backend {
            Backend::Directory { host_root } => {
                // Only indexed layers can win resolution, so the case-preserved
                // name always exists; never fall back to the caller's string as a
                // host path.
                Ok(std::fs::read(host_root.join(spelling))?)
            }
            Backend::Pbo { host_path } => crate::pbo::PboBackend::open(host_path)?.read(rel),
            Backend::Virtual { provider } => Err(crate::VfsError::Unreadable(format!(
                "{} ({provider})",
                layer.id
            ))),
        }
    }
}

/// Two spellings of one path that differ only in case: the one kept in the
/// index and the one dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseClash {
    pub kept: String,
    pub dropped: String,
}

/// A host path a scan could not read, with the IO error's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub path: PathBuf,
    pub error: String,
}

/// What a folder scan left out: case clashes it resolved and entries it could
/// not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanNotes {
    pub clashes: Vec<CaseClash>,
    pub unreadable: Vec<Unreadable>,
}

/// Minimal host-directory backend helper: mounts a directory as a layer and builds
/// its case-fold index. PBO containers mount through `pbo::PboBackend`.
pub struct DirectoryBackend;

impl DirectoryBackend {
    /// Mount `root` as a layer, with what the scan left out.
    pub fn mount(
        id: impl Into<String>,
        kind: LayerKind,
        root: &Path,
    ) -> std::io::Result<(Layer, ScanNotes)> {
        let (index, notes) = Self::scan_with_notes(root)?;
        Ok((
            Layer {
                id: LayerId::new(id),
                kind,
                backend: Backend::Directory {
                    host_root: root.to_path_buf(),
                },
                index: Some(index),
            },
            notes,
        ))
    }

    /// Mount `root` as a layer, scanning it for the case-fold index.
    pub fn layer(id: impl Into<String>, kind: LayerKind, root: &Path) -> std::io::Result<Layer> {
        Ok(Self::mount(id, kind, root)?.0)
    }

    /// Recursively scan `root` building the case-fold index (rel paths).
    pub fn scan(root: &Path) -> std::io::Result<CaseFoldIndex> {
        Ok(Self::scan_with_notes(root)?.0)
    }

    /// Recursively scan `root` building the case-fold index, reporting the case
    /// clashes and the entries the walk could not read.
    pub fn scan_with_notes(root: &Path) -> std::io::Result<(CaseFoldIndex, ScanNotes)> {
        let mut idx = CaseFoldIndex::default();
        let mut notes = ScanNotes::default();
        let mut dropped_paths = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let children = match std::fs::read_dir(&dir) {
                Ok(children) => children,
                Err(e) if dir == root => return Err(e),
                Err(e) => {
                    notes.unreadable.push(Unreadable {
                        path: dir.clone(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            for entry in children {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        notes.unreadable.push(Unreadable {
                            path: dir.clone(),
                            error: e.to_string(),
                        });
                        continue;
                    }
                };
                let kind = match entry.file_type() {
                    Ok(kind) => kind,
                    Err(e) => {
                        notes.unreadable.push(Unreadable {
                            path: entry.path(),
                            error: e.to_string(),
                        });
                        continue;
                    }
                };
                let p = entry.path();
                if kind.is_dir() {
                    stack.push(p);
                } else if kind.is_file() {
                    if let Ok(rel) = p.strip_prefix(root) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        let lower = rel.to_ascii_lowercase();
                        match idx.resolve(&lower).map(str::to_string) {
                            None => {
                                let _ = idx.insert(&rel);
                            }
                            // Two host names that read the same once made lossy are not a case clash.
                            Some(existing) if existing == rel => {
                                notes.unreadable.push(Unreadable {
                                    path: p,
                                    error: "its name reads the same as another file's".into(),
                                });
                            }
                            Some(existing) => {
                                let dropped = if rel < existing {
                                    let _ = idx.insert(&rel);
                                    existing
                                } else {
                                    rel
                                };
                                dropped_paths.push(dropped);
                            }
                        }
                    }
                }
            }
        }
        // The winner is only known once every spelling was seen.
        notes.clashes = dropped_paths
            .into_iter()
            .map(|dropped| CaseClash {
                kept: idx
                    .resolve(&dropped.to_ascii_lowercase())
                    .unwrap_or_default()
                    .to_string(),
                dropped,
            })
            .collect();
        notes.clashes.sort_by(|a, b| a.dropped.cmp(&b.dropped));
        notes.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((idx, notes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VfsError;

    fn layer(id: &str, kind: LayerKind, files: &[&str]) -> Layer {
        let mut index = CaseFoldIndex::default();
        for f in files {
            let _ = index.insert(f);
        }
        Layer {
            id: LayerId::new(id),
            kind,
            backend: Backend::Virtual {
                provider: "test".into(),
            },
            index: Some(index),
        }
    }

    #[test]
    fn resolution_is_case_insensitive_and_takes_the_layer_holding_the_path() {
        let mut ov = Overlay::new("game");
        let mut low = layer("vanilla", LayerKind::VanillaGame, &["dz/weapons/akm.p3d"]);
        // The same layer re-inserts the file with a different casing:
        let _ = low.index.as_mut().unwrap().insert("DZ/Weapons/akm.p3d");
        ov.push(low);
        ov.push(layer(
            "mod",
            LayerKind::WorkspaceSource,
            &["DZ/weapons/akm_MOD.p3d"],
        ));

        // Only the mod layer has akm_MOD.p3d → resolves to mod.
        let r = ov.resolve("DZ/weapons/akm_MOD.p3d").unwrap();
        assert_eq!(r.layer.to_string(), "mod");
        assert!(r.shadowed_by.is_empty());

        // Case-insensitive resolution of the vanilla file.
        let r2 = ov.resolve("dz/WEAPONS/AKM.p3d").unwrap();
        assert_eq!(r2.layer.to_string(), "vanilla");
    }

    #[test]
    fn directory_scan_indexes_recursively() {
        let dir = std::env::temp_dir().join(format!("mf-vfs-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/Case.c"), b"x").unwrap();
        std::fs::write(dir.join("top.txt"), b"y").unwrap();
        let idx = DirectoryBackend::scan(&dir).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.resolve("a/b/case.c"), Some("a/b/Case.c"));
        assert_eq!(idx.resolve("top.txt"), Some("top.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cycle_is_not_followed() {
        let dir = std::env::temp_dir().join(format!("mf-vfs-symlink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.c"), b"x").unwrap();
        std::os::unix::fs::symlink(".", dir.join("loop")).unwrap();
        let idx = DirectoryBackend::scan(&dir).unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.resolve("a.c"), Some("a.c"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn re_indexing_a_case_fold_collision_reports_the_displaced_path() {
        let mut idx = CaseFoldIndex::default();
        assert_eq!(idx.insert("DZ/AI/Config.bin"), None);
        assert_eq!(
            idx.insert("DZ/AI/config.BIN"),
            Some("DZ/AI/Config.bin".into())
        );
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn reads_through_the_winning_layer_of_an_overlay() {
        use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};

        let dir = std::env::temp_dir().join(format!("mf-vfs-overlay-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("DZ/AI")).unwrap();
        std::fs::write(dir.join("DZ/AI/config.bin"), b"vanilla-config").unwrap();

        let pbo = dir.join("ai.pbo");
        let bytes = pack_pbo(
            &[("prefix".to_owned(), "DZ\\AI".to_owned())],
            &[PboFileToWrite {
                name: "config.bin".into(),
                data: b"mod-config".to_vec(),
                timestamp: 1,
            }],
            &PackOptions::default(),
        )
        .unwrap();
        std::fs::write(&pbo, bytes).unwrap();

        let mut overlay = Overlay::new("game");
        overlay.push(DirectoryBackend::layer("vanilla", LayerKind::VanillaGame, &dir).unwrap());
        let mod_layer = crate::pbo::PboBackend::open(&pbo)
            .unwrap()
            .layer("mod", LayerKind::WorkspaceSource);
        overlay.push(mod_layer);

        let resolved = overlay.resolve("DZ/AI/config.bin").unwrap();
        assert_eq!(resolved.layer.to_string(), "mod");
        assert_eq!(resolved.shadowed_by.len(), 1);
        assert_eq!(overlay.read("DZ/AI/config.bin").unwrap(), b"mod-config");
        assert!(matches!(
            overlay.read("DZ/AI/absent.bin"),
            Err(VfsError::NotFound(_))
        ));

        overlay.layers.pop();
        assert_eq!(overlay.read("dz/ai/CONFIG.bin").unwrap(), b"vanilla-config");

        overlay.push(layer(
            "synthetic",
            LayerKind::Virtual,
            &["DZ/AI/config.bin"],
        ));
        assert!(matches!(
            overlay.read("DZ/AI/config.bin"),
            Err(VfsError::Unreadable(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn unindexed(id: &str, kind: LayerKind) -> Layer {
        Layer {
            id: LayerId::new(id),
            kind,
            backend: Backend::Virtual {
                provider: "test".into(),
            },
            index: None,
        }
    }

    #[test]
    fn listing_one_layer_returns_every_path_sorted_case_insensitively() {
        let mut overlay = Overlay::new("game");
        overlay.push(layer(
            "workspace",
            LayerKind::WorkspaceSource,
            &["Scripts/A.c", "config.cpp"],
        ));

        let entries = overlay.entries();
        let rels: Vec<&str> = entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["config.cpp", "Scripts/A.c"]);
        for entry in &entries {
            assert_eq!(entry.layer.to_string(), "workspace");
            assert_eq!(entry.kind, LayerKind::WorkspaceSource);
            assert!(entry.shadowed_by.is_empty());
        }
    }

    #[test]
    fn listing_reports_the_winning_spelling_and_shadowed_layers() {
        let mut overlay = Overlay::new("game");
        overlay.push(layer("base", LayerKind::VanillaDta, &["Data/X.paa"]));
        overlay.push(layer("vanilla", LayerKind::VanillaGame, &["DATA/X.paa"]));
        overlay.push(layer(
            "workspace",
            LayerKind::WorkspaceSource,
            &["data/x.paa"],
        ));

        let entries = overlay.entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.rel, "data/x.paa");
        assert_eq!(entry.layer.to_string(), "workspace");
        assert_eq!(entry.kind, LayerKind::WorkspaceSource);
        assert_eq!(
            entry.shadowed_by,
            vec![LayerId::new("vanilla"), LayerId::new("base")]
        );
    }

    #[test]
    fn listing_ignores_layers_without_an_index() {
        let mut overlay = Overlay::new("game");
        overlay.push(unindexed("unscanned", LayerKind::WorkshopMod));
        assert!(overlay.entries().is_empty());

        overlay.push(layer("workspace", LayerKind::WorkspaceSource, &["a.txt"]));
        let entries = overlay.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel, "a.txt");
        assert_eq!(entries[0].layer.to_string(), "workspace");
    }
}
