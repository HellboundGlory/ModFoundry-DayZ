//! Reads one side of a game-build compare: a game install, an unpacked folder or one `.pbo`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use modfoundry_config::ast::ConfigFile;
use modfoundry_config::resolve::{Resolver, SourceConfig};
use modfoundry_gamediff::config::{config_model, ConfigModel};
use modfoundry_gamediff::layout::{layout_model, LayoutModel};
use modfoundry_gamediff::script::{script_model, ScriptModel};
use modfoundry_pbo::PboArchive;
use serde::{Deserialize, Serialize};

use crate::config_host::{config_file_kind, folder_configs, pbo_configs, unique_ids};
use crate::prefix_marker::Markers;
use crate::search_index::{build_key_checked, game_pbos, pbo_prefix};
use crate::sources::{find_child, has_ext};
use crate::walk::{entry_path, walk_files_to};

/// The folder depth a game or mod tree is read to.
pub(crate) const FOLDER_DEPTH: usize = 16;

/// What a side is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideKind {
    Install,
    Folder,
    Pbo,
}

impl SideKind {
    /// The name a report gives this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            SideKind::Install => "install",
            SideKind::Folder => "folder",
            SideKind::Pbo => "pbo",
        }
    }
}

/// How many files of each kind were read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideCounts {
    pub scripts: usize,
    pub configs: usize,
    pub layouts: usize,
}

/// One side of a compare, read into the three models.
#[derive(Debug, Clone)]
pub struct GameSide {
    pub kind: SideKind,
    pub path: PathBuf,
    pub build: Option<String>,
    pub scripts: ScriptModel,
    pub configs: ConfigModel,
    pub layouts: LayoutModel,
    pub counts: SideCounts,
    pub warnings: Vec<String>,
}

/// Read `path` as a game install, an unpacked game folder or one `.pbo`.
pub fn read_side(path: &Path) -> Result<GameSide, String> {
    let kind = side_kind(path)?;
    let mut warnings = Vec::new();
    let mut inputs = Inputs::default();

    let build = match kind {
        SideKind::Install => {
            let pbos = game_pbos(path);
            if pbos.is_empty() {
                return Err(format!("no game PBOs under {}", path.display()));
            }
            let build = build_key_checked(path, &pbos, &mut warnings);
            read_archives(&pbos, &mut inputs, &mut warnings);
            inputs.configs = pbo_configs(&pbos, &mut warnings);
            Some(build)
        }
        SideKind::Pbo => {
            let archive = PboArchive::open(path).map_err(|err| format!("{err:#}"))?;
            read_archive(&archive, &mut inputs, &mut warnings);
            inputs.configs = pbo_configs(&[path.to_path_buf()], &mut warnings);
            None
        }
        SideKind::Folder => {
            let mut markers = Markers::new(path);
            let files = read_folder(
                path,
                false,
                |file| markers.virtual_rel(file),
                &mut inputs,
                &mut warnings,
            );
            inputs.configs = folder_configs(path, &files, &mut warnings);
            None
        }
    };

    // An install's PBO is opened twice, once for its configs, so an unreadable one is reported twice.
    let mut seen = HashSet::new();
    warnings.retain(|warning| seen.insert(warning.clone()));

    let counts = SideCounts {
        scripts: inputs.scripts.len(),
        configs: inputs.configs.len(),
        layouts: inputs.layouts.len(),
    };
    if kind != SideKind::Pbo && counts == SideCounts::default() {
        let mut error = format!("no scripts, configs or layouts under {}", path.display());
        if !warnings.is_empty() {
            error.push_str(&format!(": {}", warnings.join("; ")));
        }
        return Err(error);
    }

    Ok(GameSide {
        kind,
        path: path.to_path_buf(),
        build,
        scripts: script_model(&inputs.scripts),
        configs: config_model(&in_load_order(inputs.configs, &mut warnings)),
        layouts: layout_model(&inputs.layouts),
        counts,
        warnings,
    })
}

fn side_kind(path: &Path) -> Result<SideKind, String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if path.is_dir() {
        return Ok(if find_child(path, "dta", true).is_some() {
            SideKind::Install
        } else {
            SideKind::Folder
        });
    }
    if has_ext(path, "pbo") {
        Ok(SideKind::Pbo)
    } else {
        Err(format!(
            "{} is not a game install, a folder or a .pbo",
            path.display()
        ))
    }
}

/// The files a side is read from, before the models are built.
#[derive(Default)]
pub(crate) struct Inputs {
    pub(crate) scripts: Vec<(String, String)>,
    pub(crate) configs: Vec<SourceConfig>,
    pub(crate) layouts: Vec<(String, String)>,
}

impl Inputs {
    /// The list a script or layout file goes to; `None` for any other file.
    fn files_for(&mut self, path: &Path) -> Option<&mut Vec<(String, String)>> {
        if has_ext(path, "c") {
            Some(&mut self.scripts)
        } else if has_ext(path, "layout") {
            Some(&mut self.layouts)
        } else {
            None
        }
    }
}

pub(crate) fn read_archives(pbos: &[PathBuf], inputs: &mut Inputs, warnings: &mut Vec<String>) {
    for pbo in pbos {
        match PboArchive::open(pbo) {
            Ok(archive) => read_archive(&archive, inputs, warnings),
            Err(err) => warnings.push(format!("{}: {err}", pbo.display())),
        }
    }
}

fn read_archive(archive: &PboArchive, inputs: &mut Inputs, warnings: &mut Vec<String>) {
    let prefix = pbo_prefix(archive);
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().replace('\\', "/");
        let Some(target) = inputs.files_for(Path::new(&name)) else {
            continue;
        };
        match archive.read_entry(index) {
            Ok(bytes) => target.push((
                entry_path(&prefix, &name),
                String::from_utf8_lossy(&bytes).into_owned(),
            )),
            Err(err) => warnings.push(format!("{}: {name}: {err}", archive.path().display())),
        }
    }
}

/// Read the loose files of a folder into `inputs`, each under `key`, returning every file the walk kept;
/// `skip_builds` leaves out a mod project's top `builds` folder.
pub(crate) fn read_folder(
    root: &Path,
    skip_builds: bool,
    mut key: impl FnMut(&Path) -> String,
    inputs: &mut Inputs,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let (files, skipped) = walk_files_to(root, FOLDER_DEPTH, skip_builds, |_, name| {
        has_ext(Path::new(name), "c")
            || has_ext(Path::new(name), "layout")
            || config_file_kind(name).is_some()
    });
    for folder in skipped {
        warnings.push(format!(
            "{}: deeper than {FOLDER_DEPTH} folders, not read",
            folder.display()
        ));
    }
    for path in &files {
        let Some(target) = inputs.files_for(path) else {
            continue;
        };
        match std::fs::read(path) {
            Ok(bytes) => target.push((key(path), String::from_utf8_lossy(&bytes).into_owned())),
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
    files
}

/// The configs as `config_model` takes them: keyed by source id, in the game's load order.
fn in_load_order(
    sources: Vec<SourceConfig>,
    warnings: &mut Vec<String>,
) -> Vec<(String, ConfigFile)> {
    let sources: Vec<Arc<SourceConfig>> = unique_ids(sources, warnings)
        .into_iter()
        .map(Arc::new)
        .collect();
    let order: Vec<String> = Resolver::from_shared(sources.clone())
        .load_order()
        .into_iter()
        .map(str::to_string)
        .collect();
    let by_id: HashMap<&str, &SourceConfig> = sources
        .iter()
        .map(|source| (source.id.as_str(), source.as_ref()))
        .collect();
    order
        .into_iter()
        .filter_map(|id| {
            let source = by_id.get(id.as_str())?;
            Some((id, source.file.clone()))
        })
        .collect()
}
