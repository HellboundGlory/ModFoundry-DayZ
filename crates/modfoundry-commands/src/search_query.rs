//! Answers a search question from a game build's index, keyword and meaning rankings fused.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use modfoundry_pbo::PboArchive;
use modfoundry_platform::app_cache_dir;
use modfoundry_search::chunk::Kind;
use modfoundry_search::embed::BgeSmall;
use modfoundry_search::embed::{query_text, Embed};
use modfoundry_search::model::{check_model, default_model_dir, MODEL_ID, REVISION};
use modfoundry_search::rank::{fuse, Bm25};
use modfoundry_search::store::{Index, Meta, StoreError, StoredHit};
use serde::Serialize;

use crate::prefix_marker::{disk_candidates, marked_folders};
use crate::search_index::{
    addon_pbos, build_key_checked, game_pbos, index_dir, pbo_prefix, sha256_hex,
};
use crate::walk::entry_path;

/// Hits each ranking contributes before the rankings are fused.
const RANK_DEPTH: usize = 100;
/// The reciprocal rank fusion constant.
const RRF_K: f32 = 60.0;

/// A question and the filters and size of its answer.
pub struct SearchQuery {
    pub question: String,
    pub kinds: Vec<Kind>,
    pub sources: Vec<String>,
    pub top: usize,
}

/// One piece of an answer: where it is, what it holds and whether its file changed since indexing.
#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub source: String,
    pub reference_only: bool,
    pub lines: [usize; 2],
    pub kind: Kind,
    pub class: Option<String>,
    pub method: Option<String>,
    pub build: String,
    pub snippet: String,
    pub score: f32,
    pub stale: bool,
}

/// The hits for one question, the index they came from, how long the query took and any warnings.
#[derive(Debug, Serialize)]
pub struct SearchQueryReport {
    pub index: PathBuf,
    pub build: String,
    pub hits: Vec<SearchHit>,
    pub millis: u64,
    pub warnings: Vec<String>,
}

/// Answers `query` for the game install at `game`, with the model in `model` or the cached one; never downloads.
pub fn run_search_query(
    game: PathBuf,
    query: SearchQuery,
    model: Option<PathBuf>,
) -> Result<SearchQueryReport, String> {
    let cache = app_cache_dir().ok_or("no cache folder: set XDG_CACHE_HOME or HOME")?;
    let mut warnings = Vec::new();
    let build = build_key_checked(&game, &game_pbos(&game), &mut warnings);
    let index = index_dir(&cache, &build).join("index.sqlite");
    if !index.is_file() {
        let mut error = format!("no search index for build {build}; run `mf search index` first");
        for warning in &warnings {
            error.push_str(&format!("; {warning}"));
        }
        return Err(error);
    }
    let model = model.unwrap_or_else(|| default_model_dir(&cache));
    check_model(&model)
        .map_err(|err| format!("{err}; run `mf search index` to fetch the model"))?;
    let embedder = BgeSmall::load(&model)?;
    let mut report = query_index(&index, &embedder, &query)?;
    report.warnings = warnings;
    Ok(report)
}

/// Answers `query` from the index file at `index`, embedding the question with `embed`.
pub fn query_index(
    index: &Path,
    embed: &dyn Embed,
    query: &SearchQuery,
) -> Result<SearchQueryReport, String> {
    let start = Instant::now();
    let opened = Index::open(index).map_err(|err| err.to_string())?;
    check_index_model(opened.meta())?;
    if opened.is_empty() {
        return Err("the index holds no pieces; rebuild with `mf search index`".into());
    }
    let vector = embed
        .embed(&[query_text(&query.question).as_str()])?
        .into_iter()
        .next()
        .unwrap_or_default();
    if vector.len() != opened.dims() {
        return Err(format!(
            "query vector has {} values, the index holds {}",
            vector.len(),
            opened.dims()
        ));
    }
    let candidates = candidates(&opened, query)?;
    let fused =
        fused_ranking(&opened, &candidates, &vector, query).map_err(|err| err.to_string())?;
    let stored: Vec<StoredHit> = fused
        .iter()
        .map(|(id, _)| opened.hit(*id))
        .collect::<Result<_, _>>()
        .map_err(|err| err.to_string())?;
    let fresh = fresh_flags(&stored);
    let build = opened.meta().build.clone();
    let hits = stored
        .into_iter()
        .zip(fused)
        .enumerate()
        .map(|(i, (hit, (_, score)))| SearchHit {
            path: hit.path,
            source: hit.source,
            reference_only: hit.reference_only,
            lines: hit.lines,
            kind: hit.kind,
            class: hit.class,
            method: hit.method,
            build: build.clone(),
            snippet: hit.snippet,
            score,
            stale: !fresh[i],
        })
        .collect();
    Ok(SearchQueryReport {
        index: index.to_path_buf(),
        build,
        hits,
        millis: start.elapsed().as_millis() as u64,
        warnings: Vec::new(),
    })
}

/// The keyword and meaning rankings over the candidates, fused and cut to `query.top`, as (chunk id, score).
fn fused_ranking(
    index: &Index,
    candidates: &[(i64, String)],
    vector: &[f32],
    query: &SearchQuery,
) -> Result<Vec<(i64, f32)>, StoreError> {
    let ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    let texts: Vec<&str> = candidates.iter().map(|(_, text)| text.as_str()).collect();
    let keyword: Vec<usize> = Bm25::new(&texts)
        .scores(&query.question, RANK_DEPTH)
        .into_iter()
        .map(|(doc, _)| ids[doc] as usize)
        .collect();
    let allowed: HashSet<i64> = ids.into_iter().collect();
    let meaning: Vec<usize> = index
        .nearest(vector, index.len())?
        .into_iter()
        .filter(|(id, _)| allowed.contains(id))
        .take(RANK_DEPTH)
        .map(|(id, _)| id as usize)
        .collect();
    Ok(
        fuse(&[keyword.as_slice(), meaning.as_slice()], RRF_K, query.top)
            .into_iter()
            .map(|(id, score)| (id as i64, score))
            .collect(),
    )
}

/// The chunks a query may answer from: those matching its kind and source filters, or all of them.
fn candidates(index: &Index, query: &SearchQuery) -> Result<Vec<(i64, String)>, String> {
    let texts = index.texts().map_err(|err| err.to_string())?;
    if query.kinds.is_empty() && query.sources.is_empty() {
        return Ok(texts);
    }
    let allowed = index
        .ids_where(&query.kinds, &query.sources)
        .map_err(|err| err.to_string())?;
    Ok(texts
        .into_iter()
        .filter(|(id, _)| allowed.contains(id))
        .collect())
}

/// Whether each hit's file still has the size and hash it had when indexed; a PBO is opened once per query.
fn fresh_flags(hits: &[StoredHit]) -> Vec<bool> {
    let mut fresh = vec![false; hits.len()];
    // Docs folders are always read as loose files, whatever they hold now.
    let mut by_root: BTreeMap<(&str, bool), Vec<usize>> = BTreeMap::new();
    for (i, hit) in hits.iter().enumerate() {
        let loose = hit.kind == Kind::Docs;
        by_root
            .entry((hit.root.as_str(), loose))
            .or_default()
            .push(i);
    }
    for ((root, loose), group) in by_root {
        let root = Path::new(root);
        let pbos = if loose { None } else { addon_pbos(root) };
        if !loose && root.is_file() {
            check_pbos(&[root.to_path_buf()], hits, &group, &mut fresh);
        } else if let Some(pbos) = pbos {
            check_pbos(&pbos, hits, &group, &mut fresh);
        } else {
            let mut marked = None;
            for i in group {
                let hit = &hits[i];
                let matches =
                    |path: PathBuf| fs::read(path).is_ok_and(|bytes| matches_stored(&bytes, hit));
                fresh[i] = matches(root.join(&hit.path))
                    || disk_candidates(
                        marked.get_or_insert_with(|| marked_folders(root)),
                        &hit.path,
                    )
                    .any(matches);
            }
        }
    }
    fresh
}

/// Marks the hits in `group` fresh when their entry in one of `pbos` matches the stored size and hash.
fn check_pbos(pbos: &[PathBuf], hits: &[StoredHit], group: &[usize], fresh: &mut [bool]) {
    let mut pending = group.to_vec();
    for pbo in pbos {
        if pending.is_empty() {
            break;
        }
        let Ok(archive) = PboArchive::open(pbo) else {
            continue;
        };
        let prefix = pbo_prefix(&archive);
        let entries: HashMap<String, usize> = archive
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let name = entry.name_str().replace('\\', "/");
                (entry_path(&prefix, &name), index)
            })
            .collect();
        pending.retain(|&i| {
            let Some(&entry) = entries.get(&hits[i].path) else {
                return true;
            };
            let matched = archive
                .read_entry(entry)
                .is_ok_and(|bytes| matches_stored(&bytes, &hits[i]));
            fresh[i] = matched;
            !matched
        });
    }
}

fn matches_stored(bytes: &[u8], hit: &StoredHit) -> bool {
    bytes.len() as u64 == hit.size && sha256_hex(bytes) == hit.hash
}

/// Refuses an index embedded with another model, since its vectors do not compare with this build's.
fn check_index_model(meta: &Meta) -> Result<(), String> {
    if meta.model == MODEL_ID && meta.revision == REVISION {
        return Ok(());
    }
    Err(format!(
        "index was built with {} at revision {}, this build uses {MODEL_ID} at {REVISION}; rebuild with `mf search index`",
        meta.model, meta.revision
    ))
}
