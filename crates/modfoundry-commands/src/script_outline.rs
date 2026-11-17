//! The outline of an Enforce file, or of every Enforce file under a folder, for `script.outline`.

use modfoundry_enforce::{classify, read_source, FileClass};
use modfoundry_script::{outline, FileOutline, Layer};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};

/// The layout version of the outline JSON.
pub const OUTLINE_SCHEMA: u32 = 1;

/// The outline of an Enforce file, or of every Enforce file under a folder, as `{ schema, files, skipped }`.
#[derive(Serialize)]
pub struct OutlineTree {
    pub schema: u32,
    pub files: Vec<FileOutline>,
    /// Relative paths, with `/`, of the folders not walked (dot folders, unreadable links, links to an
    /// enclosing folder), sorted.
    pub skipped: Vec<String>,
}

/// The outline of an Enforce file, or of every Enforce file under a folder, as `{ schema, files, skipped }`.
pub fn outline_tree(path: &Path) -> io::Result<OutlineTree> {
    if path.is_dir() {
        return folder_tree(path);
    }
    if path.is_file() && classify(path) == FileClass::NotEnforce {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not an Enforce script (.c file)", path.display()),
        ));
    }
    let name = path.file_name().unwrap_or(path.as_os_str());
    Ok(OutlineTree {
        schema: OUTLINE_SCHEMA,
        files: vec![file_outline(&name.to_string_lossy(), path)?],
        skipped: Vec::new(),
    })
}

fn folder_tree(root: &Path) -> io::Result<OutlineTree> {
    let mut walk = Walk::default();
    walk.visit(root, root)?;
    let root_target = fs::canonicalize(root)?;
    // Folders reached through a link come last, so each folder keeps its real path.
    while let Some(link) = walk.links.pop_front() {
        let encloses_root = fs::canonicalize(&link).is_ok_and(|t| root_target.starts_with(t));
        if encloses_root || walk.visit(root, &link).is_err() {
            walk.skipped.push(relative_name(root, &link));
        }
    }
    walk.files.sort();
    walk.skipped.sort();
    let files = walk
        .files
        .iter()
        .map(|(name, path)| file_outline(name, path))
        .collect::<io::Result<_>>()?;
    Ok(OutlineTree {
        schema: OUTLINE_SCHEMA,
        files,
        skipped: walk.skipped,
    })
}

#[derive(Default)]
struct Walk {
    files: Vec<(String, PathBuf)>,
    skipped: Vec<String>,
    entered: HashSet<PathBuf>,
    links: VecDeque<PathBuf>,
}

impl Walk {
    /// Every Enforce file under `dir`, following symlinks, skipping folders whose name starts with `.`.
    fn visit(&mut self, root: &Path, dir: &Path) -> io::Result<()> {
        let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", dir.display()));
        if !self.entered.insert(fs::canonicalize(dir).map_err(named)?) {
            return Ok(());
        }
        let mut entries = fs::read_dir(dir)
            .map_err(named)?
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(DirEntry::path);
        for entry in entries {
            let path = entry.path();
            let Some(file_type) = entry_type(&entry)? else {
                continue;
            };
            if file_type.is_dir() {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    self.skipped.push(relative_name(root, &path));
                } else if entry.file_type()?.is_symlink() {
                    self.links.push_back(path);
                } else {
                    self.visit(root, &path)?;
                }
            } else if file_type.is_file() && classify(&path) != FileClass::NotEnforce {
                self.files.push((relative_name(root, &path), path));
            }
        }
        Ok(())
    }
}

/// The type an entry resolves to, or `None` for a link that cannot be followed (broken or looping).
fn entry_type(entry: &DirEntry) -> io::Result<Option<fs::FileType>> {
    let file_type = entry.file_type()?;
    if !file_type.is_symlink() {
        return Ok(Some(file_type));
    }
    Ok(fs::metadata(entry.path())
        .ok()
        .map(|target| target.file_type()))
}

fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_outline(name: &str, path: &Path) -> io::Result<FileOutline> {
    let source = read_source(path)?;
    let mut file = outline(name, &source);
    file.layer = Layer::from_path(&path.to_string_lossy()).map(Layer::name);
    Ok(file)
}
