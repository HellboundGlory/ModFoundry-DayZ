use std::fs;
use std::path::{Path, PathBuf};

use crate::input::{Layer, SourceFile};

use super::{entries, file_name, is_script};

// Loads a loose script tree: the folder that holds the layer folders, as the
// unpacked vanilla scripts and the probes are laid out.
pub fn load_tree(dir: &Path, layer_rule: impl Fn(&str) -> Option<Layer>) -> Vec<SourceFile> {
    load_tree_with_disk(dir, layer_rule).into_iter().map(|(file, _)| file).collect()
}

// `load_tree`, with the disk path each script was read from.
pub fn load_tree_with_disk(dir: &Path, layer_rule: impl Fn(&str) -> Option<Layer>) -> Vec<(SourceFile, PathBuf)> {
    let mut files = Vec::new();
    for root in entries(dir).into_iter().filter(|path| path.is_dir()) {
        let name = file_name(&root);
        if let Some(layer) = layer_rule(&name) {
            collect(&root, &format!("Scripts/{name}"), layer, &mut files);
        }
    }
    files.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    files
}

fn collect(dir: &Path, prefix: &str, layer: Layer, out: &mut Vec<(SourceFile, PathBuf)>) {
    for path in entries(dir) {
        let name = file_name(&path);
        if path.is_dir() {
            collect(&path, &format!("{prefix}/{name}"), layer, out);
        } else if is_script(&path)
            && let Ok(bytes) = fs::read(&path)
        {
            let file = SourceFile {
                path: format!("{prefix}/{name}"),
                layer,
                mod_index: 0,
                text: String::from_utf8_lossy(&bytes).into_owned(),
            };
            out.push((file, path));
        }
    }
}
