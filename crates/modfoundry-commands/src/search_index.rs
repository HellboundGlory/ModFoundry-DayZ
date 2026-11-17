//! Builds the search index from a game install, dependencies and docs folders.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use modfoundry_config::{read_rap, to_text};
use modfoundry_pbo::PboArchive;
use modfoundry_platform::cuda::{cuda_lib_dir, GPU_HELPER};
use modfoundry_platform::program::sibling_program;
use modfoundry_search::chunk::{config_chunks, docs_chunks, layout_chunks, script_chunks, Chunk};
use modfoundry_search::embed::{BgeSmall, Embed};
use modfoundry_search::helper::HelperEmbed;
use modfoundry_search::model::{check_model, default_model_dir, ensure_model, MODEL_ID, REVISION};
use modfoundry_search::store::{IndexWriter, Meta};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::prefix_marker::Markers;
use crate::script_index::list_ext;
use crate::sources::{find_child, has_ext, mod_display_name, vanilla_addon_dirs};
use crate::walk::{entry_path, rel_path, walk_files};

/// The `ProductVersion` string in a PE file's version resource.
pub fn product_version(exe: &[u8]) -> Option<String> {
    let key: Vec<u8> = "ProductVersion\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let key_at = exe.windows(key.len()).position(|window| window == key)?;
    // The String header is wLength, wValueLength, wType: 6 bytes before the key.
    let header = key_at.checked_sub(6)?;
    let units = usize::from(u16::from_le_bytes([exe[header + 2], exe[header + 3]]));
    let value_at = (key_at + key.len()).next_multiple_of(4);
    let value = exe.get(value_at..value_at + 2 * units)?;
    let (pairs, _) = value.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let text = String::from_utf16(&units).ok()?;
    let text = text.trim_end_matches('\0');
    (!text.is_empty()).then(|| text.to_string())
}

/// The key a game build's index is stored under.
pub fn build_key(game: &Path, pbos: &[PathBuf]) -> String {
    build_key_checked(game, pbos, &mut Vec::new())
}

/// The key a game build's index is stored under, adding a warning per PBO it cannot size.
pub fn build_key_checked(game: &Path, pbos: &[PathBuf], warnings: &mut Vec<String>) -> String {
    exe_version(game).unwrap_or_else(|| pbo_key(game, pbos, warnings))
}

fn exe_version(game: &Path) -> Option<String> {
    find_child(game, "DayZ_x64.exe", false)
        .and_then(|exe| std::fs::read(exe).ok())
        .and_then(|bytes| product_version(&bytes))
        .filter(|version| is_folder_name(version))
}

/// The key names a cache folder, so a version read from the exe must be one plain name.
pub(crate) fn is_folder_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Without the executable, a key from each PBO's path and size.
fn pbo_key(game: &Path, pbos: &[PathBuf], warnings: &mut Vec<String>) -> String {
    let mut lines: Vec<(String, u64)> = pbos
        .iter()
        .map(|pbo| {
            let size = std::fs::metadata(pbo).map_or_else(
                |err| {
                    warnings.push(format!(
                        "{}: {err}; counted as size 0 in the build key",
                        pbo.display()
                    ));
                    0
                },
                |meta| meta.len(),
            );
            (rel_path(game, pbo), size)
        })
        .collect();
    lines.sort();
    let text: String = lines
        .iter()
        .map(|(path, size)| format!("{path}\t{size}\n"))
        .collect();
    format!("unknown-{}", &sha256_hex(text.as_bytes())[..12])
}

/// What `search.index` builds from: a game folder, dependency and docs folders, and the sources kept out of results.
pub struct SearchIndexRequest {
    pub game: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub docs: Vec<PathBuf>,
    pub reference_only: Vec<String>,
}

/// What a finished build wrote and what it skipped.
#[derive(Debug, Serialize)]
pub struct SearchIndexReport {
    pub index: PathBuf,
    pub build: String,
    pub sources: usize,
    pub files: usize,
    pub chunks: usize,
    pub millis: u64,
    pub warnings: Vec<String>,
}

/// The folder holding one game build's search index and its other cached files.
pub(crate) fn index_dir(cache: &Path, build: &str) -> PathBuf {
    cache.join("modfoundry").join("search").join(build)
}

/// Writes the index for `req` under `cache`, calling `progress` with chunks done and chunks total.
pub fn build_index(
    req: &SearchIndexRequest,
    embed: &dyn Embed,
    cache: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<SearchIndexReport, String> {
    let start = Instant::now();
    check_game(&req.game)?;
    let pbos = game_pbos(&req.game);
    let mut warnings = Vec::new();
    let build = build_key_checked(&req.game, &pbos, &mut warnings);
    let mut sources = gather_game(&req.game, &pbos, &mut warnings);
    sources.extend(gather_dependencies(&req.dependencies, &mut warnings));
    sources.extend(gather_docs(&req.docs, &mut warnings));
    for name in &req.reference_only {
        if !sources
            .iter()
            .any(|source| source.marked_by(std::slice::from_ref(name)))
        {
            warnings.push(format!("reference-only {name} matches no source"));
        }
    }

    let chunks: usize = sources
        .iter()
        .flat_map(|source| &source.files)
        .map(|file| file.pieces.len())
        .sum();
    let meta = Meta {
        model: MODEL_ID.into(),
        revision: REVISION.into(),
        build: build.clone(),
        created: now_secs().to_string(),
    };
    let index = index_dir(cache, &build).join("index.sqlite");
    let mut writer = IndexWriter::create(&index, &meta).map_err(|err| err.to_string())?;
    let source_count = sources.len();
    let mut files = 0;
    let mut done = 0;
    for source in sources {
        let reference_only = source.marked_by(&req.reference_only);
        let source_id = writer
            .add_source(&source.label, &source.root, reference_only)
            .map_err(|err| err.to_string())?;
        for file in source.files {
            let file_id = writer
                .add_file(source_id, &file.path, file.size, &file.hash)
                .map_err(|err| err.to_string())?;
            let texts: Vec<&str> = file
                .pieces
                .iter()
                .map(|(chunk, _)| chunk.text.as_str())
                .collect();
            let vectors = embed.embed(&texts)?;
            if vectors.len() != texts.len() {
                return Err(format!(
                    "embedder returned {} vectors for {} texts",
                    vectors.len(),
                    texts.len()
                ));
            }
            let pieces: Vec<(Chunk, String, Vec<f32>)> = file
                .pieces
                .into_iter()
                .zip(vectors)
                .map(|((chunk, snippet), vector)| (chunk, snippet, vector))
                .collect();
            writer
                .add_chunks(file_id, &pieces)
                .map_err(|err| err.to_string())?;
            done += pieces.len();
            files += 1;
            progress(done, chunks);
        }
    }
    writer.finish().map_err(|err| err.to_string())?;
    Ok(SearchIndexReport {
        index,
        build,
        sources: source_count,
        files,
        chunks,
        millis: start.elapsed().as_millis() as u64,
        warnings,
    })
}

/// What `search.index` returns: the build report, the model folder and the lines of a model download.
#[derive(Serialize)]
pub struct SearchIndexOutcome {
    #[serde(flatten)]
    pub report: SearchIndexReport,
    pub model: PathBuf,
    pub notes: Vec<String>,
}

/// Builds the index for a game install, with the model in `model` or the cached model, fetched on first use.
pub fn run_search_index(
    game: PathBuf,
    dependencies: Vec<PathBuf>,
    docs: Vec<PathBuf>,
    reference_only: Vec<String>,
    model: Option<PathBuf>,
) -> Result<SearchIndexOutcome, String> {
    check_game(&game)?;
    let cache = modfoundry_platform::app_cache_dir()
        .ok_or("no cache folder: set XDG_CACHE_HOME or HOME")?;
    let (model, mut notes) = ready_model(model, &cache)?;
    let helper = sibling_program(GPU_HELPER);
    let (embedder, note) = pick_embedder(
        helper.as_deref(),
        &model,
        cuda_lib_dir().as_deref(),
        &load_cpu,
    )?;
    notes.push(note);
    let req = SearchIndexRequest {
        game,
        dependencies,
        docs,
        reference_only,
    };
    let report = build_index(&req, &*embedder, &cache, &mut |_, _| {})?;
    Ok(SearchIndexOutcome {
        report,
        model,
        notes,
    })
}

type CpuLoader = dyn Fn(&Path) -> Result<Box<dyn Embed>, String>;

/// The embedder for `model`: the GPU helper when it starts, the CPU model otherwise, with the line to print.
pub fn pick_embedder(
    helper: Option<&Path>,
    model: &Path,
    lib_dir: Option<&Path>,
    cpu: &CpuLoader,
) -> Result<(Box<dyn Embed>, String), String> {
    let on_cpu = |note: String| cpu(model).map(|embedder| (embedder, note));
    let Some(helper) = helper else {
        return on_cpu("no GPU helper; embedding on the CPU".to_owned());
    };
    if !helper.is_file() {
        return on_cpu(format!(
            "no GPU helper at {}; embedding on the CPU",
            helper.display()
        ));
    }
    match HelperEmbed::spawn(helper, model, lib_dir) {
        Ok(gpu) => {
            let embedder: Box<dyn Embed> = Box::new(gpu);
            Ok((
                embedder,
                format!("embedding on the GPU with {}", helper.display()),
            ))
        }
        Err(err) => on_cpu(format!("GPU helper failed: {err}; embedding on the CPU")),
    }
}

fn load_cpu(model: &Path) -> Result<Box<dyn Embed>, String> {
    Ok(Box::new(BgeSmall::load(model)?))
}

fn check_game(game: &Path) -> Result<(), String> {
    if game.is_dir() {
        Ok(())
    } else {
        Err(format!("{} is not a folder", game.display()))
    }
}

/// The model folder to embed with, and the download lines when the model was fetched.
fn ready_model(model: Option<PathBuf>, cache: &Path) -> Result<(PathBuf, Vec<String>), String> {
    if let Some(dir) = model {
        check_model(&dir).map_err(|err| err.to_string())?;
        return Ok((dir, Vec::new()));
    }
    let dir = default_model_dir(cache);
    let mut notes = Vec::new();
    ensure_model(&dir, &modfoundry_search::model::HttpFetch, &mut |line| {
        notes.push(line.to_string())
    })
    .map_err(|err| err.to_string())?;
    Ok((dir, notes))
}

/// One source as stored: a label, its root and the files that have chunks.
struct Source {
    label: String,
    root: String,
    files: Vec<StoredFile>,
    /// Game sources are never reference-only.
    referenceable: bool,
}

impl Source {
    /// Whether one of `names` marks this source reference-only.
    fn marked_by(&self, names: &[String]) -> bool {
        self.referenceable
            && names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&self.label))
    }
}

/// A file with its chunks and snippets, ready to embed and store.
struct StoredFile {
    path: String,
    size: u64,
    hash: String,
    pieces: Vec<(Chunk, String)>,
}

/// A file with no chunks is not stored.
fn stored_file(path: String, bytes: &[u8], chunks: Vec<Chunk>) -> Option<StoredFile> {
    if chunks.is_empty() {
        return None;
    }
    Some(StoredFile {
        path,
        size: bytes.len() as u64,
        hash: sha256_hex(bytes),
        pieces: chunks
            .into_iter()
            .map(|chunk| {
                let snippet = snippet(&chunk.text);
                (chunk, snippet)
            })
            .collect(),
    })
}

/// The first five lines, trailing spaces trimmed, cut at 400 characters.
fn snippet(text: &str) -> String {
    let head = text
        .lines()
        .take(5)
        .map(|line| line.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");
    match head.char_indices().nth(400) {
        Some((cut, _)) => head[..cut].to_string(),
        None => head,
    }
}

fn gather_game(game: &Path, pbos: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Source> {
    let scripts =
        find_child(game, "dta", true).and_then(|dta| find_child(&dta, "scripts.pbo", false));
    pbos.iter()
        .map(|pbo| Source {
            label: "game".into(),
            root: pbo.to_string_lossy().into_owned(),
            files: pbo_files(pbo, scripts.as_ref() == Some(pbo), warnings),
            referenceable: false,
        })
        .collect()
}

/// Every PBO under the vanilla folders.
pub(crate) fn game_pbos(game: &Path) -> Vec<PathBuf> {
    vanilla_addon_dirs(game)
        .iter()
        .flat_map(|dir| list_ext(dir, "pbo"))
        .collect()
}

fn gather_dependencies(deps: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Source> {
    deps.iter()
        .filter_map(|dep| dependency_source(dep, warnings))
        .collect()
}

fn dependency_source(dep: &Path, warnings: &mut Vec<String>) -> Option<Source> {
    if !dep.exists() {
        warnings.push(format!("dependency {} not found", dep.display()));
        return None;
    }
    let files = if dep.is_file() {
        if !has_ext(dep, "pbo") {
            warnings.push(format!("{} is not a .pbo or a folder", dep.display()));
            return None;
        }
        pbo_files(dep, true, warnings)
    } else if let Some(pbos) = addon_pbos(dep) {
        pbos.iter()
            .flat_map(|pbo| pbo_files(pbo, true, warnings))
            .collect()
    } else {
        loose_files(dep, warnings)
    };
    Some(Source {
        label: mod_display_name(dep).unwrap_or_else(|| name_of(dep)),
        root: dep.to_string_lossy().into_owned(),
        files,
        referenceable: true,
    })
}

/// The PBOs in a folder's `addons` child, when it holds any.
pub(crate) fn addon_pbos(dep: &Path) -> Option<Vec<PathBuf>> {
    let pbos = list_ext(&find_child(dep, "addons", true)?, "pbo");
    (!pbos.is_empty()).then_some(pbos)
}

const DOCS_MAX_BYTES: u64 = 4 * 1024 * 1024;

fn gather_docs(folders: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Source> {
    folders
        .iter()
        .filter_map(|folder| {
            if !folder.is_dir() {
                warnings.push(format!("docs {} is not a folder", folder.display()));
                return None;
            }
            Some(Source {
                label: name_of(folder),
                root: folder.to_string_lossy().into_owned(),
                files: walked_files(
                    folder,
                    Some(DOCS_MAX_BYTES),
                    doc_piece,
                    |path| rel_path(folder, path),
                    warnings,
                ),
                referenceable: true,
            })
        })
        .collect()
}

/// The `.c`, `config.cpp` and `.layout` files under a loose dependency folder.
fn loose_files(root: &Path, warnings: &mut Vec<String>) -> Vec<StoredFile> {
    let mut markers = Markers::new(root);
    walked_files(
        root,
        None,
        loose_piece,
        move |path| markers.virtual_rel(path),
        warnings,
    )
}

/// The files under `root` that `classify` gives a piece, each cut and stored under `key`; a file that cannot be read or cut is a warning.
fn walked_files(
    root: &Path,
    max_bytes: Option<u64>,
    classify: impl Fn(&str) -> Option<Piece>,
    mut key: impl FnMut(&Path) -> String,
    warnings: &mut Vec<String>,
) -> Vec<StoredFile> {
    walk_files(root, false, |_, name| classify(name).is_some())
        .into_iter()
        .filter_map(|path| {
            let piece = classify(&rel_path(root, &path))?;
            if let Some(max) = max_bytes {
                if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > max) {
                    warnings.push(format!(
                        "{}: larger than {} MB, skipped",
                        path.display(),
                        max / (1024 * 1024)
                    ));
                    return None;
                }
            }
            let bytes = read_file(&path, warnings)?;
            match piece_chunks(piece, &bytes) {
                Ok(chunks) => stored_file(key(&path), &bytes, chunks),
                Err(err) => {
                    warnings.push(format!("{}: {err}", path.display()));
                    None
                }
            }
        })
        .collect()
}

/// The stored files of one PBO: its `.c` entries when `scripts`, and its config and `.layout` entries.
fn pbo_files(pbo: &Path, scripts: bool, warnings: &mut Vec<String>) -> Vec<StoredFile> {
    let archive = match PboArchive::open(pbo) {
        Ok(archive) => archive,
        Err(err) => {
            warnings.push(format!("{}: {err}", pbo.display()));
            return Vec::new();
        }
    };
    let prefix = pbo_prefix(&archive);
    let mut files = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().replace('\\', "/");
        let Some(piece) = piece_kind(&name, scripts) else {
            continue;
        };
        let bytes = match archive.read_entry(index) {
            Ok(bytes) => bytes,
            Err(err) => {
                warnings.push(format!("{}: {name}: {err}", pbo.display()));
                continue;
            }
        };
        let path = entry_path(&prefix, &name);
        match piece_chunks(piece, &bytes) {
            Ok(chunks) => files.extend(stored_file(path, &bytes, chunks)),
            Err(err) => warnings.push(format!("{}: {name}: {err}", pbo.display())),
        }
    }
    files
}

/// A PBO's prefix with `/` separators and no slash at either end.
pub(crate) fn pbo_prefix(archive: &PboArchive) -> String {
    archive
        .header()
        .property("prefix")
        .unwrap_or("")
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

#[derive(Clone, Copy)]
enum Piece {
    Script,
    ConfigBin,
    ConfigText,
    Layout,
    Docs,
}

/// How an entry (or loose file, by its relative path) is cut, or `None` when it is not indexed.
fn piece_kind(name: &str, scripts: bool) -> Option<Piece> {
    let file = name.rsplit('/').next().unwrap_or_default();
    if scripts && has_ext(Path::new(name), "c") {
        Some(Piece::Script)
    } else if file.eq_ignore_ascii_case("config.bin") {
        Some(Piece::ConfigBin)
    } else if file.eq_ignore_ascii_case("config.cpp") {
        Some(Piece::ConfigText)
    } else if has_ext(Path::new(name), "layout") {
        Some(Piece::Layout)
    } else {
        None
    }
}

/// Loose folders carry scripts and `config.cpp`, never binarised configs.
fn loose_piece(rel: &str) -> Option<Piece> {
    match piece_kind(rel, true)? {
        Piece::ConfigBin => None,
        piece => Some(piece),
    }
}

fn doc_piece(rel: &str) -> Option<Piece> {
    has_ext_any(rel, &["md", "txt"]).then_some(Piece::Docs)
}

fn piece_chunks(piece: Piece, bytes: &[u8]) -> Result<Vec<Chunk>, String> {
    match piece {
        Piece::Script => Ok(script_chunks(&String::from_utf8_lossy(bytes))),
        Piece::ConfigText => Ok(config_chunks(&String::from_utf8_lossy(bytes))),
        Piece::Docs => Ok(docs_chunks(&String::from_utf8_lossy(bytes))),
        Piece::Layout => layout_chunks(&String::from_utf8_lossy(bytes)),
        Piece::ConfigBin => {
            let file = read_rap(bytes).map_err(|err| err.to_string())?;
            Ok(config_chunks(&to_text(&file)))
        }
    }
}

fn read_file(path: &Path, warnings: &mut Vec<String>) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            None
        }
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn has_ext_any(name: &str, exts: &[&str]) -> bool {
    exts.iter().any(|ext| has_ext(Path::new(name), ext))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
