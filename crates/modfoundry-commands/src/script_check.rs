//! The game check behind `script.check`: unpacks what is packed, checks each game build and
//! merges the builds' results into one list of findings.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure};
use enforce_check::loader::load_input;
use enforce_check::{CheckResult, Diagnostic, Layer, Module, SourceFile, Target};
use modfoundry_core::{Severity, Span};
use serde::{Deserialize, Serialize};

use crate::pbo_ops::pbos_of;
use crate::registry::FindingReport;
use crate::unpack_cache::{unpacked, Unpacked};
use crate::walk::walk_files_to;

/// One game finding, shown once with the builds that print it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameFinding {
    #[serde(flatten)]
    pub report: FindingReport,
    /// 1-based line, as the game prints it.
    pub line: u32,
    /// The builds that print this line, in the order given.
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related: Option<RelatedPlace>,
}

/// The second place a finding names, such as "Previously declared here".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedPlace {
    pub path: PathBuf,
    pub line: u32,
}

/// A module the game refuses to compile, with the builds that print it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedModule {
    pub module: String,
    pub targets: Vec<String>,
}

/// Arguments for `script.check`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptCheckArgs {
    pub root: PathBuf,
    #[serde(default)]
    pub dependencies: Vec<PathBuf>,
    pub game: Option<PathBuf>,
    #[serde(default)]
    pub vanilla: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    #[serde(default)]
    pub targets: Vec<String>,
}

/// The result of `script.check`: the builds checked and what each one reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameCheck {
    pub targets: Vec<String>,
    pub findings: Vec<GameFinding>,
    pub failed_modules: Vec<FailedModule>,
    /// The checked mod's files the game never reached: in a module after the first one it refused, or never loaded.
    #[serde(default)]
    pub unchecked: Vec<PathBuf>,
    /// The unpacked copy a packed mod was checked from; its findings point there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_from: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Every game build the check knows, in the order they are checked and shown.
pub const TARGETS: [Target; 3] = [Target::RetailServer, Target::DiagServer, Target::DiagClient];

/// The name a build is shown under.
pub fn target_name(target: Target) -> &'static str {
    match target {
        Target::RetailServer => "retail-server",
        Target::DiagServer => "diag-server",
        Target::DiagClient => "diag-client",
    }
}

/// The build a name stands for; the reverse of `target_name`.
pub fn parse_target(name: &str) -> Option<Target> {
    TARGETS
        .into_iter()
        .find(|target| target_name(*target) == name)
}

/// Merges per-build results into findings, keeping the order they are first seen.
pub fn merge_findings(
    results: &[(Target, CheckResult)],
    files: &[SourceFile],
    disk: &[PathBuf],
) -> (Vec<GameFinding>, Vec<FailedModule>) {
    let mut sources = SourceIndex::new(files, disk);
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of: HashMap<(&str, u32, &str), usize> = HashMap::new();
    let mut failed: Vec<FailedModule> = Vec::new();
    let mut failed_of: HashMap<&str, usize> = HashMap::new();
    for (target, result) in results {
        let name = target_name(*target);
        for diagnostic in &result.diagnostics {
            let key = (
                diagnostic.file.as_str(),
                diagnostic.line,
                diagnostic.message.as_str(),
            );
            let index = *group_of.entry(key).or_insert_with(|| {
                groups.push(Group::new(diagnostic, &mut sources));
                groups.len() - 1
            });
            groups[index].absorb(name, diagnostic, &sources);
        }
        for module in &result.failed_modules {
            let index = *failed_of.entry(module.as_str()).or_insert_with(|| {
                failed.push(FailedModule {
                    module: module.as_str().to_string(),
                    targets: Vec::new(),
                });
                failed.len() - 1
            });
            push_once(&mut failed[index].targets, name);
        }
    }
    let findings = groups.into_iter().map(Group::finish).collect();
    (findings, failed)
}

/// The disk paths of the checked mod's files (`mod_index` `checked_mod`) in a module after the first one any
/// build refused; the game stops at a refused module, so it never looked at those.
pub fn unchecked_files(
    results: &[(Target, CheckResult)],
    files: &[SourceFile],
    disk: &[PathBuf],
    checked_mod: usize,
) -> Vec<PathBuf> {
    let position = |module: Module| {
        Layer::ALL
            .iter()
            .position(|&layer| Module::from(layer) == module)
    };
    let Some(first_refused) = results
        .iter()
        .flat_map(|(_, result)| &result.failed_modules)
        .filter_map(|&module| position(module))
        .min()
    else {
        return Vec::new();
    };
    files
        .iter()
        .zip(disk)
        .filter(|(file, _)| file.mod_index == checked_mod)
        .filter(|(file, _)| position(Module::from(file.layer)) > Some(first_refused))
        .map(|(_, path)| path.clone())
        .collect()
}

/// The checked mod's script files on disk that the loader did not read, such as those in a folder
/// `CfgMods` never binds.
fn never_loaded(checked: Option<&(String, PathBuf)>, loaded: &[PathBuf]) -> Vec<PathBuf> {
    let Some((_, root)) = checked else {
        return Vec::new();
    };
    let loaded: HashSet<&PathBuf> = loaded.iter().collect();
    walk_files_to(root, usize::MAX, true, |_, name| {
        Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("c"))
    })
    .0
    .into_iter()
    .filter(|path| !loaded.contains(path))
    .collect()
}

/// Loads the vanilla scripts and the mods once, then checks them for each build.
pub fn check_folders(vanilla: &Path, mods: &[(String, PathBuf)], targets: &[Target]) -> GameCheck {
    let mut loaded = load_input(Target::RetailServer, Some(vanilla), mods);
    let mut results = Vec::with_capacity(targets.len());
    for &target in targets {
        loaded.input.target = target;
        results.push((target, enforce_check::check(&loaded.input)));
    }
    let (findings, failed_modules) = merge_findings(&results, &loaded.input.files, &loaded.disk);
    GameCheck {
        targets: targets
            .iter()
            .map(|&target| target_name(target).to_string())
            .collect(),
        unchecked: unchecked_files(&results, &loaded.input.files, &loaded.disk, mods.len())
            .into_iter()
            .chain(never_loaded(mods.last(), &loaded.disk))
            .collect(),
        findings,
        failed_modules,
        checked_from: None,
        notes: loaded.notes,
    }
}

/// Where the game's scripts come from: a folder already unpacked, or a game install.
pub enum VanillaSource {
    Folder(PathBuf),
    Game(PathBuf),
}

/// The folders `check_folders` reads, and the notes of what unpacking left out.
pub struct Sources {
    pub vanilla: PathBuf,
    pub mods: Vec<(String, PathBuf)>,
    pub notes: Vec<String>,
}

/// Unpacks the packed mods (the last is the one checked) and the game's scripts through `cache`; other folders pass through.
pub fn unpack_sources(
    cache: Option<&Path>,
    vanilla: VanillaSource,
    mods: Vec<(String, PathBuf)>,
) -> anyhow::Result<Sources> {
    // A wrong game folder is refused before a large mod is unpacked for nothing.
    if let VanillaSource::Game(game) = &vanilla {
        scripts_pbo(game)?;
    }
    let mut notes = Vec::new();
    let mut checked = Vec::with_capacity(mods.len());
    let checked_mod = mods.len().saturating_sub(1);
    for (index, (name, folder)) in mods.into_iter().enumerate() {
        if pbos_of(&folder).is_empty() {
            checked.push((name, folder));
            continue;
        }
        match unpack_copy(cache, &folder, &mut notes) {
            Ok(copy) => {
                if has_own_scripts(&folder) {
                    notes.push(format!(
                        "{}: checked from its PBOs, not from the scripts beside them",
                        folder.display()
                    ));
                }
                checked.push((name, copy.dir));
            }
            // A dependency that will not unpack is left out and said; the mod itself must unpack.
            Err(error) if index < checked_mod && cache.is_some() => {
                notes.push(format!("{}: left out, {error:#}", folder.display()));
            }
            Err(error) => return Err(error),
        }
    }
    let vanilla = match vanilla {
        VanillaSource::Folder(folder) => folder,
        VanillaSource::Game(game) => {
            let pbo = scripts_pbo(&game)?;
            let copy = unpack_copy(cache, &pbo, &mut notes)?;
            scripts_folder(&copy.dir)
                .ok_or_else(|| anyhow!("{} holds no 1_Core folder", pbo.display()))?
        }
    };
    Ok(Sources {
        vanilla,
        mods: checked,
        notes,
    })
}

fn unpack_copy(
    cache: Option<&Path>,
    source: &Path,
    notes: &mut Vec<String>,
) -> anyhow::Result<Unpacked> {
    let root = cache.ok_or_else(|| anyhow!("no cache folder: set XDG_CACHE_HOME or HOME"))?;
    let copy = unpacked(root, source)?;
    if let Some(left_out) = &copy.left_out {
        notes.push(format!("{}: {left_out}", source.display()));
    }
    Ok(copy)
}

fn scripts_pbo(game: &Path) -> anyhow::Result<PathBuf> {
    let pbo = game.join("dta").join("scripts.pbo");
    ensure!(
        pbo.is_file(),
        "no vanilla scripts at {}: pass game or vanilla",
        pbo.display()
    );
    Ok(pbo)
}

/// Whether the folder itself, beside its PBOs, holds scripts the check would read.
fn has_own_scripts(folder: &Path) -> bool {
    let own = enforce_check::loader::load_mod_dir(folder, "");
    own.packs.iter().any(|pack| !pack.files.is_empty())
}

fn scripts_folder(copy: &Path) -> Option<PathBuf> {
    if has_core(copy) {
        return Some(copy.to_path_buf());
    }
    let mut children: Vec<PathBuf> = fs::read_dir(copy)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    children.into_iter().find(|child| has_core(child))
}

// The loader takes the layer folders by their exact names.
fn has_core(dir: &Path) -> bool {
    dir.join("1_Core").is_dir()
}

fn push_once(targets: &mut Vec<String>, name: &str) {
    if !targets.iter().any(|existing| existing == name) {
        targets.push(name.to_string());
    }
}

struct Group<'a> {
    first: &'a Diagnostic,
    path: PathBuf,
    span: Option<Span>,
    targets: Vec<String>,
    related: Option<RelatedPlace>,
    crash: bool,
    fixme: bool,
    error: bool,
}

impl<'a> Group<'a> {
    fn new(first: &'a Diagnostic, sources: &mut SourceIndex) -> Self {
        Self {
            first,
            path: sources.disk_path(&first.file),
            span: sources.span(&first.file, first.line),
            targets: Vec::new(),
            related: None,
            crash: false,
            fixme: false,
            error: false,
        }
    }

    fn absorb(&mut self, target: &str, diagnostic: &Diagnostic, sources: &SourceIndex) {
        push_once(&mut self.targets, target);
        self.related = self.related.take().or_else(|| sources.related(diagnostic));
        self.crash |= diagnostic.game_crash;
        self.fixme |= diagnostic.fixme;
        self.error |= diagnostic.severity == enforce_check::Severity::Error;
    }

    fn code_and_severity(&self) -> (&'static str, Severity) {
        if self.crash {
            ("MF-E4003", Severity::Fatal)
        } else if self.fixme {
            ("MF-W4002", Severity::Warning)
        } else if self.error {
            ("MF-E4001", Severity::Error)
        } else {
            ("MF-W4004", Severity::Warning)
        }
    }

    fn finish(self) -> GameFinding {
        let (code, severity) = self.code_and_severity();
        GameFinding {
            report: FindingReport {
                diagnostic: modfoundry_core::Diagnostic {
                    code: code.to_string(),
                    severity,
                    span: self.span,
                    message: self.first.message.clone(),
                },
                path: self.path,
            },
            line: self.first.line,
            targets: self.targets,
            related: self.related,
        }
    }
}

struct SourceIndex<'a> {
    files: &'a [SourceFile],
    disk: &'a [PathBuf],
    index: HashMap<&'a str, usize>,
    /// Where each line of a file starts, filled the first time a finding needs that file.
    line_starts: HashMap<usize, Vec<usize>>,
}

impl<'a> SourceIndex<'a> {
    fn new(files: &'a [SourceFile], disk: &'a [PathBuf]) -> Self {
        let index = files
            .iter()
            .enumerate()
            .map(|(i, file)| (file.path.as_str(), i))
            .collect();
        Self {
            files,
            disk,
            index,
            line_starts: HashMap::new(),
        }
    }

    fn disk_path(&self, game_path: &str) -> PathBuf {
        let known = self.index.get(game_path).and_then(|&i| self.disk.get(i));
        known.map_or_else(|| PathBuf::from(game_path), Clone::clone)
    }

    fn related(&self, diagnostic: &Diagnostic) -> Option<RelatedPlace> {
        let (file, line) = diagnostic.related.as_ref()?;
        Some(RelatedPlace {
            path: self.disk_path(file),
            line: *line,
        })
    }

    fn span(&mut self, game_path: &str, line: u32) -> Option<Span> {
        let &i = self.index.get(game_path)?;
        let text = self.files[i].text.as_str();
        let starts = self
            .line_starts
            .entry(i)
            .or_insert_with(|| line_starts(text));
        let at = usize::try_from(line).ok()?.checked_sub(1)?;
        let start = *starts.get(at)?;
        let end = starts.get(at + 1).map_or(text.len(), |next| next - 1);
        let body = &text[start..end];
        Some(Span::new(
            start,
            start + body.strip_suffix('\r').unwrap_or(body).len(),
        ))
    }
}

/// The byte each line starts at; the text after a final newline is a line too, as the game counts it.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}
