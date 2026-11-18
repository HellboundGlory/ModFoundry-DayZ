//! Checks `.layout` files against the game's widget vocabulary and the files they reference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use modfoundry_layout::check::{check, Reference, Resolve};
use modfoundry_layout::imageset::{parse_imageset, ImagesetError};
use modfoundry_layout::parse::parse;
use modfoundry_layout::styles::style_names;
use modfoundry_layout::vocab::Vocabulary;
use modfoundry_pbo::PboArchive;
use modfoundry_script::extract;
use serde::{Deserialize, Serialize};

use crate::paths::{ends_with_path, extension, norm};
use crate::prefix_marker::Markers;
use crate::registry::FindingReport;
use crate::search_index::{addon_pbos, build_key_checked, game_pbos, index_dir, pbo_prefix};
use crate::sources::open_pbo;
use crate::sources::{find_child, has_ext};
use crate::walk::{entry_path, walk_layout_tree};

/// The learning and reading rules, bumped whenever they change; the cache is keyed by game build alone.
const SCHEMA: u32 = 2;

/// What the game's own files teach the check, cached once per game build.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GameLayouts {
    pub schema: u32,
    pub vocabulary: Vocabulary,
    pub image_sets: BTreeMap<String, BTreeSet<String>>,
    pub styles: BTreeSet<String>,
}

/// What `layout.check` checks and against which install and mod.
pub struct LayoutCheckRequest {
    pub path: PathBuf,
    pub root: Option<PathBuf>,
    pub game: PathBuf,
    pub dependencies: Vec<PathBuf>,
}

/// The findings over every checked layout, with the game cache's state and any warnings.
#[derive(Debug, Serialize)]
pub struct LayoutCheckReport {
    pub files: usize,
    pub findings: Vec<FindingReport>,
    pub cache: PathBuf,
    pub cached: bool,
    pub warnings: Vec<String>,
}

/// Checks the `.layout` files at `req.path` against the game, its dependencies and the mod.
pub fn check_layouts(req: &LayoutCheckRequest, cache: &Path) -> Result<LayoutCheckReport, String> {
    if !req.path.exists() {
        return Err(format!("{} does not exist", req.path.display()));
    }
    if !req.game.is_dir() {
        return Err(format!("game {} is not a folder", req.game.display()));
    }
    let mut warnings = Vec::new();
    let files = layout_files(&req.path, &mut warnings)?;
    let pbos = game_pbos(&req.game);
    let mut archives = Vec::new();
    let mut known = Known::default();
    for pbo in &pbos {
        if let Some(archive) = open_pbo(pbo, &mut warnings) {
            known.add_pbo(&archive);
            archives.push(archive);
        }
    }
    let failed = pbos.len() - archives.len();
    let (game, cached, cache_file) =
        game_layouts(&req.game, &pbos, &archives, failed, cache, &mut warnings);
    let mut learner = Learner::new(game);
    for dep in &req.dependencies {
        learn_dependency(&mut learner, &mut known, dep, &mut warnings);
    }
    let lib = learner.finish(&mut warnings);
    for (set, images) in &lib.image_sets {
        known.add_image_set(set, images);
    }
    known.add_styles(&lib.styles);
    let root = mod_root(&req.path, req.root.as_deref());
    add_mod_root(&mut known, &root, &mut warnings);

    let mut findings = Vec::new();
    let mut checked = 0;
    for file in &files {
        let Some(text) = read_text(file, &mut warnings) else {
            continue;
        };
        checked += 1;
        for diagnostic in check(&text, &lib.vocabulary, &known) {
            findings.push(FindingReport {
                diagnostic,
                path: file.clone(),
            });
        }
    }
    Ok(LayoutCheckReport {
        files: checked,
        findings,
        cache: cache_file,
        cached,
        warnings,
    })
}

/// The `.layout` files at `path`, or `path` itself when it is a file.
fn layout_files(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<PathBuf>, String> {
    let files = if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        walk_layout_tree(path, |_, name| has_ext(Path::new(name), "layout"), warnings)
    };
    if files.is_empty() {
        let mut error = format!("no .layout files under {}", path.display());
        for warning in warnings.iter() {
            error.push_str(&format!("; {warning}"));
        }
        return Err(error);
    }
    Ok(files)
}

/// The game's layouts from the build's cache file, or learned from the already-open game PBOs and cached again.
fn game_layouts(
    game: &Path,
    pbos: &[PathBuf],
    archives: &[PboArchive],
    failed: usize,
    cache: &Path,
    warnings: &mut Vec<String>,
) -> (GameLayouts, bool, PathBuf) {
    let dir = index_dir(cache, &build_key_checked(game, pbos, warnings));
    let file = dir.join("layout-game.json");
    if let Some(lib) = read_cache(&file) {
        return (lib, true, file);
    }
    let (lib, complete) = build_game(archives, failed, warnings);
    if !complete {
        return (lib, false, file);
    }
    if let Err(err) = write_cache(&dir, &file, &lib) {
        warnings.push(format!(
            "layout cache not written to {}: {err}",
            file.display()
        ));
    }
    (lib, false, file)
}

fn read_cache(file: &Path) -> Option<GameLayouts> {
    let lib: GameLayouts = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    (lib.schema == SCHEMA).then_some(lib)
}

fn write_cache(dir: &Path, file: &Path, lib: &GameLayouts) -> std::io::Result<()> {
    static PART_COUNTER: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let n = PART_COUNTER.fetch_add(1, Ordering::Relaxed);
    let part = dir.join(format!("layout-game-{}-{n}.part", std::process::id()));
    let written = serde_json::to_vec(lib)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&part, bytes))
        .and_then(|()| std::fs::rename(&part, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// The game's layouts, scripts, image sets and styles, learned from the game PBOs already open; `complete` is
/// false when a PBO failed to open or an entry could not be read.
fn build_game(
    archives: &[PboArchive],
    failed: usize,
    warnings: &mut Vec<String>,
) -> (GameLayouts, bool) {
    let mut learner = Learner::new(GameLayouts::default());
    let mut complete = failed == 0;
    for archive in archives {
        complete &= learner.learn_pbo(archive, warnings);
    }
    (learner.finish(warnings), complete)
}

/// Learns what a set of files teaches the check, adding each class and file to `lib` as it goes.
struct Learner {
    lib: GameLayouts,
    classes: Vec<(String, Option<String>)>,
    unparsed: usize,
    unparsed_imagesets: usize,
}

impl Learner {
    fn new(mut lib: GameLayouts) -> Self {
        lib.schema = SCHEMA;
        // Script widgets learned earlier stay reachable, so dependency classes deriving from them count.
        let classes = lib
            .vocabulary
            .script_widgets
            .iter()
            .filter_map(|name| name.strip_suffix("Class"))
            .map(|name| (name.to_string(), Some("Widget".to_string())))
            .collect();
        Self {
            lib,
            classes,
            unparsed: 0,
            unparsed_imagesets: 0,
        }
    }

    /// Learns every wanted entry; `false` when one could not be read.
    fn learn_pbo(&mut self, archive: &PboArchive, warnings: &mut Vec<String>) -> bool {
        let mut complete = true;
        for (index, entry) in archive.entries().iter().enumerate() {
            let name = entry.name_str().replace('\\', "/");
            if !is_learned(&name) {
                continue;
            }
            match archive.read_entry(index) {
                Ok(bytes) => self.learn(&name, &String::from_utf8_lossy(&bytes)),
                Err(err) => {
                    complete = false;
                    warnings.push(format!("{}: {name}: {err}", archive.path().display()));
                }
            }
        }
        complete
    }

    fn learn(&mut self, name: &str, text: &str) {
        match extension(name).as_str() {
            "layout" => match parse(text) {
                Ok(widgets) => self.lib.vocabulary.learn(&widgets),
                Err(_) => self.unparsed += 1,
            },
            "imageset" => match image_set(text) {
                Ok((set, images)) => self.lib.image_sets.entry(set).or_default().extend(images),
                Err(_) => self.unparsed_imagesets += 1,
            },
            "styles" => self.lib.styles.extend(style_names(text)),
            "c" => self.classes.extend(
                extract(text)
                    .classes
                    .into_iter()
                    .map(|class| (class.name, class.base.map(|base| base.name))),
            ),
            _ => {}
        }
    }

    fn finish(mut self, warnings: &mut Vec<String>) -> GameLayouts {
        self.lib.vocabulary.add_script_widgets(&self.classes);
        if self.unparsed > 0 {
            warnings.push(format!(
                "{} .layout file(s) did not parse and were not learned",
                self.unparsed
            ));
        }
        if self.unparsed_imagesets > 0 {
            warnings.push(format!(
                "{} .imageset file(s) did not parse and were not learned",
                self.unparsed_imagesets
            ));
        }
        self.lib
    }
}

/// Whether an entry or loose file holds something the check learns from.
fn is_learned(name: &str) -> bool {
    matches!(
        extension(name).as_str(),
        "layout" | "imageset" | "styles" | "c"
    )
}

/// Adds a dependency's PBOs or loose files to the resolver and the learner; a missing one is a warning.
fn learn_dependency(
    learner: &mut Learner,
    known: &mut Known,
    dep: &Path,
    warnings: &mut Vec<String>,
) {
    if !dep.exists() {
        warnings.push(format!("dependency {} not found", dep.display()));
    } else if dep.is_file() {
        if has_ext(dep, "pbo") {
            learn_pbo(learner, known, dep, warnings);
        } else {
            warnings.push(format!("{} is not a .pbo or a folder", dep.display()));
        }
    } else if let Some(pbos) = addon_pbos(dep) {
        for pbo in pbos {
            learn_pbo(learner, known, &pbo, warnings);
        }
    } else {
        learn_loose(learner, known, dep, warnings);
    }
}

fn learn_pbo(learner: &mut Learner, known: &mut Known, pbo: &Path, warnings: &mut Vec<String>) {
    if let Some(archive) = open_pbo(pbo, warnings) {
        known.add_pbo(&archive);
        learner.learn_pbo(&archive, warnings);
    }
}

fn learn_loose(learner: &mut Learner, known: &mut Known, root: &Path, warnings: &mut Vec<String>) {
    let mut markers = Markers::new(root);
    for path in walk_layout_tree(root, |_, _| true, warnings) {
        let rel = markers.virtual_rel(&path);
        known.add_loose(&rel);
        if is_learned(&rel) {
            if let Some(text) = read_text(&path, warnings) {
                learner.learn(&rel, &text);
            }
        }
    }
}

/// The mod's own loose files, image sets and styles; its layouts are not learned.
fn add_mod_root(known: &mut Known, root: &Path, warnings: &mut Vec<String>) {
    if !root.is_dir() {
        warnings.push(format!("mod root {} is not a folder", root.display()));
        return;
    }
    for (rel, path) in loose_files(root, warnings) {
        known.add_loose(&rel);
        match extension(&rel).as_str() {
            "imageset" => {
                if let Some(text) = read_text(&path, warnings) {
                    match image_set(&text) {
                        Ok((set, images)) => known.add_image_set(&set, &images),
                        Err(err) => warnings.push(format!("{}: {err}", path.display())),
                    }
                }
            }
            "styles" => {
                if let Some(text) = read_text(&path, warnings) {
                    known.add_styles(&style_names(&text));
                }
            }
            _ => {}
        }
    }
}

/// The mod root: `root` when given, else the nearest folder at or above `path` holding a `config.cpp`,
/// else `path` itself when it is a folder or its parent when it is a file.
pub(crate) fn mod_root(path: &Path, requested: Option<&Path>) -> PathBuf {
    if let Some(root) = requested {
        return root.to_path_buf();
    }
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let start = if path.is_dir() {
        path.clone()
    } else {
        path.parent()
            .map_or_else(|| path.clone(), Path::to_path_buf)
    };
    start
        .ancestors()
        .find(|dir| find_child(dir, "config.cpp", false).is_some())
        .map_or_else(|| start.clone(), Path::to_path_buf)
}

fn loose_files(root: &Path, warnings: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    walk_layout_tree(root, |_, _| true, warnings)
        .into_iter()
        .map(|path| (relative(root, &path), path))
        .collect()
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn read_text(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            None
        }
    }
}

/// The files a layout may reference, by the names the resolver matches (ASCII case ignored, `\` as `/`).
#[derive(Default)]
struct Known {
    paths: BTreeSet<String>,
    loose: BTreeSet<String>,
    image_sets: BTreeMap<String, BTreeSet<String>>,
    styles: BTreeSet<String>,
}

impl Known {
    fn add_pbo(&mut self, archive: &PboArchive) {
        let prefix = pbo_prefix(archive);
        for entry in archive.entries() {
            self.paths
                .insert(norm(&entry_path(&prefix, &entry.name_str())));
        }
    }

    fn add_loose(&mut self, rel: &str) {
        self.loose.insert(norm(rel));
    }

    fn add_image_set(&mut self, set: &str, images: &BTreeSet<String>) {
        self.image_sets
            .entry(norm(set))
            .or_default()
            .extend(images.iter().map(|image| norm(image)));
    }

    fn add_styles(&mut self, styles: &BTreeSet<String>) {
        self.styles.extend(styles.iter().map(|style| norm(style)));
    }

    fn has_file(&self, path: &str) -> bool {
        let path = norm(path);
        self.paths.contains(&path) || self.loose.iter().any(|rel| ends_with_path(&path, rel))
    }
}

impl Resolve for Known {
    fn resolves(&self, reference: &Reference) -> bool {
        match reference {
            Reference::Texture(path) => self.has_file(path),
            // A family name such as `gui/fonts/Metron` is described by `Metron.xml`, which lists its sizes.
            Reference::Font(path) => ["", ".fnt", ".xml"]
                .iter()
                .any(|ext| self.has_file(&format!("{path}{ext}"))),
            Reference::Image { set, image } => self
                .image_sets
                .get(&norm(set))
                .is_some_and(|images| images.contains(&norm(image))),
            Reference::Style(name) => self.styles.contains(&norm(name)),
        }
    }
}

/// The set name and image names of a `.imageset` file, read by the render's strict reader.
pub fn image_set(text: &str) -> Result<(String, BTreeSet<String>), ImagesetError> {
    let set = parse_imageset(text)?;
    if set.name.is_empty() {
        return Err(ImagesetError {
            line: 1,
            message: "the set has no Name".to_string(),
        });
    }
    Ok((
        set.name,
        set.images.into_iter().map(|image| image.name).collect(),
    ))
}
