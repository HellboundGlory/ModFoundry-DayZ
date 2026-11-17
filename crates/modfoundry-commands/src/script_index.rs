//! Script index loader: vanilla, dependencies and workspace sources.

use modfoundry_pbo::{read_pbo_file, PboArchive};
use modfoundry_script::{FileItems, FileKey, Index, Layer, Origin};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::par::parallel_map;
use crate::sources::{find_child, game_dir, has_ext, map_addon_dirs, mod_display_name};
use crate::NO_GAME_INSTALL;

/// One file the loader can index, addressable by a stable id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSource {
    File(PathBuf),
    PboEntry {
        pbo: PathBuf,
        prefix: String,
        entry: String,
    },
}

impl ScriptSource {
    /// The stable id used as the index key: the absolute file path, or
    /// `pbo:<pbo>!<entry>` with `/` separators and no prefix.
    pub fn id(&self) -> String {
        match self {
            ScriptSource::File(path) => path.to_string_lossy().into_owned(),
            ScriptSource::PboEntry { pbo, entry, .. } => {
                format!("pbo:{}!{}", pbo.to_string_lossy(), entry)
            }
        }
    }

    /// The human-facing path: the file path, or `<prefix>/<entry>` with `/`
    /// separators.
    pub fn display_path(&self) -> String {
        match self {
            ScriptSource::File(path) => path.to_string_lossy().into_owned(),
            ScriptSource::PboEntry { prefix, entry, .. } => {
                let prefix = prefix.replace('\\', "/");
                let entry = entry.replace('\\', "/");
                if prefix.is_empty() {
                    entry
                } else {
                    format!("{prefix}/{entry}")
                }
            }
        }
    }
}

/// The roots the loader indexes.
#[derive(Debug, Clone)]
pub struct IndexRequest {
    pub workspace: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub game: Option<PathBuf>,
}

/// Per-origin indexing outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceReport {
    pub origin: String,
    pub path: PathBuf,
    pub files: usize,
    /// Display name from the dependency's `mod.cpp`/`meta.cpp` (`null` when the
    /// source has none, e.g. vanilla, workspace, or a `.pbo` dependency).
    pub name: Option<String>,
}

/// The whole load's outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexReport {
    pub sources: Vec<SourceReport>,
    pub files: usize,
    pub classes: usize,
    pub millis: u64,
    pub warnings: Vec<String>,
}

/// Parse progress, shared with a frontend's progress bar.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Files parsed out of the total planned; `0.0` when nothing was planned.
    pub fn fraction(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.done() as f64 / total as f64
        }
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    fn begin(&self, total: usize) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}

struct Job {
    key: FileKey,
    source: ScriptSource,
    text: String,
}

/// Load the index from vanilla, dependency and workspace sources, returning the
/// text of every non-workspace source (vanilla PBO entries and dependency files)
/// so a query can answer without re-reading them.
pub fn load_index_with_texts(req: &IndexRequest, progress: &Progress) -> Loaded {
    build(req, progress, Scope::ALL)
}

/// Index only the workspace tree: the partial index a frontend swaps in while
/// the full load is still running.
pub fn load_workspace_with_texts(req: &IndexRequest, progress: &Progress) -> Loaded {
    build(req, progress, Scope::WORKSPACE)
}

/// A loaded index with its report, source side table and non-workspace texts.
pub type Loaded = (
    Index,
    IndexReport,
    HashMap<String, ScriptSource>,
    HashMap<String, std::sync::Arc<str>>,
);

#[derive(Clone, Copy)]
struct Scope {
    vanilla: bool,
    dependencies: bool,
    workspace: bool,
}

impl Scope {
    const ALL: Scope = Scope {
        vanilla: true,
        dependencies: true,
        workspace: true,
    };
    const WORKSPACE: Scope = Scope {
        vanilla: false,
        dependencies: false,
        workspace: true,
    };
}

fn build(req: &IndexRequest, progress: &Progress, scope: Scope) -> Loaded {
    let start = Instant::now();
    let mut jobs = Vec::new();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();

    if scope.vanilla {
        gather_vanilla(req, &mut jobs, &mut sources, &mut warnings);
    }
    if scope.dependencies {
        gather_dependencies(req, &mut jobs, &mut sources, &mut warnings);
    }
    if scope.workspace {
        gather_workspace(req, &mut jobs, &mut sources, &mut warnings);
    }

    let parsed = parse_all(&jobs, progress);
    let mut index = Index::new();
    let mut side_table = HashMap::with_capacity(jobs.len());
    let mut texts = HashMap::new();
    for (i, items) in parsed {
        let job = &mut jobs[i];
        side_table.insert(job.key.id.clone(), job.source.clone());
        if job.key.origin != Origin::Workspace {
            let text = std::mem::take(&mut job.text);
            texts.insert(job.key.id.clone(), std::sync::Arc::<str>::from(text));
        }
        index.set_file(job.key.clone(), items);
    }
    let stats = index.stats();
    let report = IndexReport {
        sources,
        files: stats.files,
        classes: stats.classes,
        millis: start.elapsed().as_millis() as u64,
        warnings,
    };
    (index, report, side_table, texts)
}

/// Dependency suggestions found on this host: a Steam library's DayZ workshop
/// Community Framework, when installed.
pub fn suggest_dependencies() -> Vec<(String, PathBuf)> {
    use modfoundry_platform::{discover_steam_installations, workshop_content_root, DAYZ_APP_ID};
    let mut out = Vec::new();
    for install in discover_steam_installations() {
        let cf = workshop_content_root(&install, DAYZ_APP_ID).join("1559212036");
        if cf.exists() {
            let name = mod_display_name(&cf).unwrap_or_else(|| "Community Framework".to_string());
            out.push((name, cf));
        }
    }
    out
}

fn gather_vanilla(
    req: &IndexRequest,
    jobs: &mut Vec<Job>,
    sources: &mut Vec<SourceReport>,
    warnings: &mut Vec<String>,
) {
    let game = game_dir(req.game.as_ref());
    let Some(game) = game else {
        warnings.push(format!("{NO_GAME_INSTALL}; vanilla scripts not indexed"));
        return;
    };
    let pbo = game.join("dta/scripts.pbo");
    let files = gather_pbo(&pbo, Origin::Vanilla, jobs, warnings);
    if files > 0 || pbo.is_file() {
        sources.push(SourceReport {
            origin: "vanilla".to_string(),
            path: pbo,
            files,
            name: None,
        });
    }
    for addons in map_addon_dirs(&game) {
        let map = addons
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned());
        for pbo in list_ext(&addons, "pbo") {
            let files = gather_map_pbo(&pbo, jobs, warnings);
            if files > 0 {
                sources.push(SourceReport {
                    origin: "vanilla".to_string(),
                    path: pbo,
                    files,
                    name: map.clone(),
                });
            }
        }
    }
}

/// The `.c` entries under `scripts/` of one map addon PBO, read entry by entry.
fn gather_map_pbo(pbo: &Path, jobs: &mut Vec<Job>, warnings: &mut Vec<String>) -> usize {
    let archive = match PboArchive::open(pbo) {
        Ok(archive) => archive,
        Err(err) => {
            warnings.push(format!("{}: {err}", pbo.display()));
            return 0;
        }
    };
    let prefix = archive.header().property("prefix").unwrap_or("");
    let mut count = 0;
    for (index, entry) in archive.entries().iter().enumerate() {
        let rel = entry.name_str().replace('\\', "/");
        // A map PBO also holds gear and tool export scripts, which are not game code.
        if !has_ext(Path::new(&rel), "c") || !under_scripts(&rel) {
            continue;
        }
        let bytes = match archive.read_entry(index) {
            Ok(bytes) => bytes,
            Err(err) => {
                warnings.push(format!("{}: {err}", pbo.display()));
                continue;
            }
        };
        jobs.push(pbo_entry_job(pbo, prefix, rel, Origin::Vanilla, &bytes));
        count += 1;
    }
    count
}

fn pbo_entry_job(pbo: &Path, prefix: &str, entry: String, origin: Origin, bytes: &[u8]) -> Job {
    let layer = Layer::from_path(&entry);
    let source = ScriptSource::PboEntry {
        pbo: pbo.to_path_buf(),
        prefix: prefix.to_string(),
        entry,
    };
    let key = FileKey {
        id: source.id(),
        origin,
        layer,
    };
    Job {
        key,
        source,
        text: String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn under_scripts(rel: &str) -> bool {
    rel.split_once('/')
        .is_some_and(|(head, _)| head.eq_ignore_ascii_case("scripts"))
}

fn gather_dependencies(
    req: &IndexRequest,
    jobs: &mut Vec<Job>,
    sources: &mut Vec<SourceReport>,
    warnings: &mut Vec<String>,
) {
    for (i, dep) in req.dependencies.iter().enumerate() {
        let origin = Origin::Dependency(i.min(u16::MAX as usize) as u16);
        let label = format!("dependency:{i}");
        if !dep.exists() {
            warnings.push(format!("{label}: {} not found", dep.display()));
            continue;
        }
        let files = if dep.is_file() {
            if has_ext(dep, "pbo") {
                gather_pbo(dep, origin, jobs, warnings)
            } else {
                warnings.push(format!(
                    "{label}: {} is not a .pbo or a directory",
                    dep.display()
                ));
                0
            }
        } else if let Some(addons) = find_child_dir(dep, "addons") {
            let pbos = list_ext(&addons, "pbo");
            pbos.iter()
                .map(|pbo| gather_pbo(pbo, origin, jobs, warnings))
                .sum()
        } else {
            walk(dep, WalkOptions::all())
                .iter()
                .filter(|p| push_file_job(dep, p, origin, jobs, warnings))
                .count()
        };
        sources.push(SourceReport {
            origin: label,
            path: dep.clone(),
            files,
            name: mod_display_name(dep),
        });
    }
}

fn gather_workspace(
    req: &IndexRequest,
    jobs: &mut Vec<Job>,
    sources: &mut Vec<SourceReport>,
    warnings: &mut Vec<String>,
) {
    let files = if req.workspace.is_dir() {
        walk(&req.workspace, WalkOptions::workspace())
            .iter()
            .filter(|p| push_file_job(&req.workspace, p, Origin::Workspace, jobs, warnings))
            .count()
    } else {
        warnings.push(format!(
            "workspace: {} is not a directory",
            req.workspace.display()
        ));
        0
    };
    sources.push(SourceReport {
        origin: "workspace".to_string(),
        path: req.workspace.clone(),
        files,
        name: None,
    });
}

fn gather_pbo(
    pbo: &Path,
    origin: Origin,
    jobs: &mut Vec<Job>,
    warnings: &mut Vec<String>,
) -> usize {
    let (header, data) = match read_pbo_file(pbo) {
        Ok(read) => read,
        Err(err) => {
            warnings.push(format!("{}: {err}", pbo.display()));
            return 0;
        }
    };
    let prefix = header.property("prefix").unwrap_or("");
    let mut count = 0;
    for (entry, bytes) in header.entries.iter().zip(&data) {
        let name = entry.name_str();
        if !has_ext(Path::new(name.as_ref()), "c") {
            continue;
        }
        jobs.push(pbo_entry_job(
            pbo,
            prefix,
            name.replace('\\', "/"),
            origin,
            bytes,
        ));
        count += 1;
    }
    count
}

fn push_file_job(
    root: &Path,
    path: &Path,
    origin: Origin,
    jobs: &mut Vec<Job>,
    warnings: &mut Vec<String>,
) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return false;
        }
    };
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let source = ScriptSource::File(abs);
    let key = FileKey {
        id: source.id(),
        origin,
        layer: Layer::from_path(&rel),
    };
    jobs.push(Job {
        key,
        source,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    });
    true
}

fn parse_all(jobs: &[Job], progress: &Progress) -> Vec<(usize, FileItems)> {
    progress.begin(jobs.len());
    parallel_map(jobs, |job| {
        let items = modfoundry_script::extract(&job.text);
        progress.tick();
        items
    })
}

struct WalkOptions {
    skip_hidden: bool,
    skip_top_builds: bool,
}

impl WalkOptions {
    fn workspace() -> Self {
        WalkOptions {
            skip_hidden: true,
            skip_top_builds: true,
        }
    }

    fn all() -> Self {
        WalkOptions {
            skip_hidden: false,
            skip_top_builds: false,
        }
    }
}

/// Every `.c` file under `root`, deterministically ordered, honoring `options`.
fn walk(root: &Path, options: WalkOptions) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_into(root, true, &options, &mut out);
    out.sort();
    out
}

fn walk_into(dir: &Path, top: bool, options: &WalkOptions, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if options.skip_hidden && name.starts_with('.') {
                continue;
            }
            if top && options.skip_top_builds && name == "builds" {
                continue;
            }
            walk_into(&path, false, options, out);
        } else if kind.is_file() && has_ext(&path, "c") {
            out.push(path);
        }
    }
}

pub(crate) fn list_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && has_ext(path, ext))
        .collect();
    out.sort();
    out
}

pub(crate) fn find_child_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    find_child(dir, name, true)
}
