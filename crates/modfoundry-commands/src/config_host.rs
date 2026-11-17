//! The session's config index behind the `config.*` query commands: every
//! vanilla, dependency and workspace config parsed once and merged into the pure
//! [`Resolver`].

use crate::latest::Latest;
use crate::par::parallel_map;
use crate::prefix_marker::Markers;
use crate::script_host::normalized;
use crate::script_index::{find_child_dir, list_ext};
use crate::sources::{game_dir, has_ext, mod_display_name, vanilla_addon_dirs};
use crate::NO_GAME_INSTALL;
use modfoundry_config::ast::{ConfigFile, Member, Value as AstValue};
use modfoundry_config::parser::parse;
use modfoundry_config::rap::read_rap;
use modfoundry_config::resolve::{ConfigOrigin, ResolvedClass, Resolver, SourceConfig};
use modfoundry_core::{CommandError, Diagnostic};
use modfoundry_pbo::PboArchive;
use modfoundry_vfs::pbo::{entry_vpath, sanitize_prefix};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The roots `config.index` loads: the workspace source tree, dependency mods,
/// and the game install (`None` asks the host).
#[derive(Debug, Clone)]
pub struct ConfigIndexRequest {
    pub workspace: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub game: Option<PathBuf>,
}

/// One loaded config and the raw form `config.source` renders it from.
#[derive(Debug)]
pub(crate) struct ConfigSource {
    pub config: Arc<SourceConfig>,
    /// The `config.cpp`'s text; `None` for a `config.bin`, which renders with
    /// `modfoundry_config::to_text`.
    pub text: Option<String>,
}

/// The session's config index: loaded once and shared by every `config.*`
/// command.
pub struct ConfigHost {
    slot: Latest<Loaded>,
}

impl Default for ConfigHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigHost {
    pub fn new() -> Self {
        Self {
            slot: Latest::default(),
        }
    }

    /// Load `req` and return the index report. A newer call replaces the whole
    /// state; the returned report is always this load's.
    pub fn index(&self, req: ConfigIndexRequest) -> Value {
        let generation = self.slot.begin();
        let (loaded, report) = load(&req);
        self.slot.install(generation, Arc::new(loaded));
        report
    }

    /// The load a newer one hasn't superseded.
    pub(crate) fn loaded(&self) -> Option<Arc<Loaded>> {
        self.slot.get()
    }

    /// `config.resolve`: one file's forward-declared externals, or one class
    /// resolved against the loaded index. Addresses either a workspace `path`
    /// (with unsaved `text` overlaid) or an indexed `source` id.
    pub(crate) fn resolve(
        &self,
        path: Option<&Path>,
        source: Option<&str>,
        text: Option<&str>,
        class: Option<&[String]>,
    ) -> Result<Value, CommandError> {
        match (path, source) {
            (Some(_), Some(_)) => {
                return Err(CommandError::InvalidArguments(
                    "config.resolve takes either `path` or `source`, not both".to_string(),
                ))
            }
            (None, None) => {
                return Err(CommandError::InvalidArguments(
                    "config.resolve needs a `path` or a `source`".to_string(),
                ))
            }
            _ => {}
        }
        let Some(loaded) = self.loaded() else {
            return Ok(match class {
                Some(_) => json!({ "ready": false, "externals": [], "class": null }),
                None => json!({ "ready": false, "externals": [] }),
            });
        };
        let resolver = &loaded.resolver;
        let table = &loaded.sources;
        let (id, overlay) = match source {
            Some(id) => {
                if !table.contains_key(id) {
                    return Err(CommandError::InvalidArguments(format!(
                        "unknown config source `{id}`"
                    )));
                }
                (id.to_string(), None)
            }
            None => {
                let path = path.expect("one of path/source");
                let rel = workspace_rel(&loaded.workspace, path);
                let id = config_source_id(ConfigOrigin::Workspace, Path::new(&rel), None);
                let overlay = match text {
                    Some(text) => Some(overlay_source(&id, &rel, text)),
                    None if !table.contains_key(&id) => std::fs::read(path)
                        .ok()
                        .map(|bytes| overlay_source(&id, &rel, &String::from_utf8_lossy(&bytes))),
                    None => None,
                };
                (id, overlay)
            }
        };
        let resolver: Arc<Resolver> = match &overlay {
            Some(source) => Arc::new(resolver.with_overlay(source.clone())),
            None => Arc::clone(resolver),
        };
        let sources = Sources {
            table,
            overlay: overlay.as_ref(),
        };
        let own = match &overlay {
            Some(source) => Some(&source.file),
            None => table.get(&id).map(|source| &source.config.file),
        };
        Ok(match class {
            None => json!({
                "ready": true,
                "externals": own
                    .map(|file| externals(&file.members, &resolver, &id, &sources))
                    .unwrap_or_default(),
            }),
            Some(class) => {
                let resolved = resolver.class(class);
                let base = resolved.as_ref().and_then(|c| c.base_chain.first());
                json!({
                    "ready": true,
                    "class": resolved.as_ref().map(|c| resolved_class_json(c, &sources)),
                    "owner_patch": resolver.owner_patch(&id).map(str::to_string),
                    "required_addons": required_addons(&resolver, &id),
                    "base_source": base
                        .and_then(|link| sources.get(&link.source))
                        .map(source_info),
                    "base_owner_patch": base
                        .and_then(|link| resolver.owner_patch(&link.source))
                        .map(str::to_string),
                    "base_owner_required": base
                        .is_some_and(|link| resolver.required_closure(&id).contains(&link.source)),
                    "defined_by": resolver
                        .defined_by(class)
                        .into_iter()
                        .map(|source| match sources.get(source) {
                            Some(source) => source_ref(source),
                            None => json!({ "id": source, "display": source, "origin": Value::Null }),
                        })
                        .collect::<Vec<_>>(),
                    "unresolved_base": resolved.and_then(|c| c.unresolved_base),
                })
            }
        })
    }

    /// `config.validate`'s MF-C410 pass: `text` linted against the loaded
    /// index, ignoring the patch names stored for this file. Empty before the
    /// first index.
    pub(crate) fn index_findings(&self, path: &Path, text: &str) -> Vec<Diagnostic> {
        let Some(loaded) = self.loaded() else {
            return Vec::new();
        };
        let rel = workspace_rel(&loaded.workspace, path);
        let id = config_source_id(ConfigOrigin::Workspace, Path::new(&rel), None);
        modfoundry_config::lints::index_findings(text, &loaded.resolver, Some(id.as_str()))
    }

    /// `config.source`: one loaded config by id, or by vpath (case-insensitive,
    /// the last source loaded for a shared vpath).
    pub(crate) fn source(
        &self,
        id: Option<&str>,
        vpath: Option<&str>,
    ) -> Result<Value, CommandError> {
        let Some(loaded) = self.loaded() else {
            return Err(CommandError::Failed(
                "config index is not loaded".to_string(),
            ));
        };
        let resolver = &loaded.resolver;
        let table = &loaded.sources;
        let found = match (id, vpath) {
            (Some(id), _) => table.get(id),
            (None, Some(vpath)) => resolver
                .load_order()
                .into_iter()
                .filter_map(|id| table.get(id))
                .rfind(|source| source.config.vpath.eq_ignore_ascii_case(vpath)),
            (None, None) => {
                return Err(CommandError::InvalidArguments(
                    "config.source needs an `id` or a `vpath`".to_string(),
                ))
            }
        };
        let source = found.ok_or_else(|| {
            CommandError::InvalidArguments(match (id, vpath) {
                (Some(id), _) => format!("unknown config source `{id}`"),
                (_, Some(vpath)) => format!("unknown config vpath `{vpath}`"),
                _ => unreachable!(),
            })
        })?;
        let config = &source.config;
        Ok(json!({
            "id": config.id,
            "display": config.display,
            "vpath": config.vpath,
            "origin": origin_id(config.origin),
            "text": source.text.clone().unwrap_or_else(|| modfoundry_config::to_text(&config.file)),
        }))
    }
}

/// The stable id of a config: `vanilla:<pbo file name>:<entry>`,
/// `dependency:<n>:<pbo file name>:<entry>` (entry lowercased, `/`-separated)
/// for a PBO entry, and `<origin>:<path>` when there is no PBO — a loose
/// `config.cpp`, addressed by its path relative to the source root.
pub fn config_source_id(origin: ConfigOrigin, pbo_or_file: &Path, entry: Option<&str>) -> String {
    let path = pbo_or_file.to_string_lossy().replace('\\', "/");
    match (origin, entry) {
        (ConfigOrigin::Workspace, _) => format!("workspace:{path}"),
        (origin, Some(entry)) => format!(
            "{}:{}:{}",
            origin_id(origin),
            file_name(pbo_or_file),
            entry.replace('\\', "/").to_ascii_lowercase()
        ),
        (origin, None) => format!("{}:{path}", origin_id(origin)),
    }
}

/// The virtual path of a PBO entry, built exactly as
/// [`modfoundry_vfs::pbo::PboBackend`] builds it: the sanitized `prefix`
/// property and the sanitized entry name joined with `/`.
pub fn config_vpath(prefix: Option<&str>, entry: &[u8]) -> Option<String> {
    let prefix = match prefix {
        Some(prefix) => Some(sanitize_prefix(prefix)?),
        None => None,
    };
    entry_vpath(prefix.as_deref(), entry)
}

/// The `CfgPatches` one mod declares, read fresh from `root` — a `.pbo`, a
/// directory with an `addons` child, or a source directory of loose
/// `config.cpp` files. Does not touch the session's index.
pub fn patches(root: &Path) -> Result<Value, CommandError> {
    if !root.exists() {
        return Err(CommandError::Failed(format!(
            "{}: not found",
            root.display()
        )));
    }
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    if root.is_file() {
        if !has_ext(root, "pbo") {
            return Err(CommandError::Failed(format!(
                "{}: not a .pbo or a directory",
                root.display()
            )));
        }
        gather_pbo(
            root,
            ConfigOrigin::Dependency(0),
            &mut entries,
            &mut warnings,
        );
    } else if let Some(addons) = find_child_dir(root, "addons") {
        for pbo in list_ext(&addons, "pbo") {
            gather_pbo(
                &pbo,
                ConfigOrigin::Dependency(0),
                &mut entries,
                &mut warnings,
            );
        }
    } else {
        gather_loose(root, ConfigOrigin::Workspace, false, &mut entries);
    }
    let mut sources = Vec::new();
    for (i, result) in parallel_map(&entries, read_one) {
        match result {
            Ok((file, _)) => sources.push(source_config(&entries[i], file)),
            Err(message) => warnings.push(format!("{}: {message}", entries[i].warn_path())),
        }
    }
    let declared: Vec<Value> = Resolver::new(sources)
        .patches()
        .into_iter()
        .map(|patch| {
            json!({
                "name": patch.name,
                "source": patch.source,
                "required_addons": patch.required_addons,
            })
        })
        .collect();
    Ok(json!({
        "root": root,
        "name": mod_display_name(root),
        "patches": declared,
        "warnings": warnings,
    }))
}

/// One planned config entry: how to read it and every name it is known by.
struct Entry {
    origin: ConfigOrigin,
    kind: Kind,
    /// The path inside the container (or relative to the source root), `/`-separated.
    rel: String,
    /// The human label of the source: the PBO's file name, or the path relative
    /// to the source root for a loose file.
    display: String,
    vpath: String,
    id: String,
    raw: Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Bin,
    Cpp,
}

enum Raw {
    /// A PBO entry: the header is read once per PBO, the payload seeked when read.
    Pbo {
        archive: Arc<PboArchive>,
        index: usize,
    },
    Loose {
        path: PathBuf,
    },
}

impl Entry {
    /// The location a read failure is reported against.
    fn warn_path(&self) -> String {
        match &self.raw {
            Raw::Pbo { archive, .. } => format!("{}:{}", archive.path().display(), self.rel),
            Raw::Loose { path } => path.display().to_string(),
        }
    }
}

/// A load's result: the session's index state.
pub(crate) struct Loaded {
    pub(crate) resolver: Arc<Resolver>,
    pub(crate) sources: HashMap<String, Arc<ConfigSource>>,
    pub(crate) workspace: PathBuf,
}

/// One `config.index` source row: a root and how many configs it contributed.
#[derive(Debug, Clone, serde::Serialize)]
struct SourceRow {
    origin: String,
    path: PathBuf,
    /// Configs found in the root, whether or not each one reads back.
    configs: usize,
    name: Option<String>,
}

fn load(req: &ConfigIndexRequest) -> (Loaded, Value) {
    let started = Instant::now();
    let mut entries = Vec::new();
    let mut rows = Vec::new();
    let mut warnings = Vec::new();

    gather_vanilla(req, &mut entries, &mut rows, &mut warnings);
    gather_dependencies(req, &mut entries, &mut rows, &mut warnings);
    gather_workspace(req, &mut entries, &mut rows, &mut warnings);

    let mut configs = Vec::with_capacity(entries.len());
    let mut texts = Vec::with_capacity(entries.len());
    let mut classes = 0;
    for (i, result) in parallel_map(&entries, read_one) {
        let entry = &entries[i];
        match result {
            Ok((file, text)) => {
                classes += class_count(&file);
                configs.push(source_config(entry, file));
                texts.push(text);
            }
            Err(message) => warnings.push(format!("{}: {message}", entry.warn_path())),
        }
    }
    let mut sources = Vec::with_capacity(configs.len());
    let mut table = HashMap::with_capacity(configs.len());
    for (config, text) in unique_ids(configs, &mut warnings).into_iter().zip(texts) {
        let config = Arc::new(config);
        table.insert(
            config.id.clone(),
            Arc::new(ConfigSource {
                config: Arc::clone(&config),
                text,
            }),
        );
        sources.push(config);
    }
    let resolver = Resolver::from_shared(sources);
    let report = json!({
        "sources": rows,
        "patches": resolver.patches().len(),
        "classes": classes,
        "millis": started.elapsed().as_millis() as u64,
        "warnings": warnings,
        "cycles": resolver.cycles(),
    });
    (
        Loaded {
            resolver: Arc::new(resolver),
            sources: table,
            workspace: absolute(&req.workspace),
        },
        report,
    )
}

fn source_config(entry: &Entry, file: ConfigFile) -> SourceConfig {
    SourceConfig {
        id: entry.id.clone(),
        display: entry.display.clone(),
        vpath: entry.vpath.clone(),
        origin: entry.origin,
        file,
    }
}

/// Source ids name a PBO by its file name alone, so two vanilla folders holding the same name would share one.
pub(crate) fn unique_ids(
    mut sources: Vec<SourceConfig>,
    warnings: &mut Vec<String>,
) -> Vec<SourceConfig> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for source in &mut sources {
        let count = seen.entry(source.id.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            let id = format!("{}#{count}", source.id);
            warnings.push(format!(
                "{}: another vanilla PBO has the same name; its config is read as {id}",
                source.display
            ));
            source.id = id;
        }
    }
    sources
}

fn gather_vanilla(
    req: &ConfigIndexRequest,
    entries: &mut Vec<Entry>,
    rows: &mut Vec<SourceRow>,
    warnings: &mut Vec<String>,
) {
    let game = game_dir(req.game.as_ref());
    let Some(game) = game else {
        warnings.push(format!("{NO_GAME_INSTALL}; vanilla configs not indexed"));
        return;
    };
    let mut configs = 0;
    for addons in vanilla_addon_dirs(&game) {
        for pbo in list_ext(&addons, "pbo") {
            configs += gather_pbo(&pbo, ConfigOrigin::Vanilla, entries, warnings);
        }
    }
    rows.push(SourceRow {
        origin: "vanilla".to_string(),
        path: game,
        configs,
        name: None,
    });
}

fn gather_dependencies(
    req: &ConfigIndexRequest,
    entries: &mut Vec<Entry>,
    rows: &mut Vec<SourceRow>,
    warnings: &mut Vec<String>,
) {
    for (i, dep) in req.dependencies.iter().enumerate() {
        let origin = ConfigOrigin::Dependency(i.min(u16::MAX as usize) as u16);
        let label = format!("dependency:{i}");
        if !dep.exists() {
            warnings.push(format!("{label}: {} not found", dep.display()));
            continue;
        }
        let configs = if dep.is_file() {
            if has_ext(dep, "pbo") {
                gather_pbo(dep, origin, entries, warnings)
            } else {
                warnings.push(format!(
                    "{label}: {} is not a .pbo or a directory",
                    dep.display()
                ));
                0
            }
        } else if let Some(addons) = find_child_dir(dep, "addons") {
            list_ext(&addons, "pbo")
                .iter()
                .map(|pbo| gather_pbo(pbo, origin, entries, warnings))
                .sum()
        } else {
            gather_loose(dep, origin, false, entries)
        };
        rows.push(SourceRow {
            origin: label,
            path: dep.clone(),
            configs,
            name: mod_display_name(dep),
        });
    }
}

fn gather_workspace(
    req: &ConfigIndexRequest,
    entries: &mut Vec<Entry>,
    rows: &mut Vec<SourceRow>,
    warnings: &mut Vec<String>,
) {
    let configs = if req.workspace.is_dir() {
        gather_loose(&req.workspace, ConfigOrigin::Workspace, true, entries)
    } else {
        warnings.push(format!(
            "workspace: {} is not a directory",
            req.workspace.display()
        ));
        0
    };
    rows.push(SourceRow {
        origin: "workspace".to_string(),
        path: req.workspace.clone(),
        configs,
        name: None,
    });
}

/// Plan every config entry of one PBO, at any depth.
fn gather_pbo(
    pbo: &Path,
    origin: ConfigOrigin,
    entries: &mut Vec<Entry>,
    warnings: &mut Vec<String>,
) -> usize {
    let archive = match PboArchive::open(pbo) {
        Ok(archive) => Arc::new(archive),
        Err(err) => {
            warnings.push(format!("{}: {err}", pbo.display()));
            return 0;
        }
    };
    let declared_prefix = archive.header().property("prefix").map(str::to_string);
    let prefix = declared_prefix.as_deref().and_then(sanitize_prefix);
    if declared_prefix.is_some() && prefix.is_none() {
        warnings.push(format!(
            "{}: PBO prefix cannot be a virtual path",
            pbo.display()
        ));
        return 0;
    }
    let mut count = 0;
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str();
        let Some(kind) = config_kind(&name) else {
            continue;
        };
        let Some(vpath) = config_vpath(prefix.as_deref(), &entry.name) else {
            continue;
        };
        let rel = name.replace('\\', "/");
        entries.push(Entry {
            origin,
            kind,
            display: file_name(pbo),
            vpath,
            id: config_source_id(origin, pbo, Some(&rel)),
            rel,
            raw: Raw::Pbo {
                archive: Arc::clone(&archive),
                index,
            },
        });
        count += 1;
    }
    count
}

/// Every config in these PBOs, read as vanilla sources; a read failure is a warning.
pub(crate) fn pbo_configs(pbos: &[PathBuf], warnings: &mut Vec<String>) -> Vec<SourceConfig> {
    let mut entries = Vec::new();
    for pbo in pbos {
        let mut own = Vec::new();
        gather_pbo(pbo, ConfigOrigin::Vanilla, &mut own, warnings);
        entries.extend(binary_over_text(own));
    }
    read_entries(&entries, warnings)
}

/// Drops each `config.cpp` that has a `config.bin` in the same folder: the game loads the binary, and an unpack
/// writes the text beside it with fewer float digits.
fn binary_over_text(entries: Vec<Entry>) -> Vec<Entry> {
    let folder = |entry: &Entry| {
        let rel = entry.rel.to_ascii_lowercase();
        rel.rsplit_once('/')
            .map(|(dir, _)| dir.to_owned())
            .unwrap_or_default()
    };
    let binary: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.kind == Kind::Bin)
        .map(folder)
        .collect();
    entries
        .into_iter()
        .filter(|entry| entry.kind == Kind::Bin || !binary.contains(&folder(entry)))
        .collect()
}

/// Every `config.cpp` and `config.bin` under a folder, read as vanilla sources; a `config.bin` wins
/// over a `config.cpp` beside it.
pub(crate) fn folder_configs(
    root: &Path,
    files: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<SourceConfig> {
    let mut markers = Markers::new(root);
    let mut entries = Vec::new();
    for path in files {
        let rel = markers.virtual_rel(path);
        let Some(kind) = config_kind(&rel) else {
            continue;
        };
        entries.push(Entry {
            origin: ConfigOrigin::Vanilla,
            kind,
            display: rel.clone(),
            vpath: rel.clone(),
            id: config_source_id(ConfigOrigin::Vanilla, Path::new(&rel), None),
            rel,
            raw: Raw::Loose { path: path.clone() },
        });
    }
    let entries = binary_over_text(entries);
    read_entries(&entries, warnings)
}

fn read_entries(entries: &[Entry], warnings: &mut Vec<String>) -> Vec<SourceConfig> {
    let mut sources = Vec::new();
    for (i, result) in parallel_map(entries, read_one) {
        match result {
            Ok((file, _)) => sources.push(source_config(&entries[i], file)),
            Err(message) => warnings.push(format!("{}: {message}", entries[i].warn_path())),
        }
    }
    sources
}

/// Plan every loose `config.cpp` under one source root.
fn gather_loose(
    root: &Path,
    origin: ConfigOrigin,
    skip_top_builds: bool,
    entries: &mut Vec<Entry>,
) -> usize {
    let mut count = 0;
    for path in walk_config_cpp(root, skip_top_builds) {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(Entry {
            origin,
            kind: Kind::Cpp,
            display: rel.clone(),
            vpath: rel.clone(),
            id: config_source_id(origin, Path::new(&rel), None),
            rel,
            raw: Raw::Loose { path },
        });
        count += 1;
    }
    count
}

/// One entry's read: its parsed file and, for a `config.cpp`, its raw text.
type ReadResult = Result<(ConfigFile, Option<String>), String>;

fn read_one(entry: &Entry) -> ReadResult {
    let bytes = match &entry.raw {
        Raw::Pbo { archive, index } => archive.read_entry(*index).map_err(|e| format!("{e:#}"))?,
        Raw::Loose { path } => std::fs::read(path).map_err(|e| e.to_string())?,
    };
    match entry.kind {
        Kind::Bin => read_rap(&bytes)
            .map(|file| (file, None))
            .map_err(|e| e.to_string()),
        Kind::Cpp => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            Ok((parse(&text).file, Some(text)))
        }
    }
}

/// The payload of one PBO entry, without reading the container's other entries.
pub(crate) fn read_pbo_entry(path: &Path, index: usize) -> Result<Vec<u8>, String> {
    PboArchive::open(path)
        .and_then(|archive| archive.read_entry(index))
        .map_err(|e| format!("{e:#}"))
}

/// `config.bin` or `config.cpp` by file name, at any depth; every other entry is
/// not a config.
fn config_kind(name: &str) -> Option<Kind> {
    config_file_kind(name.rsplit(['/', '\\']).next()?)
}

/// `config.bin` or `config.cpp` by bare file name.
pub(crate) fn config_file_kind(file: &str) -> Option<Kind> {
    if file.eq_ignore_ascii_case("config.bin") {
        Some(Kind::Bin)
    } else if file.eq_ignore_ascii_case("config.cpp") {
        Some(Kind::Cpp)
    } else {
        None
    }
}

/// Every `config.cpp` under `root`, deterministically ordered.
fn walk_config_cpp(root: &Path, skip_top_builds: bool) -> Vec<PathBuf> {
    crate::walk::walk_files(root, skip_top_builds, |_, name| {
        name.eq_ignore_ascii_case("config.cpp")
    })
}

/// Every `class` statement in the file, at any depth.
fn class_count(file: &ConfigFile) -> usize {
    fn count(members: &[Member]) -> usize {
        members
            .iter()
            .map(|member| match member {
                Member::Class(class) => 1 + count(&class.members),
                _ => 0,
            })
            .sum()
    }
    count(&file.members)
}

/// One id's `SourceConfig`: the loaded table first, then the overlay.
struct Sources<'a> {
    table: &'a HashMap<String, Arc<ConfigSource>>,
    overlay: Option<&'a SourceConfig>,
}

impl Sources<'_> {
    fn get(&self, id: &str) -> Option<&SourceConfig> {
        match self.table.get(id) {
            Some(source) => Some(&source.config),
            None => self.overlay.filter(|source| source.id == id),
        }
    }
}

fn overlay_source(id: &str, rel: &str, text: &str) -> SourceConfig {
    SourceConfig {
        id: id.to_string(),
        display: rel.to_string(),
        vpath: rel.to_string(),
        origin: ConfigOrigin::Workspace,
        file: parse(text).file,
    }
}

/// `path` relative to the indexed workspace root, `/`-separated; the full path
/// when it lies outside. Both sides are compared absolute with `.`/`..`
/// resolved, as the script host compares them.
fn workspace_rel(workspace: &Path, path: &Path) -> String {
    let path = absolute(path);
    path.strip_prefix(workspace)
        .unwrap_or(path.as_path())
        .to_string_lossy()
        .replace('\\', "/")
}

fn absolute(path: &Path) -> PathBuf {
    normalized(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn source_info(source: &SourceConfig) -> Value {
    json!({
        "id": source.id,
        "display": source.display,
        "vpath": source.vpath,
        "origin": origin_id(source.origin),
    })
}

fn source_ref(source: &SourceConfig) -> Value {
    json!({
        "id": source.id,
        "display": source.display,
        "origin": origin_id(source.origin),
    })
}

/// Every forward-declared class path in the file, at any depth, in file order.
fn forwards(members: &[Member]) -> Vec<Vec<String>> {
    fn walk(members: &[Member], path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        for member in members {
            if let Member::Class(class) = member {
                path.push(class.name.clone());
                if class.forward {
                    out.push(path.clone());
                }
                walk(&class.members, path, out);
                path.pop();
            }
        }
    }
    let mut out = Vec::new();
    walk(members, &mut Vec::new(), &mut out);
    out
}

fn externals(members: &[Member], resolver: &Resolver, own: &str, sources: &Sources) -> Vec<Value> {
    forwards(members)
        .into_iter()
        .map(|path| {
            let source = resolver
                .defined_by(&path)
                .into_iter()
                .find(|defined| *defined != own)
                .and_then(|defined| sources.get(defined));
            json!({
                "path": path,
                "origin": source.map(|source| origin_id(source.origin)),
                "source_display": source.map(|source| source.display.clone()),
            })
        })
        .collect()
}

/// The union of the `requiredAddons` of every patch `id` declares, in order.
fn required_addons(resolver: &Resolver, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for patch in resolver.patches().into_iter().filter(|p| p.source == id) {
        for addon in patch.required_addons {
            if !out.contains(&addon) {
                out.push(addon);
            }
        }
    }
    out
}

fn resolved_class_json(class: &ResolvedClass, sources: &Sources) -> Value {
    json!({
        "path": class.path,
        "base_chain": class.base_chain.iter().map(|link| json!({
            "path": link.path,
            "source": sources.get(&link.source).map(source_info),
        })).collect::<Vec<_>>(),
        "properties": class.properties.iter().map(|property| json!({
            "name": property.name,
            "value": value_text(&property.value),
            "kind": value_kind(&property.value),
            "items": value_items(&property.value),
            "array": property.array,
            "from_class": property.from_class,
            "from_source": sources.get(&property.from_source).map(source_info),
            "own": property.own,
        })).collect::<Vec<_>>(),
        "children": class.children.iter().map(|child| json!({
            "name": child.name,
            "own": child.own,
            "from_class": child.from_class,
            "classes_below": child.classes_below,
        })).collect::<Vec<_>>(),
        "rebases": class.rebases.iter().map(|rebase| json!({
            "source": rebase.source,
            "from": rebase.from,
            "to": rebase.to,
        })).collect::<Vec<_>>(),
        "unresolved_base": class.unresolved_base,
    })
}

fn value_kind(value: &AstValue) -> &'static str {
    match value {
        AstValue::Str(_) => "str",
        AstValue::Number(_) | AstValue::Float(_) => "number",
        AstValue::Ident(_) => "ident",
        AstValue::Array(_) => "array",
        AstValue::Other(_) => "other",
    }
}

fn value_items(value: &AstValue) -> Vec<Value> {
    match value {
        AstValue::Array(items) => items
            .iter()
            .map(|item| json!({ "kind": value_kind(item), "text": value_text(item) }))
            .collect(),
        _ => Vec::new(),
    }
}

fn value_text(value: &AstValue) -> String {
    match value {
        AstValue::Str(text)
        | AstValue::Number(text)
        | AstValue::Ident(text)
        | AstValue::Other(text) => text.clone(),
        AstValue::Float(number) => format!("{number}"),
        AstValue::Array(items) => format!(
            "{{{}}}",
            items.iter().map(value_text).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn origin_id(origin: ConfigOrigin) -> String {
    match origin {
        ConfigOrigin::Vanilla => "vanilla".to_string(),
        ConfigOrigin::Dependency(n) => format!("dependency:{n}"),
        ConfigOrigin::Workspace => "workspace".to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_config::ast::ConfigClass;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mf-confhost-src-{label}-{}-{n}",
            std::process::id()
        ))
    }

    fn config_bin(text: &str) -> Vec<u8> {
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        modfoundry_config::write_rap(&parsed.file).unwrap()
    }

    fn pack(path: &Path, props: &[(&str, &str)], files: &[(&str, &[u8])]) {
        let properties: Vec<(String, String)> = props
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let entries: Vec<PboFileToWrite> = files
            .iter()
            .map(|(n, d)| PboFileToWrite {
                name: (*n).to_string(),
                data: d.to_vec(),
                timestamp: 1,
            })
            .collect();
        let bytes = pack_pbo(&properties, &entries, &PackOptions::default()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn find_class<'a>(members: &'a [Member], name: &str) -> Option<&'a ConfigClass> {
        members.iter().find_map(|member| match member {
            Member::Class(class) if class.name == name => Some(class),
            Member::Class(class) => find_class(&class.members, name),
            _ => None,
        })
    }

    fn request(workspace: &Path, game: &Path) -> ConfigIndexRequest {
        ConfigIndexRequest {
            workspace: workspace.to_path_buf(),
            dependencies: Vec::new(),
            game: Some(game.to_path_buf()),
        }
    }

    #[test]
    fn load_stores_the_id_the_vpath_and_the_raw_text_of_every_source() {
        let game = tmp("game");
        let root = tmp("ws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.cpp"), "class CfgVehicles {};\n").unwrap();
        let vanilla = config_bin("class CfgPatches { class DZ_A {}; };\n");
        let nested = config_bin("class CfgVehicles { class Nested_A {}; };\n");
        pack(
            &game.join("Addons/a.pbo"),
            &[("prefix", "dz/a")],
            &[
                ("config.bin", vanilla.as_slice()),
                ("sub\\config.bin", nested.as_slice()),
            ],
        );

        let (loaded, report) = load(&request(&root, &game));
        let mut ids: Vec<&str> = loaded.sources.keys().map(String::as_str).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "vanilla:a.pbo:config.bin",
                "vanilla:a.pbo:sub/config.bin",
                "workspace:config.cpp"
            ],
            "{}",
            report
        );
        let nested = &loaded.sources["vanilla:a.pbo:sub/config.bin"];
        assert_eq!(nested.config.vpath, "dz/a/sub/config.bin");
        assert_eq!(nested.config.display, "a.pbo");
        assert_eq!(nested.config.origin, ConfigOrigin::Vanilla);
        assert!(nested.text.is_none(), "a config.bin renders from its AST");
        let workspace = &loaded.sources["workspace:config.cpp"];
        assert_eq!(workspace.config.vpath, "config.cpp");
        assert_eq!(workspace.text.as_deref(), Some("class CfgVehicles {};\n"));
    }

    #[test]
    fn a_config_vpath_drops_the_outer_separators_of_a_prefix() {
        assert_eq!(
            config_vpath(Some("dz\\a\\"), b"config.bin").as_deref(),
            Some("dz/a/config.bin")
        );
    }

    #[test]
    fn a_bin_config_keeps_base_names_as_written() {
        let game = tmp("binbase");
        let root = tmp("binbase-ws");
        let text = "class CfgVehicles { class AmmoCore {}; class Bullet: ammocore {}; };\n";
        let bin = config_bin(text);
        pack(
            &game.join("Addons/a.pbo"),
            &[("prefix", "dz/a")],
            &[("config.bin", bin.as_slice())],
        );

        let (loaded, _) = load(&request(&root, &game));
        let file = &loaded.sources["vanilla:a.pbo:config.bin"].config.file;
        let root_class = find_class(&file.members, "CfgVehicles").unwrap();
        let bullet = find_class(&root_class.members, "Bullet").unwrap();
        assert_eq!(bullet.parents, vec!["ammocore".to_string()]);
        assert!(
            modfoundry_config::to_text(file).contains("class Bullet: AmmoCore"),
            "to_text rewrites the base name; the AST must not"
        );
    }

    #[test]
    fn folder_configs_are_keyed_under_their_prefix_marker() {
        let root = tmp("folder-marker");
        let addon = root.join("a");
        std::fs::create_dir_all(&addon).unwrap();
        let text = "class CfgPatches { class P { units[] = {}; }; };";
        std::fs::write(addon.join("$PBOPREFIX$"), "My\\Mod").unwrap();
        std::fs::write(addon.join("config.cpp"), text).unwrap();
        std::fs::write(addon.join("config.bin"), config_bin(text)).unwrap();
        let files = vec![addon.join("config.bin"), addon.join("config.cpp")];
        let mut warnings = Vec::new();

        let configs = folder_configs(&root, &files, &mut warnings);

        let _ = std::fs::remove_dir_all(&root);
        assert!(warnings.is_empty(), "{warnings:?}");
        let keys: Vec<(&str, &str)> = configs
            .iter()
            .map(|config| (config.vpath.as_str(), config.display.as_str()))
            .collect();
        assert_eq!(keys, [("My/Mod/config.bin", "My/Mod/config.bin")]);
    }
}
