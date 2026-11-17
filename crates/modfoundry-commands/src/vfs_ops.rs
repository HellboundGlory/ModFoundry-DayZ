//! The `vfs.list` body: mount the caller's layer stack and enumerate it.

use crate::sources::{has_ext, map_addon_dirs};
use modfoundry_core::CommandError;
use modfoundry_vfs::{DirectoryBackend, LayerKind, ListedEntry, Overlay, PboBackend};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// One layer the caller asks `vfs.list` to mount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerArg {
    pub id: String,
    pub kind: LayerKind,
    pub path: PathBuf,
}

/// Arguments for the `vfs.list` command: the layer stack, lowest priority first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsListArgs {
    pub layers: Vec<LayerArg>,
}

/// The `vfs.list` argument schema, matching [`VfsListArgs`].
pub fn args_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "layers": {
                "type": "array",
                "description": "layer stack, lowest priority first",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "layer identity reported back for every entry" },
                        "kind": {
                            "type": "string",
                            "description": "layer kind, a serde LayerKind spelling",
                            "enum": [
                                "VanillaGame", "VanillaDta", "WorkshopMod", "WorkspaceSource",
                                "Staging", "Mission", "Virtual",
                            ],
                        },
                        "path": {
                            "type": "string",
                            "description": "host directory, an `addons` directory, or a .pbo file; a missing path is reported in `skipped`. A VanillaGame layer at the game's `addons` also mounts each DLC map folder's PBOs as `<id>/<map>/<pbo>`",
                        },
                    },
                    "required": ["id", "kind", "path"],
                },
            },
        },
        "required": ["layers"],
    })
}

/// Mount the requested layers onto one overlay and list every visible path with
/// the layer that provides it.
pub fn list_layers(args: &VfsListArgs) -> Result<Value, CommandError> {
    let mut overlay = Overlay::new("p");
    let mut layers = Vec::new();
    let mut skipped = Vec::new();
    let mut case_clashes = Vec::new();
    for arg in &args.layers {
        mount(
            arg,
            &mut overlay,
            &mut layers,
            &mut skipped,
            &mut case_clashes,
        )?;
    }
    let entries: Vec<Value> = overlay.entries().iter().map(entry_json).collect();
    Ok(json!({
        "entries": entries,
        "layers": layers,
        "skipped": skipped,
        "case_clashes": case_clashes,
    }))
}

fn mount(
    arg: &LayerArg,
    overlay: &mut Overlay,
    layers: &mut Vec<Value>,
    skipped: &mut Vec<Value>,
    case_clashes: &mut Vec<Value>,
) -> Result<(), CommandError> {
    let Some(path) = resolve_path(&arg.path) else {
        skipped.push(json!({ "path": arg.path, "error": "path does not exist" }));
        return Ok(());
    };
    let arg = &LayerArg {
        path,
        ..arg.clone()
    };
    if arg.path.is_dir() {
        if is_addons_dir(&arg.path) {
            mount_pbo_dir(overlay, layers, skipped, &arg.id, arg.kind, &arg.path)?;
            if arg.kind == LayerKind::VanillaGame {
                if let Some(game) = arg.path.parent() {
                    // Map PBOs go above the base game's; one unreadable map folder is skipped, not fatal.
                    for map in map_addon_dirs(game) {
                        let name = map
                            .parent()
                            .and_then(Path::file_name)
                            .map(|name| name.to_string_lossy().to_ascii_lowercase())
                            .unwrap_or_default();
                        let map_id = format!("{}/{name}", arg.id);
                        if let Err(e) =
                            mount_pbo_dir(overlay, layers, skipped, &map_id, arg.kind, &map)
                        {
                            skipped.push(json!({ "path": map, "error": e.to_string() }));
                        }
                    }
                }
            }
        } else {
            let (layer, notes) = DirectoryBackend::mount(&arg.id, arg.kind, &arg.path)
                .map_err(|e| CommandError::Failed(format!("{}: {e}", arg.path.display())))?;
            layers.push(layer_json(&arg.id, arg.kind));
            overlay.push(layer);
            for note in notes.unreadable {
                skipped.push(json!({ "path": note.path.to_string_lossy(), "error": note.error }));
            }
            for clash in notes.clashes {
                case_clashes.push(json!({
                    "layer": arg.id,
                    "kept": clash.kept,
                    "dropped": clash.dropped,
                }));
            }
        }
        return Ok(());
    }
    if has_ext(&arg.path, "pbo") {
        mount_pbo(overlay, layers, skipped, &arg.id, arg.kind, &arg.path);
        return Ok(());
    }
    Err(CommandError::InvalidArguments(format!(
        "layer `{}` path is neither a directory nor a .pbo file: {}",
        arg.id,
        arg.path.display()
    )))
}

/// Mount every PBO directly inside `dir` as a layer `<id>/<pbo stem>`, in file-name order.
fn mount_pbo_dir(
    overlay: &mut Overlay,
    layers: &mut Vec<Value>,
    skipped: &mut Vec<Value>,
    id: &str,
    kind: LayerKind,
    dir: &Path,
) -> Result<(), CommandError> {
    for pbo in pbo_files(dir)? {
        let stem = pbo.file_stem().unwrap_or_default().to_string_lossy();
        mount_pbo(
            overlay,
            layers,
            skipped,
            &format!("{id}/{stem}"),
            kind,
            &pbo,
        );
    }
    Ok(())
}

/// A PBO that fails to open is reported in `skipped`, not fatal: one corrupt
/// archive must not blank the whole tree.
fn mount_pbo(
    overlay: &mut Overlay,
    layers: &mut Vec<Value>,
    skipped: &mut Vec<Value>,
    id: &str,
    kind: LayerKind,
    path: &Path,
) {
    match PboBackend::open(path) {
        Ok(backend) => {
            layers.push(layer_json(id, kind));
            overlay.push(backend.layer(id, kind));
        }
        Err(e) => skipped.push(json!({ "path": path, "error": e.to_string() })),
    }
}

/// `path`, or a sibling whose name differs only in case. DayZ installs mix
/// `Addons` and `addons`, which only matters on a case-sensitive host.
fn resolve_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    let wanted = path.file_name()?;
    std::fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
        })
}

fn is_addons_dir(dir: &Path) -> bool {
    dir.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("addons"))
}

/// The `*.pbo` files directly inside `dir`, in file-name order.
fn pbo_files(dir: &Path) -> Result<Vec<PathBuf>, CommandError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| CommandError::Failed(format!("{}: {e}", dir.display())))?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| CommandError::Failed(format!("{}: {e}", dir.display())))?
            .path();
        if path.is_file() && has_ext(&path, "pbo") {
            out.push(path);
        }
    }
    out.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(out)
}

fn layer_json(id: &str, kind: LayerKind) -> Value {
    json!({ "id": id, "kind": kind })
}

fn entry_json(entry: &ListedEntry) -> Value {
    json!({
        "rel": entry.rel,
        "layer": entry.layer.to_string(),
        "kind": entry.kind,
        "shadowed_by": entry.shadowed_by.iter().map(ToString::to_string).collect::<Vec<_>>(),
    })
}
