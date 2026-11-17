//! `pbo.unpack`: a PBO's files, or every PBO of a mod folder, into folders under `out`.

use crate::script_index::list_ext;
use crate::search_index::addon_pbos;
use crate::{dir_txn, file_rel};
use anyhow::{bail, ensure, Context, Result};
use modfoundry_vfs::{ChangeOp, ChangeSet, PboBackend, WriteTxn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Arguments for `pbo.unpack`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PboUnpackArgs {
    /// The PBO to unpack, or a mod folder.
    pub path: PathBuf,
    /// The folder to unpack into; created if missing, never overwritten.
    pub out: PathBuf,
}

/// What an unpack wrote: its summary, the entries left out, and the change-set that undoes it.
pub struct UnpackDone {
    pub summary: String,
    /// The summary's tail after `; `; `None` when everything was unpacked.
    pub left_out: Option<String>,
    pub change_set: ChangeSet,
}

#[derive(Clone, Copy)]
enum Layout {
    AsStored,
    UnderPrefix,
}

impl Layout {
    fn inside<'a>(self, rel: &'a str, prefix: Option<&str>) -> &'a str {
        match (self, prefix) {
            (Layout::AsStored, Some(prefix)) => rel
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(rel),
            _ => rel,
        }
    }
}

/// Unpacks the PBO or the folder's PBOs; returns the summary line and the change-set that undoes it.
pub fn unpack_pbo(args: &PboUnpackArgs) -> Result<(String, ChangeSet)> {
    unpack(args, true, Layout::AsStored).map(|done| (done.summary, done.change_set))
}

/// The same unpack as a dry run: the change-set it would apply.
pub fn plan_unpack_pbo(args: &PboUnpackArgs) -> Result<ChangeSet> {
    unpack(args, false, Layout::AsStored).map(|done| done.change_set)
}

/// The game check's cache keeps the prefix folders, because its loader finds scripts through them.
pub(crate) fn unpack_under_prefix(args: &PboUnpackArgs) -> Result<UnpackDone> {
    unpack(args, true, Layout::UnderPrefix)
}

/// Refuses a path that is neither a file nor a folder with PBOs, or an `out` that is a file.
pub fn check_args(args: &PboUnpackArgs) -> Result<()> {
    if args.path.is_dir() {
        ensure!(
            !units(&args.path).is_empty(),
            "no PBOs under {}",
            args.path.display()
        );
    } else {
        ensure!(args.path.is_file(), "{} is not a file", args.path.display());
    }
    ensure!(
        !args.out.is_file(),
        "{} is a file, not a folder",
        args.out.display()
    );
    Ok(())
}

/// Bytes held before the transaction applies them, so a large PBO is never in memory whole.
const HELD_BYTES: usize = 64 * 1024 * 1024;

/// A PBO to unpack, and the folder its entries go into under `out`; none for a single PBO.
struct Unit {
    pbo: PathBuf,
    folder: Option<String>,
}

pub(crate) fn pbos_of(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return vec![path.to_path_buf()];
    }
    addon_pbos(path).unwrap_or_else(|| list_ext(path, "pbo"))
}

fn units(path: &Path) -> Vec<Unit> {
    let in_folder = path.is_dir();
    pbos_of(path)
        .into_iter()
        .map(|pbo| {
            let folder = in_folder.then(|| {
                pbo.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            Unit { pbo, folder }
        })
        .collect()
}

/// A PBO whose entries are planned, each path already under its folder.
struct Ready {
    name: String,
    backend: PboBackend,
    planned: Vec<(String, String)>,
    converted: Vec<Conversion>,
    prefix_file: Option<PrefixFile>,
}

const PREFIX_FILE: &str = "$PBOPREFIX$";

struct PrefixFile {
    rel: String,
    bytes: Vec<u8>,
}

fn prefix_file_of(folder: &str, prefix: &str, planned: &[(String, String)]) -> Option<PrefixFile> {
    let rel = join(folder, PREFIX_FILE);
    let lower = rel.to_ascii_lowercase();
    let beneath = format!("{lower}/");
    let taken = planned.iter().any(|(_, path)| {
        let path = path.to_ascii_lowercase();
        path == lower || path.starts_with(&beneath)
    });
    (!taken).then(|| PrefixFile {
        rel,
        bytes: prefix.replace('/', "\\").into_bytes(),
    })
}

struct Conversion {
    index: String,
    bin_rel: String,
    cpp_rel: String,
}

fn conversions_of(planned: &[(String, String)]) -> Vec<Conversion> {
    let mut taken: HashSet<String> = planned
        .iter()
        .flat_map(|(_, rel)| ancestors_and_self(rel))
        .collect();
    planned
        .iter()
        .filter(|(_, rel)| file_name(rel).eq_ignore_ascii_case("config.bin"))
        .filter_map(|(index, rel)| {
            let cpp_rel = sibling(rel, "config.cpp");
            taken
                .insert(cpp_rel.to_ascii_lowercase())
                .then(|| Conversion {
                    index: index.clone(),
                    bin_rel: rel.clone(),
                    cpp_rel,
                })
        })
        .collect()
}

fn ancestors_and_self(rel: &str) -> impl Iterator<Item = String> + '_ {
    rel.match_indices('/')
        .map(move |(at, _)| rel[..at].to_ascii_lowercase())
        .chain(std::iter::once(rel.to_ascii_lowercase()))
}

fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

fn sibling(rel: &str, name: &str) -> String {
    join(rel.rsplit_once('/').map_or("", |(dir, _)| dir), name)
}

/// A PBO of a folder that did not unpack, by file name and reason.
struct Failure {
    name: String,
    reason: String,
}

fn file_name_of(pbo: &Path) -> String {
    pbo.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Refuses a folder the unpack cannot write, or one another PBO of the call already has;
/// the game folds letter case, so `a` and `A` are one folder.
fn claim_folder(out: &Path, unit: &Unit, taken: &mut HashSet<String>) -> Result<()> {
    let Some(folder) = &unit.folder else {
        return Ok(());
    };
    let writable = !folder.is_empty()
        && dir_txn(out, false)
            .write(join(folder, "x"), Vec::new())
            .is_ok();
    ensure!(writable, "its name cannot be a folder name");
    ensure!(
        taken.insert(folder.to_ascii_lowercase()),
        "another PBO already unpacks into the folder {folder}"
    );
    Ok(())
}

fn prepare(unit: &Unit, layout: Layout, left_out: &mut LeftOut) -> Result<Ready> {
    let backend = PboBackend::open(&unit.pbo)?;
    let (planned, shadowed) = planned_paths(&backend);
    left_out.unsafe_names += backend.skipped_names() + shadowed;
    left_out.case_duplicates += backend.case_collisions();
    let folder = unit.folder.as_deref().unwrap_or_default();
    let planned: Vec<(String, String)> = planned
        .into_iter()
        .map(|(index, rel)| {
            let inside = layout.inside(&rel, backend.prefix());
            (index, join(folder, inside))
        })
        .collect();
    let converted = conversions_of(&planned);
    let prefix_file = match (layout, backend.prefix()) {
        (Layout::AsStored, Some(prefix)) => prefix_file_of(folder, prefix, &planned),
        _ => None,
    };
    Ok(Ready {
        name: file_name_of(&unit.pbo),
        backend,
        planned,
        converted,
        prefix_file,
    })
}

fn failed_part(failures: &[Failure]) -> Option<String> {
    let first = failures.first()?;
    Some(format!(
        "{} PBO(s) failed (first: {}: {})",
        failures.len(),
        first.name,
        first.reason
    ))
}

fn with_tail(head: String, parts: &[String]) -> String {
    if parts.is_empty() {
        head
    } else {
        format!("{head}; {}", parts.join("; "))
    }
}

fn unpack(args: &PboUnpackArgs, recording: bool, layout: Layout) -> Result<UnpackDone> {
    check_args(args)?;
    let out = std::path::absolute(&args.out)?;
    let single = !args.path.is_dir();
    let mut left_out = LeftOut::default();
    let mut prepared: Vec<Result<Ready, Failure>> = Vec::new();
    let mut taken = HashSet::new();
    for unit in units(&args.path) {
        let ready = claim_folder(&out, &unit, &mut taken)
            .and_then(|()| prepare(&unit, layout, &mut left_out));
        match ready {
            Ok(ready) => prepared.push(Ok(ready)),
            Err(error) if single => return Err(error),
            Err(error) => prepared.push(Err(Failure {
                name: file_name_of(&unit.pbo),
                reason: error.to_string(),
            })),
        }
    }
    refuse_existing(
        &out,
        prepared
            .iter()
            .filter_map(|slot| slot.as_ref().ok())
            .flat_map(|ready| {
                let planned = ready.planned.iter().map(|(_, rel)| rel.as_str());
                let converted = ready
                    .converted
                    .iter()
                    .map(|conversion| conversion.cpp_rel.as_str());
                let prefix_file = ready.prefix_file.iter().map(|file| file.rel.as_str());
                planned.chain(converted).chain(prefix_file)
            }),
    )?;
    let partial = || format!("files already written stay under {}", out.display());
    let mut txn = dir_txn(&out, recording);
    let mut held = 0;
    let mut unconverted = Unconverted::default();
    let (mut files, mut pbos) = (0, 0);
    let mut failures = Vec::new();
    for slot in prepared {
        let ready = match slot {
            Ok(ready) => ready,
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        let written = write_entries(
            &ready.backend,
            &ready,
            &mut txn,
            &mut left_out,
            &mut unconverted,
            &mut held,
        )
        .with_context(partial)?;
        if written == 0 {
            failures.push(Failure {
                name: ready.name,
                reason: "nothing to unpack".to_owned(),
            });
            continue;
        }
        if let Some(file) = ready.prefix_file {
            put(&mut txn, &file.rel, file.bytes, &mut held).with_context(partial)?;
        }
        files += written;
        pbos += 1;
    }
    let source = if single {
        file_rel(&args.path)?
    } else {
        args.path.display().to_string()
    };
    let mut parts: Vec<String> = left_out
        .summary_part()
        .into_iter()
        .chain(unconverted.summary_part())
        .collect();
    if !single {
        parts.extend(failed_part(&failures));
    }
    ensure!(
        files > 0,
        "{}",
        with_tail(format!("nothing to unpack from {source}"), &parts)
    );
    let change_set = txn.finish().with_context(partial)?;
    let head = if single {
        format!(
            "unpacked {files} file(s) from {source} into {}",
            out.display()
        )
    } else {
        format!(
            "unpacked {files} file(s) from {pbos} PBO(s) under {source} into {}",
            out.display()
        )
    };
    Ok(UnpackDone {
        summary: with_tail(head, &parts),
        left_out: (!parts.is_empty()).then(|| parts.join("; ")),
        change_set,
    })
}

/// Writes every readable entry and returns how many were written. An entry that
/// cannot be read is counted in `left_out` and skipped. `held` carries the bytes
/// written but not yet applied across the PBOs of one unpack.
fn write_entries(
    backend: &PboBackend,
    ready: &Ready,
    txn: &mut WriteTxn,
    left_out: &mut LeftOut,
    unconverted: &mut Unconverted,
    held: &mut usize,
) -> Result<usize> {
    let mut written = 0;
    for (index, rel) in &ready.planned {
        let conversion = ready.converted.iter().find(|c| &c.index == index);
        match backend.read(index) {
            Ok(bytes) => {
                if let Some(conversion) = conversion {
                    write_conversion(txn, conversion, &bytes, unconverted, held)?;
                }
                put(txn, rel, bytes, held)?;
                written += 1;
            }
            Err(error) => {
                left_out.skip_unreadable(rel);
                if let Some(conversion) = conversion {
                    unconverted.skip(&conversion.bin_rel, &error.to_string());
                }
            }
        }
    }
    Ok(written)
}

fn write_conversion(
    txn: &mut WriteTxn,
    conversion: &Conversion,
    bytes: &[u8],
    unconverted: &mut Unconverted,
    held: &mut usize,
) -> Result<()> {
    match config_text(bytes) {
        Ok(text) => put(txn, &conversion.cpp_rel, text.into_bytes(), held),
        Err(error) => {
            unconverted.skip(&conversion.bin_rel, &error);
            Ok(())
        }
    }
}

fn config_text(bytes: &[u8]) -> Result<String, String> {
    let file = modfoundry_config::read_rap(bytes).map_err(|error| error.to_string())?;
    Ok(modfoundry_config::to_text(&file))
}

fn put(txn: &mut WriteTxn, rel: &str, bytes: Vec<u8>, held: &mut usize) -> Result<()> {
    *held += bytes.len();
    txn.write(rel, bytes)?;
    if *held >= HELD_BYTES {
        txn.apply_pending()?;
        *held = 0;
    }
    Ok(())
}

#[derive(Default)]
struct Unconverted {
    count: usize,
    first: Option<String>,
}

impl Unconverted {
    fn skip(&mut self, rel: &str, error: &str) {
        self.count += 1;
        self.first.get_or_insert_with(|| format!("{rel}: {error}"));
    }

    fn summary_part(&self) -> Option<String> {
        let first = self.first.as_deref()?;
        Some(format!(
            "{} config.bin not converted (first: {first})",
            self.count
        ))
    }
}

/// Refuses a set that would replace a file under `out`. The dry run also preflights
/// the whole set, so a case clash or a path through a file stops it before any
/// payload is read.
fn refuse_existing<'a>(out: &Path, rels: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let rels: Vec<&str> = rels.into_iter().collect();
    let mut dry = dir_txn(out, false);
    for rel in &rels {
        dry.write(rel, Vec::new())?;
    }
    let change_set = dry.finish()?;
    let existing: Vec<&str> = rels
        .iter()
        .zip(change_set.files())
        .filter(|(_, change)| change.op == ChangeOp::Update)
        .map(|(rel, _)| *rel)
        .collect();
    if let Some(first) = existing.first() {
        bail!(
            "{} already holds {} of these files (first: {}); unpack into another folder or delete them",
            out.display(),
            existing.len(),
            out.join(first).display()
        );
    }
    Ok(())
}

/// Room for the writer's staging suffix under the usual 255-byte file name limit.
const MAX_NAME_BYTES: usize = 200;
const MAX_PATH_BYTES: usize = 2048;

fn fits_filesystem(rel: &str) -> bool {
    rel.len() <= MAX_PATH_BYTES && rel.split('/').all(|name| name.len() <= MAX_NAME_BYTES)
}

/// Each indexed path with the path it unpacks to, and how many were left out (too long, or a file where a folder goes).
/// A folder keeps its first spelling in sorted order, as the game folds folder case and Linux does not.
fn planned_paths(backend: &PboBackend) -> (Vec<(String, String)>, usize) {
    let mut paths: Vec<&str> = backend.index().paths().collect();
    paths.sort_unstable();
    let mut folders = HashMap::new();
    let all: Vec<(String, String)> = paths
        .into_iter()
        .map(|index| (index.to_owned(), fold_folders(index, &mut folders)))
        .collect();
    let (kept, dropped): (Vec<_>, Vec<_>) = all.into_iter().partition(|(index, rel)| {
        fits_filesystem(rel) && !folders.contains_key(&index.to_ascii_lowercase())
    });
    (kept, dropped.len())
}

fn fold_folders(index: &str, folders: &mut HashMap<String, String>) -> String {
    let (dirs, name) = index.rsplit_once('/').unwrap_or(("", index));
    let mut lower = String::new();
    let mut folded = String::new();
    for dir in dirs.split('/').filter(|dir| !dir.is_empty()) {
        if !lower.is_empty() {
            lower.push('/');
        }
        lower.push_str(&dir.to_ascii_lowercase());
        folded = folders
            .entry(lower.clone())
            .or_insert_with(|| join(&folded, dir))
            .clone();
    }
    join(&folded, name)
}

fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

/// The entries an unpack did not write, by reason.
#[derive(Default)]
struct LeftOut {
    unsafe_names: usize,
    case_duplicates: usize,
    unreadable: usize,
    first_unreadable: Option<String>,
}

impl LeftOut {
    fn skip_unreadable(&mut self, rel: &str) {
        self.unreadable += 1;
        self.first_unreadable.get_or_insert_with(|| rel.to_owned());
    }

    fn summary_part(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.unsafe_names > 0 {
            parts.push(format!("{} unsafe name(s)", self.unsafe_names));
        }
        if self.case_duplicates > 0 {
            parts.push(format!("{} case duplicate(s)", self.case_duplicates));
        }
        if self.unreadable > 0 {
            let first = self.first_unreadable.as_deref().unwrap_or_default();
            parts.push(format!("{} unreadable (first: {first})", self.unreadable));
        }
        if parts.is_empty() {
            return None;
        }
        let total = self.unsafe_names + self.case_duplicates + self.unreadable;
        Some(format!("left out {total}: {}", parts.join(", ")))
    }
}
