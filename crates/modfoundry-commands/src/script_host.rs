//! The session's script index behind the `script.*` query commands: loaded once
//! from vanilla/dependency/workspace sources, queried with the caller's unsaved
//! buffer overlaid.

use crate::script_index::{
    load_index_with_texts, load_workspace_with_texts, suggest_dependencies, IndexReport,
    IndexRequest, Loaded, Progress, ScriptSource,
};
use modfoundry_core::{text::line_col, CommandError, Diagnostic};
use modfoundry_enforce::Tree;
use modfoundry_script::{
    complete_at, extract, resolve_at, semantic_diagnostics, CompletionItem, CompletionKind,
    FileKey, Index, Layer, Location, Origin, SymbolInfo, SymbolKind,
};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Indexing,
    Ready,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Indexing => "indexing",
            State::Ready => "ready",
        }
    }
}

type Sources = HashMap<String, ScriptSource>;
type Texts = HashMap<String, Arc<str>>;

struct Inner {
    request: Option<IndexRequest>,
    index: Index,
    sources: Sources,
    texts: Texts,
    report: Option<IndexReport>,
    progress: Arc<Progress>,
    state: State,
    generation: u64,
}

/// The session's script index: loaded once at workspace open and shared by every
/// `script.*` command.
pub struct ScriptHost {
    inner: Mutex<Inner>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptHost {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                request: None,
                index: Index::new(),
                sources: Sources::new(),
                texts: Texts::new(),
                report: None,
                progress: Arc::new(Progress::new()),
                state: State::Idle,
                generation: 0,
            }),
        }
    }

    /// Rebuild the index for `req`: a workspace-only index is swapped in first
    /// (queries during indexing answer from it), then the full load replaces it.
    /// The full load runs without the lock held.
    pub fn index(&self, req: IndexRequest) -> Value {
        let (generation, progress) = self.begin(req.clone());
        let workspace = IndexRequest {
            workspace: req.workspace.clone(),
            dependencies: Vec::new(),
            game: None,
        };
        // Its own progress: the shared one reports the full load, so the chip
        // does not fall back to zero when the second pass starts.
        self.install(
            generation,
            load_workspace_with_texts(&workspace, &Progress::new()),
            false,
        );

        let loaded = load_index_with_texts(&req, &progress);
        let mut value = serde_json::to_value(&loaded.1).unwrap_or(Value::Null);
        self.install(generation, loaded, true);
        let suggested: Vec<Value> = suggest_dependencies()
            .into_iter()
            .filter(|(_, path)| !req.dependencies.contains(path))
            .map(|(name, path)| json!({ "name": name, "path": path }))
            .collect();
        value["suggested"] = json!(suggested);
        value
    }

    /// Starts a load for `req`; a later `begin` supersedes it. The workspace
    /// root is stored absolute and with `.`/`..` resolved so display paths can
    /// strip it: the loader keys every file by its absolute path.
    fn begin(&self, mut req: IndexRequest) -> (u64, Arc<Progress>) {
        let workspace = std::path::absolute(&req.workspace).unwrap_or(req.workspace);
        req.workspace = normalized(&workspace);
        let mut inner = self.lock();
        inner.generation += 1;
        inner.request = Some(req);
        inner.progress = Arc::new(Progress::new());
        inner.state = State::Indexing;
        (inner.generation, inner.progress.clone())
    }

    /// Swaps in a load's result unless a newer load has begun since.
    fn install(&self, generation: u64, loaded: Loaded, ready: bool) -> bool {
        let mut inner = self.lock();
        if inner.generation != generation {
            return false;
        }
        let (index, report, sources, texts) = loaded;
        inner.index = index;
        inner.sources = sources;
        inner.texts = texts;
        inner.report = Some(report);
        if ready {
            inner.state = State::Ready;
        }
        true
    }

    /// The index load's current state and progress (status chip).
    pub fn status(&self) -> Value {
        let inner = self.lock();
        let (files, classes, millis) = match &inner.report {
            Some(report) => (report.files, report.classes, report.millis),
            None => (0, 0, 0),
        };
        json!({
            "state": inner.state.name(),
            "fraction": inner.progress.fraction(),
            "files": files,
            "classes": classes,
            "millis": millis,
            "root": inner.request.as_ref().map(|r| r.workspace.clone()),
        })
    }

    /// Every indexed declaration of the script class `name`, matched exactly
    /// (Enforce is case-sensitive): non-`modded` first, then each `modded` link
    /// in load order. Empty until the index is `Ready`.
    pub fn find_class(&self, name: &str) -> Value {
        let inner = self.lock();
        let ready = inner.state == State::Ready;
        let locations = if ready {
            inner
                .index
                .class_chain(name)
                .into_iter()
                .map(|link| {
                    let location = Location {
                        file: link.key.id.clone(),
                        span: link.class.name_span,
                        origin: link.key.origin,
                        layer: link.key.layer,
                    };
                    let mut value = location_json(&inner, "", "", &location);
                    value["modded"] = json!(link.class.modded);
                    value
                })
                .collect()
        } else {
            Vec::new()
        };
        json!({ "ready": ready, "locations": locations })
    }

    fn resolution(&self, path: &str, text: &str, offset: usize) -> Option<Value> {
        self.with_overlay(path, text, |inner, key| {
            resolve_at(&inner.index, key, text, offset).map(|resolution| {
                json!({
                    "symbol": symbol_json(&resolution.symbol),
                    "targets": targets_json(inner, &key.id, text, &resolution.targets),
                })
            })
        })
    }

    /// Hover at `offset` in the live `text` of `path`: `null` when nothing
    /// resolves.
    pub fn hover(&self, path: &str, text: &str, offset: usize) -> Value {
        self.resolution(path, text, offset).unwrap_or(Value::Null)
    }

    /// The definition targets at `offset` in the live `text` of `path`, with the
    /// resolved symbol; empty targets and a null symbol when nothing resolves.
    pub fn definition(&self, path: &str, text: &str, offset: usize) -> Value {
        self.resolution(path, text, offset)
            .unwrap_or_else(|| json!({ "symbol": null, "targets": [] }))
    }

    /// Completion candidates at `offset` in the live `text` of `path`.
    pub fn complete(&self, path: &str, text: &str, offset: usize) -> Value {
        self.with_overlay(path, text, |inner, _| {
            let completion = complete_at(&inner.index, text, offset);
            json!({
                "items": completion.items.iter().map(completion_item_json).collect::<Vec<_>>(),
                "replace": { "start": completion.replace.start, "end": completion.replace.end },
                "inferred": completion.inferred,
                "truncated": completion.truncated,
            })
        })
    }

    /// The source a `Location.id` addresses: its text and whether it is
    /// read-only. Unknown ids are invalid arguments.
    pub fn source(&self, id: &str) -> Result<Value, CommandError> {
        let inner = self.lock();
        let source = inner.sources.get(id).ok_or_else(|| {
            CommandError::InvalidArguments(format!("unknown script source `{id}`"))
        })?;
        let read_only = inner.index.file(id).map(|(key, _)| key.origin) != Some(Origin::Workspace);
        let text = match inner.texts.get(id) {
            Some(text) => text.to_string(),
            None => match source {
                ScriptSource::File(path) => std::fs::read_to_string(path)
                    .map_err(|e| CommandError::Failed(format!("{}: {e}", path.display())))?,
                ScriptSource::PboEntry { .. } => {
                    return Err(CommandError::Failed(format!("no cached text for `{id}`")))
                }
            },
        };
        Ok(json!({
            "id": id,
            "display_path": source.display_path(),
            "text": text,
            "read_only": read_only,
        }))
    }

    /// Run `f` against the index with the live `text` of `path` overlaid, then
    /// put the file's indexed entry back: the overlay answers one query, and a
    /// discarded buffer must not leave its declarations in the session index. A
    /// `path` that is an indexed non-workspace id (a PBO entry or dependency
    /// file) keeps its read-only key and is not overlaid.
    fn with_overlay<T>(&self, path: &str, text: &str, f: impl FnOnce(&Inner, &FileKey) -> T) -> T {
        let mut inner = self.lock();
        let id = workspace_id(path);
        let previous = match inner.index.file(&id) {
            Some((key, _)) if key.origin != Origin::Workspace => {
                let key = key.clone();
                return f(&inner, &key);
            }
            Some((key, items)) => Some((key.clone(), items.clone())),
            None => None,
        };
        let key = FileKey {
            id: id.clone(),
            origin: Origin::Workspace,
            layer: workspace_layer(&inner.request, &id),
        };
        inner.index.set_file(key.clone(), extract(text));
        let out = f(&inner, &key);
        match previous {
            Some((previous_key, items)) => inner.index.set_file(previous_key, items),
            None => inner.index.remove_file(&id),
        }
        out
    }

    /// Semantic lints (MF-E3001/MF-W3002) for a workspace file: empty unless the
    /// index is ready and `path` lies under the indexed workspace root. `tree` and
    /// `source` are the caller's parse of the file's text.
    pub fn semantic_diagnostics(&self, path: &Path, tree: &Tree, source: &str) -> Vec<Diagnostic> {
        let inner = self.lock();
        if inner.state != State::Ready {
            return Vec::new();
        }
        let Some(request) = inner.request.as_ref() else {
            return Vec::new();
        };
        let Ok(root) = std::path::absolute(&request.workspace) else {
            return Vec::new();
        };
        let Ok(abs) = std::path::absolute(path) else {
            return Vec::new();
        };
        let Ok(rel) = abs.strip_prefix(&root) else {
            return Vec::new();
        };
        let key = FileKey {
            id: abs.to_string_lossy().into_owned(),
            origin: Origin::Workspace,
            layer: Layer::from_path(&rel.to_string_lossy()),
        };
        semantic_diagnostics(&inner.index, &key, tree, source)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The index key for a filesystem path: absolute, so it matches the loader's
/// keys. A `pbo:…` id is already the key.
fn workspace_id(path: &str) -> String {
    if path.starts_with("pbo:") {
        return path.to_owned();
    }
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

fn workspace_layer(request: &Option<IndexRequest>, id: &str) -> Option<Layer> {
    let rel = match request {
        Some(req) => Path::new(id)
            .strip_prefix(&req.workspace)
            .unwrap_or(Path::new(id)),
        None => Path::new(id),
    };
    Layer::from_path(&rel.to_string_lossy())
}

fn symbol_json(symbol: &SymbolInfo) -> Value {
    json!({
        "kind": kind_name(symbol.kind),
        "name": symbol.name,
        "container": symbol.container,
        "signature": symbol.signature,
        "doc": symbol.doc,
        "origin": symbol.origin.map(origin_name),
        "layer": symbol.layer.map(Layer::name),
    })
}

fn targets_json(inner: &Inner, queried: &str, overlay: &str, targets: &[Location]) -> Vec<Value> {
    targets
        .iter()
        .map(|location| location_json(inner, queried, overlay, location))
        .collect()
}

fn location_json(inner: &Inner, queried: &str, overlay: &str, location: &Location) -> Value {
    let display_path = location_display_path(inner, location);
    let (line, column) = target_text(inner, queried, overlay, &location.file)
        .map(|text| line_col(&text, location.span.start))
        .unwrap_or((1, 1));
    json!({
        "id": location.file,
        "display_path": display_path,
        "read_only": location.origin != Origin::Workspace,
        "span": { "start": location.span.start, "end": location.span.end },
        "line": line,
        "column": column,
        "origin": origin_name(location.origin),
        "dependency": match location.origin {
            Origin::Dependency(index) => json!(index),
            _ => Value::Null,
        },
        "layer": location.layer.map(Layer::name),
    })
}

/// The path a location is shown by: a workspace target relative to the index
/// request's root (`Scripts/4_World/…`), every other target as its source
/// names it.
fn location_display_path(inner: &Inner, location: &Location) -> String {
    if location.origin == Origin::Workspace {
        if let Some(relative) = inner
            .request
            .as_ref()
            .and_then(|request| relative_path(&request.workspace, &location.file))
        {
            return relative;
        }
    }
    inner
        .sources
        .get(&location.file)
        .map(ScriptSource::display_path)
        .unwrap_or_else(|| location.file.clone())
}

/// `id` relative to `root`, with `/` separators; `None` when `id` is outside.
fn relative_path(root: &Path, id: &str) -> Option<String> {
    let id = normalized(Path::new(id));
    id.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

/// `path` with `.` dropped and `..` collapsing the component before it, so a
/// root given as a relative path compares equal to the absolute ids the loader
/// records. A `..` above the root stays at the root, as the filesystem reads it.
pub(crate) fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The text of a target file: the overlay for the queried file, the cache for
/// non-workspace files, disk for every other workspace file.
fn target_text<'a>(
    inner: &'a Inner,
    queried: &str,
    overlay: &'a str,
    file: &str,
) -> Option<Cow<'a, str>> {
    if file == queried {
        return Some(Cow::Borrowed(overlay));
    }
    if let Some(text) = inner.texts.get(file) {
        return Some(Cow::Borrowed(&**text));
    }
    std::fs::read_to_string(file).ok().map(Cow::Owned)
}

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Vanilla => "vanilla",
        Origin::Dependency(_) => "dependency",
        Origin::Workspace => "workspace",
    }
}

fn completion_item_json(item: &CompletionItem) -> Value {
    json!({
        "label": item.label,
        "kind": completion_kind_name(item.kind),
        "detail": item.detail,
        "origin": item.origin.map(origin_name),
        "layer": item.layer.map(Layer::name),
    })
}

fn completion_kind_name(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Local => "local",
        CompletionKind::Param => "param",
        CompletionKind::Field => "field",
        CompletionKind::Method => "method",
        CompletionKind::Class => "class",
        CompletionKind::Enum => "enum",
        CompletionKind::EnumMember => "enum_member",
        CompletionKind::Typedef => "typedef",
        CompletionKind::Function => "function",
        CompletionKind::Keyword => "keyword",
    }
}

fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Class => "class",
        SymbolKind::Enum => "enum",
        SymbolKind::EnumMember => "enum_member",
        SymbolKind::Typedef => "typedef",
        SymbolKind::Method => "method",
        SymbolKind::Field => "field",
        SymbolKind::Local => "local",
        SymbolKind::Param => "param",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loaded(files: usize) -> Loaded {
        let report = IndexReport {
            sources: Vec::new(),
            files,
            classes: 0,
            millis: 0,
            warnings: Vec::new(),
        };
        (Index::new(), report, Sources::new(), Texts::new())
    }

    fn request() -> IndexRequest {
        IndexRequest {
            workspace: PathBuf::from("/ws"),
            dependencies: Vec::new(),
            game: None,
        }
    }

    #[test]
    fn a_superseded_index_load_never_replaces_the_newer_one() {
        let host = ScriptHost::new();
        let (older, _) = host.begin(request());
        let (newer, _) = host.begin(request());

        assert!(host.install(newer, loaded(2), true));
        assert!(!host.install(older, loaded(1), true));

        assert_eq!(host.status()["files"], 2);
        assert_eq!(host.status()["state"], "ready");
    }
}
