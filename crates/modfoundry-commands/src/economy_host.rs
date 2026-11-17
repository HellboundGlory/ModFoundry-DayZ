//! The session's economy index behind `ce.index`: the reference mission and the
//! workspace's CE files merged once into one pure [`Economy`], plus the
//! `ce.reference`, `ce.source` and cross-file `ce.validate` queries over it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use modfoundry_ce::economy::{
    cross_findings, include_findings, Economy, EconomySource, IncludeFact, Origin, Vocabulary,
};
use modfoundry_ce::kind::CeKind;
use modfoundry_ce::model::{self, Document};
use modfoundry_ce::schema::{self, KindSchema};
use modfoundry_core::{CommandError, Diagnostic};
use serde_json::{json, Value};

use crate::economy_load::{self, EconomyIndexRequest, LoadedFile, Reference, ReferenceOrigin};
use crate::latest::Latest;

/// Every loaded file of one economy index.
pub(crate) struct Loaded {
    pub reference: Option<Reference>,
    pub workspace: Vec<LoadedFile>,
    pub economy: Economy,
}

/// The session's economy index: loaded once and shared by every `ce.*` command.
pub struct EconomyHost {
    slot: Latest<Loaded>,
}

impl Default for EconomyHost {
    fn default() -> Self {
        Self::new()
    }
}

impl EconomyHost {
    pub fn new() -> Self {
        Self {
            slot: Latest::default(),
        }
    }

    /// Load `req` and return the index report. A newer call replaces the whole
    /// state; the returned report is always this load's.
    pub fn index(&self, req: EconomyIndexRequest) -> Value {
        let generation = self.slot.begin();
        let started = Instant::now();
        let mut warnings = Vec::new();
        let reference = economy_load::find_reference(&req, &mut warnings);
        let workspace = economy_load::load_workspace(&req.workspace, &mut warnings);
        let economy = build_economy(reference.as_ref(), &workspace);
        let report = report(&reference, &workspace, &economy, started, &warnings);
        self.slot.install(
            generation,
            Arc::new(Loaded {
                reference,
                workspace,
                economy,
            }),
        );
        report
    }

    /// The load a newer one hasn't superseded.
    pub(crate) fn loaded(&self) -> Option<Arc<Loaded>> {
        self.slot.get()
    }

    /// `ce.reference`: the reference's loaded records of `kind`, later files
    /// overriding earlier ones by key. Not ready before an index, or without a
    /// reference; a kind with no schema has no sections.
    pub fn reference(&self, kind: CeKind) -> Value {
        let loaded = self.loaded();
        let reference = loaded.as_ref().and_then(|loaded| loaded.reference.as_ref());
        let sections = match (reference, schema::schema(kind)) {
            (Some(reference), Some(schema)) => reference_sections(reference, schema, kind),
            _ => Vec::new(),
        };
        json!({
            "ready": reference.is_some(),
            "mission": mission_json(reference),
            "kind": kind,
            "sections": sections,
        })
    }

    /// `ce.source`: one reference file by `mission:<rel>` id, read on demand
    /// when the index only listed it.
    pub fn source(&self, id: &str) -> Result<Value, CommandError> {
        let Some(loaded) = self.loaded() else {
            return Err(CommandError::Failed(
                "the economy index is not loaded".to_string(),
            ));
        };
        let Some(reference) = loaded.reference.as_ref() else {
            return Err(CommandError::Failed(
                "the economy index has no mission reference".to_string(),
            ));
        };
        let Some(file) = reference.files.iter().find(|file| file.id == id) else {
            return Err(unknown_source(id));
        };
        let text = match &file.text {
            Some(text) => text.clone(),
            None => {
                economy_load::read_reference_text(reference, file).map_err(CommandError::Failed)?
            }
        };
        Ok(json!({
            "id": file.id,
            "display": format!("{}/{}", reference.name, file.rel),
            "kind": file.document.as_ref().and_then(|document| document.kind),
            "text": text,
        }))
    }

    /// The cross-file findings of one validated document against the index. A
    /// limits or user-limits text rebuilds the economy with itself in place of
    /// its workspace source; an economy core resolves its `<ce>` includes on
    /// disk; every other file lints against the host's.
    pub(crate) fn cross(&self, path: &Path, document: &Document) -> Vec<Diagnostic> {
        if document.kind == Some(CeKind::EconomyCore) {
            return include_findings(document, &include_facts(path, document));
        }
        let Some(loaded) = self.loaded() else {
            return Vec::new();
        };
        let overlaid;
        let economy = match document.kind {
            Some(CeKind::Limits) | Some(CeKind::LimitsUser) => {
                overlaid = overlaid_economy(&loaded, path, document);
                overlaid.as_ref().unwrap_or(&loaded.economy)
            }
            _ => &loaded.economy,
        };
        cross_findings(document, economy)
    }
}

/// The merged economy: the reference's documents (the two limits files first,
/// then `rel` order), then the workspace's in path order.
fn build_economy(reference: Option<&Reference>, workspace: &[LoadedFile]) -> Economy {
    build_economy_with(reference, workspace, |file| file.document.as_ref())
}

fn build_economy_with<'a>(
    reference: Option<&'a Reference>,
    workspace: &'a [LoadedFile],
    pick: impl Fn(&'a LoadedFile) -> Option<&'a Document>,
) -> Economy {
    let mut sources = Vec::new();
    if let Some(reference) = reference {
        for file in reference_load_order(reference) {
            if let Some(document) = &file.document {
                sources.push(EconomySource {
                    id: &file.id,
                    origin: Origin::Reference,
                    document,
                });
            }
        }
    }
    for file in workspace {
        if let Some(document) = pick(file) {
            sources.push(EconomySource {
                id: &file.id,
                origin: Origin::Workspace,
                document,
            });
        }
    }
    Economy::build(&sources)
}

fn reference_load_order(reference: &Reference) -> Vec<&LoadedFile> {
    let mut files: Vec<&LoadedFile> = reference.files.iter().collect();
    files.sort_by_key(|file| match file.rel.as_str() {
        "cfglimitsdefinition.xml" => 0,
        "cfglimitsdefinitionuser.xml" => 1,
        _ => 2,
    });
    files
}

/// The reference's files in the order a later record overrides an earlier one:
/// base files first, then the `<ce>` includes in `cfgeconomycore.xml` order.
fn reference_files_in_order(reference: &Reference) -> Vec<&LoadedFile> {
    let includes = reference
        .files
        .iter()
        .find(|file| file.rel == "cfgeconomycore.xml")
        .and_then(|file| file.text.as_deref())
        .map(economy_load::economy_core_includes)
        .unwrap_or_default();
    let mut bundled: Vec<&LoadedFile> = reference
        .files
        .iter()
        .filter(|file| file.declared.is_some())
        .collect();
    bundled.sort_by_key(|file| {
        includes
            .iter()
            .position(|(rel, _)| rel == &file.rel)
            .unwrap_or(usize::MAX)
    });
    let mut files: Vec<&LoadedFile> = reference
        .files
        .iter()
        .filter(|file| file.declared.is_none())
        .collect();
    files.extend(bundled);
    files
}

struct MergedSection {
    id: &'static str,
    label: &'static str,
    records: Vec<Value>,
    index: HashMap<String, usize>,
}

fn reference_sections(reference: &Reference, schema: &KindSchema, kind: CeKind) -> Vec<Value> {
    let mut sections: Vec<MergedSection> = schema
        .sections
        .iter()
        .map(|section| MergedSection {
            id: section.id,
            label: section.label,
            records: Vec::new(),
            index: HashMap::new(),
        })
        .collect();
    for file in reference_files_in_order(reference) {
        let Some(document) = &file.document else {
            continue;
        };
        if document.kind != Some(kind) {
            continue;
        }
        for section in &document.sections {
            let Some(target) = sections.iter_mut().find(|target| target.id == section.id) else {
                continue;
            };
            for record in &section.records {
                let mut value = json!(record);
                value["file"] = json!(file.id);
                let key = record.key.to_ascii_lowercase();
                match target.index.get(&key).copied() {
                    Some(position) => target.records[position] = value,
                    None => {
                        if !key.is_empty() {
                            target.index.insert(key, target.records.len());
                        }
                        target.records.push(value);
                    }
                }
            }
        }
    }
    sections
        .into_iter()
        .map(|section| {
            json!({
                "id": section.id,
                "label": section.label,
                "records": section.records,
            })
        })
        .collect()
}

fn unknown_source(id: &str) -> CommandError {
    CommandError::InvalidArguments(format!("unknown economy source `{id}`"))
}

/// The `<ce>` includes of an economy core document, resolved against the
/// document's own folder: each `<file>` becomes a fact with the folder and name
/// it declares, whether the folder and the file exist, and the kind its content
/// detects.
fn include_facts(path: &Path, document: &Document) -> Vec<IncludeFact> {
    let mut facts = Vec::new();
    let Some(section) = document.section("ce") else {
        return facts;
    };
    let base = path.parent();
    for record in &section.records {
        let Some(list) = record.list("file") else {
            continue;
        };
        let folder_exists = base.is_some_and(|base| base.join(&record.key).is_dir());
        for item in &list.items {
            let Some(attr) = item.attr("name") else {
                continue;
            };
            if attr.value.is_empty() {
                continue;
            }
            let resolved = base.map(|base| base.join(&record.key).join(&attr.value));
            let (exists, kind) = match resolved.as_deref() {
                Some(file) if file.is_file() => {
                    let text = std::fs::read_to_string(file).unwrap_or_default();
                    (
                        true,
                        model::read(&text, file.file_name().and_then(|name| name.to_str())).kind,
                    )
                }
                _ => (false, None),
            };
            facts.push(IncludeFact {
                folder: record.key.clone(),
                name: attr.value.clone(),
                folder_exists,
                exists,
                kind,
            });
        }
    }
    facts
}

/// The economy rebuilt with `document` in place of the workspace source `path`.
/// `None` when `path` is not a workspace source of this index.
fn overlaid_economy(loaded: &Loaded, path: &Path, document: &Document) -> Option<Economy> {
    let target = path.to_string_lossy();
    let matches = |file: &LoadedFile| {
        file.id.as_str() == target.as_ref() || file.path.as_deref() == Some(path)
    };
    if !loaded.workspace.iter().any(&matches) {
        return None;
    }
    Some(build_economy_with(
        loaded.reference.as_ref(),
        &loaded.workspace,
        |file| {
            if matches(file) {
                Some(document)
            } else {
                file.document.as_ref()
            }
        },
    ))
}

fn report(
    reference: &Option<Reference>,
    workspace: &[LoadedFile],
    economy: &Economy,
    started: Instant,
    warnings: &[String],
) -> Value {
    let mut files = Vec::new();
    if let Some(reference) = reference {
        for file in &reference.files {
            files.push(file_row(file, "reference"));
        }
    }
    for file in workspace {
        files.push(file_row(file, "workspace"));
    }
    json!({
        "mission": mission_json(reference.as_ref()),
        "files": files,
        "vocabulary": vocabulary_json(&economy.vocabulary),
        "types": economy.type_count(),
        "millis": started.elapsed().as_millis() as u64,
        "warnings": warnings,
    })
}

fn mission_json(reference: Option<&Reference>) -> Value {
    match reference {
        Some(reference) => json!({
            "name": reference.name,
            "path": reference.location,
            "origin": origin_name(reference.origin),
        }),
        None => Value::Null,
    }
}

fn origin_name(origin: ReferenceOrigin) -> &'static str {
    match origin {
        ReferenceOrigin::Folder => "folder",
        ReferenceOrigin::Server => "server",
        ReferenceOrigin::Game => "game",
    }
}

fn file_row(file: &LoadedFile, origin: &str) -> Value {
    let (kind, records, fragment) = match &file.document {
        Some(document) => (
            document.kind,
            document
                .sections
                .iter()
                .map(|section| section.records.len())
                .sum::<usize>(),
            document.fragment,
        ),
        None => (None, 0, false),
    };
    json!({
        "id": file.id,
        "rel": file.rel,
        "path": file.path,
        "origin": origin,
        "kind": kind,
        "records": records,
        "fragment": fragment,
        "loaded": file.text.is_some(),
    })
}

fn vocabulary_json(vocabulary: &Vocabulary) -> Value {
    json!({
        "categories": vocabulary.categories,
        "tags": vocabulary.tags,
        "usage": vocabulary.usage,
        "value": vocabulary.value,
        "user_usage": vocabulary.user_usage,
        "user_value": vocabulary.user_value,
    })
}
