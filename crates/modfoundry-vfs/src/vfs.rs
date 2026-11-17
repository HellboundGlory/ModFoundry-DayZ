//! Multi-root facade: the `Roots` (`game:`, `mod:<slug>:`,
//! `workshop:<id>:`, `stage:`, `mission:`, `virtual:`) each holding an ordered
//! [`Overlay`], so a caller reads a [`VirtualPath`] without knowing which
//! backend (directory, PBO, synthetic) answers it.

use crate::layer::{Layer, Overlay, Provenance, Resolved};
use crate::path::VirtualPath;
use crate::VfsError;
use std::collections::BTreeMap;

/// The mounted roots of one VFS session.
#[derive(Debug, Clone, Default)]
pub struct Vfs {
    roots: BTreeMap<String, Overlay>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `layer` on top of `root`'s stack (last mounted wins).
    pub fn mount(&mut self, root: &str, layer: Layer) {
        self.roots
            .entry(root.to_string())
            .or_insert_with(|| Overlay::new(root))
            .push(layer);
    }

    pub fn root_names(&self) -> impl Iterator<Item = &str> {
        self.roots.keys().map(String::as_str)
    }

    pub fn overlay(&self, root: &str) -> Option<&Overlay> {
        self.roots.get(root)
    }

    pub fn resolve(&self, path: &VirtualPath) -> Option<Resolved> {
        let (overlay, rel) = self.split(path)?;
        overlay.resolve(rel)
    }

    pub fn provenance(&self, path: &VirtualPath) -> Option<Provenance> {
        self.resolve(path).map(|r| Provenance { layer: r.layer })
    }

    pub fn read(&self, path: &VirtualPath) -> Result<Vec<u8>, VfsError> {
        let (overlay, rel) = self
            .split(path)
            .ok_or_else(|| VfsError::UnknownRoot(path.root().to_string()))?;
        overlay.read(rel)
    }

    /// Split a path into its overlay and relative part, matching the *longest*
    /// mounted root so namespaced roots (`mod:mymod`, `workshop:221100`) beat
    /// their bare prefix.
    fn split<'a, 'b>(&'a self, path: &'b VirtualPath) -> Option<(&'a Overlay, &'b str)> {
        let canonical = path.as_str();
        self.roots
            .iter()
            .rev()
            .find(|(root, _)| {
                canonical.len() > root.len() + 1
                    && canonical.starts_with(root.as_str())
                    && canonical.as_bytes()[root.len()] == b':'
            })
            .map(|(root, overlay)| (overlay, &canonical[root.len() + 1..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{DirectoryBackend, LayerId, LayerKind};
    use crate::path::VirtualPath;
    use crate::pbo::PboBackend;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};

    fn pbo_file(
        dir: &std::path::Path,
        file: &str,
        prefix: &str,
        name: &str,
        data: &[u8],
    ) -> std::path::PathBuf {
        let bytes = pack_pbo(
            &[("prefix".to_owned(), prefix.to_owned())],
            &[PboFileToWrite {
                name: name.into(),
                data: data.to_vec(),
                timestamp: 3,
            }],
            &PackOptions::default(),
        )
        .unwrap();
        let path = dir.join(file);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn resolves_and_reads_each_root() {
        let dir = std::env::temp_dir().join(format!("mf-vfs-roots-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Scripts/3_Game")).unwrap();
        std::fs::write(dir.join("Scripts/3_Game/MyMod.c"), b"class MyMod {};").unwrap();
        let ai = pbo_file(
            &dir,
            "DZ_AI.pbo",
            "DZ\\AI",
            "config.bin",
            b"vanilla-ai-config",
        );

        let mut vfs = Vfs::new();
        vfs.mount(
            "game",
            PboBackend::open(&ai)
                .unwrap()
                .layer("vanilla-ai", LayerKind::VanillaGame),
        );
        vfs.mount(
            "mod:mymod",
            DirectoryBackend::layer("mod-src", LayerKind::WorkspaceSource, &dir).unwrap(),
        );

        assert_eq!(
            vfs.read(&VirtualPath::parse("game:DZ/AI/config.bin").unwrap())
                .unwrap(),
            b"vanilla-ai-config"
        );
        assert_eq!(
            vfs.read(&VirtualPath::parse("mod:mymod:Scripts/3_Game/MyMod.c").unwrap())
                .unwrap(),
            b"class MyMod {};"
        );

        let resolved = vfs
            .resolve(&VirtualPath::parse("game:dz/ai/CONFIG.bin").unwrap())
            .unwrap();
        assert_eq!(resolved.layer, LayerId::new("vanilla-ai"));
        assert_eq!(
            vfs.provenance(&VirtualPath::parse("game:DZ/AI/config.bin").unwrap())
                .unwrap()
                .layer,
            LayerId::new("vanilla-ai")
        );

        assert!(matches!(
            vfs.read(&VirtualPath::parse("nope:file.c").unwrap()),
            Err(VfsError::UnknownRoot(_))
        ));
        assert!(matches!(
            vfs.read(&VirtualPath::parse("game:DZ/AI/absent.bin").unwrap()),
            Err(VfsError::NotFound(_))
        ));

        assert_eq!(
            vfs.root_names().collect::<Vec<_>>(),
            vec!["game", "mod:mymod"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn several_pbos_mount_into_one_root_in_priority_order() {
        let dir = std::env::temp_dir().join(format!("mf-vfs-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let low = pbo_file(&dir, "DZ_AI.pbo", "DZ\\AI", "config.bin", b"low");
        let mid = pbo_file(&dir, "mid.pbo", "DZ\\AI", "extra.c", b"mid");
        let high = pbo_file(&dir, "high.pbo", "DZ\\AI", "config.bin", b"high");

        let mut vfs = Vfs::new();
        for (id, path) in [("low", &low), ("mid", &mid), ("high", &high)] {
            vfs.mount(
                "game",
                PboBackend::open(path)
                    .unwrap()
                    .layer(id, LayerKind::VanillaGame),
            );
        }

        assert_eq!(
            vfs.read(&VirtualPath::parse("game:DZ/AI/config.bin").unwrap())
                .unwrap(),
            b"high"
        );
        assert_eq!(
            vfs.read(&VirtualPath::parse("game:DZ/AI/extra.c").unwrap())
                .unwrap(),
            b"mid"
        );
        let shadowed = vfs
            .resolve(&VirtualPath::parse("game:DZ/AI/config.bin").unwrap())
            .unwrap()
            .shadowed_by;
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0], LayerId::new("low"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
