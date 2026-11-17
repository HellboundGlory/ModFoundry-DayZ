//! The economy model: many read CE documents merged into one pure [`Economy`],
//! which the cross-file lints MF-X130, MF-X131 and MF-X132 check a types file
//! against, MF-X310 and MF-X320 check spawnable types and presets against,
//! MF-X210 and MF-X220 check events and event spawns against, and MF-X410
//! checks a user limits file against the limits definitions. [`include_findings`] checks an economy core
//! file's `<ce>` includes against the facts of what is on disk.

use crate::fixes::did_you_mean;
use crate::kind::CeKind;
use crate::lints::{nearest, span_start};
use crate::model::{Document, ListItem, Record};
use crate::schema::Vocab;
use modfoundry_core::{Diagnostic, Severity, Span};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Where an economy source came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Reference,
    Workspace,
}

/// One read CE document offered to [`Economy::build`].
pub struct EconomySource<'a> {
    pub id: &'a str,
    pub origin: Origin,
    pub document: &'a Document,
}

/// A mission-defined `user` flag and the base names it lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFlag {
    pub name: String,
    pub names: Vec<String>,
}

/// The names the economy defines, in source order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocabulary {
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub usage: Vec<String>,
    pub value: Vec<String>,
    pub user_usage: Vec<UserFlag>,
    pub user_value: Vec<UserFlag>,
    /// Each preset as its record element (`cargo` or `attachments`) and its name.
    pub presets: Vec<(String, String)>,
    /// Event names, in source order.
    pub events: Vec<String>,
}

/// The result of looking a name up in a [`Vocabulary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<'a> {
    Exact,
    CaseOnly(&'a str),
    Missing { nearest: Option<&'a str> },
}

impl Vocabulary {
    /// The defined names for `vocab`, in source order.
    pub fn names(&self, vocab: Vocab) -> Vec<&str> {
        self.each_name(vocab).collect()
    }

    fn each_name(&self, vocab: Vocab) -> Box<dyn Iterator<Item = &str> + '_> {
        match vocab {
            Vocab::Category => Box::new(self.categories.iter().map(String::as_str)),
            Vocab::Tag => Box::new(self.tags.iter().map(String::as_str)),
            Vocab::Usage => Box::new(self.usage.iter().map(String::as_str)),
            Vocab::Value => Box::new(self.value.iter().map(String::as_str)),
            Vocab::UserUsage => Box::new(self.user_usage.iter().map(|flag| flag.name.as_str())),
            Vocab::UserValue => Box::new(self.user_value.iter().map(|flag| flag.name.as_str())),
            Vocab::CargoPreset => self.preset_names("cargo"),
            Vocab::AttachmentsPreset => self.preset_names("attachments"),
            Vocab::Event => Box::new(self.events.iter().map(String::as_str)),
        }
    }

    fn preset_names(&self, element: &'static str) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.presets
                .iter()
                .filter(move |(preset_element, _)| preset_element == element)
                .map(|(_, name)| name.as_str()),
        )
    }

    /// Looks `name` up in `vocab`, case-insensitively.
    pub fn find(&self, vocab: Vocab, name: &str) -> Lookup<'_> {
        if self.each_name(vocab).any(|known| known == name) {
            return Lookup::Exact;
        }
        if let Some(known) = self
            .each_name(vocab)
            .find(|known| known.eq_ignore_ascii_case(name))
        {
            return Lookup::CaseOnly(known);
        }
        Lookup::Missing {
            nearest: nearest(name, &self.names(vocab)),
        }
    }
}

/// One `<ce>` include resolved on disk: the folder and name the entry points
/// at, whether the folder and the file exist, and its detected kind when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeFact {
    pub folder: String,
    pub name: String,
    pub folder_exists: bool,
    pub exists: bool,
    pub kind: Option<CeKind>,
}

/// The merged economy: the vocabulary and the type names, lowercased.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economy {
    pub vocabulary: Vocabulary,
    pub has_limits: bool,
    pub has_presets: bool,
    pub has_events: bool,
    types: BTreeSet<String>,
}

impl Economy {
    /// Merges `sources` in the order given (reference files first).
    pub fn build(sources: &[EconomySource<'_>]) -> Economy {
        let mut economy = Economy::default();
        for source in sources {
            match source.document.kind {
                Some(CeKind::Limits) => {
                    economy.has_limits = true;
                    push_names(&mut economy.vocabulary.categories, source, "categories");
                    push_names(&mut economy.vocabulary.tags, source, "tags");
                    push_names(&mut economy.vocabulary.usage, source, "usageflags");
                    push_names(&mut economy.vocabulary.value, source, "valueflags");
                }
                Some(CeKind::LimitsUser) => {
                    push_users(
                        &mut economy.vocabulary.user_usage,
                        source,
                        "usageflags",
                        "usage",
                    );
                    push_users(
                        &mut economy.vocabulary.user_value,
                        source,
                        "valueflags",
                        "value",
                    );
                }
                Some(CeKind::Types) => {
                    let Some(section) = source.document.section("types") else {
                        continue;
                    };
                    for record in &section.records {
                        if record.key.is_empty() {
                            continue;
                        }
                        economy.types.insert(record.key.to_ascii_lowercase());
                    }
                }
                Some(CeKind::RandomPresets) => {
                    economy.has_presets = true;
                    let Some(section) = source.document.section("presets") else {
                        continue;
                    };
                    for record in &section.records {
                        if record.key.is_empty() {
                            continue;
                        }
                        let element = record.element.as_str();
                        if !matches!(element, "cargo" | "attachments") {
                            continue;
                        }
                        let presets = &mut economy.vocabulary.presets;
                        if presets.iter().any(|(known_element, name)| {
                            known_element.as_str() == element
                                && name.eq_ignore_ascii_case(&record.key)
                        }) {
                            continue;
                        }
                        presets.push((element.to_string(), record.key.clone()));
                    }
                }
                Some(CeKind::Events) => {
                    economy.has_events = true;
                    push_names(&mut economy.vocabulary.events, source, "events");
                }
                _ => {}
            }
        }
        economy
    }

    /// Whether any source defines a type named `name` (case-insensitively).
    pub fn has_type(&self, name: &str) -> bool {
        self.types.contains(&name.to_ascii_lowercase())
    }

    /// How many distinct type names the sources define.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }
}

/// The cross-file findings for `document` against `economy`: MF-X130, MF-X131
/// and MF-X132 for a types document, MF-X310 and MF-X320 for a spawnable types
/// or random presets document, MF-X210 for an events document, MF-X220 for an
/// event spawns document and MF-X410 for a user limits document; empty for any
/// other kind, or without the vocabulary a code needs.
pub fn cross_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let mut findings = match document.kind {
        Some(CeKind::Types) if economy.has_limits => types_cross_findings(document, economy),
        Some(CeKind::SpawnableTypes) => spawnable_findings(document, economy),
        Some(CeKind::RandomPresets) => preset_findings(document, economy),
        Some(CeKind::Events) if economy.type_count() > 0 => event_findings(document, economy),
        Some(CeKind::EventSpawns) if economy.has_events => spawn_event_findings(document, economy),
        Some(CeKind::LimitsUser) if economy.has_limits => limits_user_findings(document, economy),
        _ => return Vec::new(),
    };
    findings.sort_by_key(span_start);
    findings
}

fn limits_user_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    for (section_id, list_id, vocab, kind) in [
        ("usageflags", "usage", Vocab::Usage, "usage"),
        ("valueflags", "value", Vocab::Value, "value"),
    ] {
        let Some(section) = document.section(section_id) else {
            continue;
        };
        for record in &section.records {
            check_items(
                &mut findings,
                economy,
                record,
                list_id,
                vocab,
                kind,
                "MF-X410",
            );
        }
    }
    findings
}

fn types_cross_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let Some(section) = document.section("types") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        check_category(&mut findings, economy, record);
        check_items(
            &mut findings,
            economy,
            record,
            "tag",
            Vocab::Tag,
            "tag",
            "MF-X130",
        );
        check_items(
            &mut findings,
            economy,
            record,
            "usage",
            Vocab::Usage,
            "usage",
            "MF-X130",
        );
        check_items(
            &mut findings,
            economy,
            record,
            "value",
            Vocab::Value,
            "value",
            "MF-X130",
        );
        check_users(&mut findings, economy, record, "usage", Vocab::UserUsage);
        check_users(&mut findings, economy, record, "value", Vocab::UserValue);
    }
    findings
}

fn spawnable_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let Some(section) = document.section("spawnabletypes") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        if economy.has_presets {
            check_presets(
                &mut findings,
                economy,
                record,
                "cargo",
                Vocab::CargoPreset,
                "cargo preset",
            );
            check_presets(
                &mut findings,
                economy,
                record,
                "attachments",
                Vocab::AttachmentsPreset,
                "attachments preset",
            );
        }
        if economy.type_count() > 0 {
            push_missing_type(
                &mut findings,
                economy,
                &record.key,
                record.key_span.unwrap_or(record.span),
            );
            for list in ["cargo", "attachments"] {
                let Some(list) = record.list(list) else {
                    continue;
                };
                for item in &list.items {
                    check_item_names(&mut findings, economy, &item.nested);
                }
            }
        }
    }
    findings
}

fn preset_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    if economy.type_count() == 0 {
        return Vec::new();
    }
    let Some(section) = document.section("presets") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        let Some(list) = record.list("item") else {
            continue;
        };
        check_item_names(&mut findings, economy, &list.items);
    }
    findings
}

fn event_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let Some(section) = document.section("events") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        let Some(children) = record.list("children") else {
            continue;
        };
        for child in &children.items {
            let Some(attr) = child.attr("type") else {
                continue;
            };
            if attr.value.is_empty() || economy.has_type(&attr.value) {
                continue;
            }
            findings.push(Diagnostic::at(
                "MF-X210",
                Severity::Warning,
                attr.span,
                format!(
                    "`{}` has no types entry, so this event can't spawn it.",
                    attr.value
                ),
            ));
        }
    }
    findings
}

fn spawn_event_findings(document: &Document, economy: &Economy) -> Vec<Diagnostic> {
    let Some(section) = document.section("events") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        if record.key.is_empty() {
            continue;
        }
        if let Lookup::Missing { nearest } = economy.vocabulary.find(Vocab::Event, &record.key) {
            findings.push(Diagnostic::at(
                "MF-X220",
                Severity::Warning,
                record.key_span.unwrap_or(record.span),
                format!(
                    "`{}` has spawn positions but no event.{}",
                    record.key,
                    did_you_mean(nearest)
                ),
            ));
        }
    }
    findings
}

fn check_presets(
    findings: &mut Vec<Diagnostic>,
    economy: &Economy,
    record: &Record,
    list: &str,
    vocab: Vocab,
    kind: &str,
) {
    let Some(list) = record.list(list) else {
        return;
    };
    for item in &list.items {
        let Some(attr) = item.attr("preset") else {
            continue;
        };
        if attr.value.is_empty() {
            continue;
        }
        if let Lookup::Missing { nearest } = economy.vocabulary.find(vocab, &attr.value) {
            findings.push(Diagnostic::at(
                "MF-X310",
                Severity::Warning,
                attr.span,
                format!(
                    "`{}` isn't {} {kind} this mission defines.{}",
                    attr.value,
                    article(kind),
                    did_you_mean(nearest)
                ),
            ));
        }
    }
}

fn check_item_names(findings: &mut Vec<Diagnostic>, economy: &Economy, items: &[ListItem]) {
    for item in items {
        if let Some(attr) = item.attr("name") {
            push_missing_type(findings, economy, &attr.value, attr.span);
        }
        check_item_names(findings, economy, &item.nested);
    }
}

fn push_missing_type(findings: &mut Vec<Diagnostic>, economy: &Economy, name: &str, span: Span) {
    if name.is_empty() || economy.has_type(name) {
        return;
    }
    findings.push(Diagnostic::at(
        "MF-X320",
        Severity::Hint,
        span,
        format!("`{name}` has no types entry in this mission."),
    ));
}

fn push_names(list: &mut Vec<String>, source: &EconomySource<'_>, section: &str) {
    let Some(section) = source.document.section(section) else {
        return;
    };
    for record in &section.records {
        if record.key.is_empty()
            || list
                .iter()
                .any(|seen| seen.eq_ignore_ascii_case(&record.key))
        {
            continue;
        }
        list.push(record.key.clone());
    }
}

fn push_users(list: &mut Vec<UserFlag>, source: &EconomySource<'_>, section: &str, item: &str) {
    let Some(section) = source.document.section(section) else {
        return;
    };
    for record in &section.records {
        if record.key.is_empty() {
            continue;
        }
        list.push(UserFlag {
            name: record.key.clone(),
            names: record.names(item),
        });
    }
}

fn check_category(findings: &mut Vec<Diagnostic>, economy: &Economy, record: &Record) {
    let Some(field) = record.field("category") else {
        return;
    };
    let (Some(name), Some(span)) = (field.raw.as_deref(), field.span) else {
        return;
    };
    if !name.is_empty() {
        push_name(
            findings,
            economy,
            Vocab::Category,
            "MF-X130",
            "category",
            name,
            span,
        );
    }
}

fn check_items(
    findings: &mut Vec<Diagnostic>,
    economy: &Economy,
    record: &Record,
    list: &str,
    vocab: Vocab,
    kind: &str,
    code: &str,
) {
    let Some(list) = record.list(list) else {
        return;
    };
    for item in &list.items {
        let Some(attr) = item.attr("name") else {
            continue;
        };
        if !attr.value.is_empty() {
            push_name(findings, economy, vocab, code, kind, &attr.value, attr.span);
        }
    }
}

fn check_users(
    findings: &mut Vec<Diagnostic>,
    economy: &Economy,
    record: &Record,
    list: &str,
    vocab: Vocab,
) {
    let Some(list) = record.list(list) else {
        return;
    };
    for item in &list.items {
        let Some(attr) = item.attr("user") else {
            continue;
        };
        if attr.value.is_empty() {
            continue;
        }
        push_name(
            findings,
            economy,
            vocab,
            "MF-X131",
            "user flag",
            &attr.value,
            attr.span,
        );
    }
}

fn push_name(
    findings: &mut Vec<Diagnostic>,
    economy: &Economy,
    vocab: Vocab,
    code: &str,
    kind: &str,
    name: &str,
    span: Span,
) {
    match economy.vocabulary.find(vocab, name) {
        Lookup::Exact => {}
        Lookup::CaseOnly(defined) => findings.push(case_hint(name, defined, span)),
        Lookup::Missing { nearest } => findings.push(Diagnostic::at(
            code,
            Severity::Warning,
            span,
            format!(
                "`{name}` isn't a {kind} this mission defines.{}",
                did_you_mean(nearest)
            ),
        )),
    }
}

fn case_hint(name: &str, defined: &str, span: Span) -> Diagnostic {
    Diagnostic::at(
        "MF-X132",
        Severity::Hint,
        span,
        format!("`{name}` is defined as `{defined}`."),
    )
}

fn article(word: &str) -> &'static str {
    match word.chars().next() {
        Some('a' | 'e' | 'i' | 'o' | 'u' | 'A' | 'E' | 'I' | 'O' | 'U') => "an",
        _ => "a",
    }
}

/// The include findings for an economy core document against `facts`: MF-X500
/// for a `<ce>` folder or `<file>` that isn't on disk, and MF-X510 for a file
/// whose content is another kind than its `type`; empty for any other kind.
///
/// The folder is named once, in place of its files, when the facts say the
/// folder itself is missing.
pub fn include_findings(document: &Document, facts: &[IncludeFact]) -> Vec<Diagnostic> {
    if document.kind != Some(CeKind::EconomyCore) {
        return Vec::new();
    }
    let Some(section) = document.section("ce") else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for record in &section.records {
        let folder = record.key.as_str();
        let Some(list) = record.list("file") else {
            continue;
        };
        let matched: Vec<_> = list
            .items
            .iter()
            .filter_map(|item| include_fact(facts, folder, item))
            .collect();
        if !matched.is_empty() && matched.iter().all(|fact| !fact.folder_exists) {
            findings.push(Diagnostic::at(
                "MF-X500",
                Severity::Warning,
                record.key_span.unwrap_or(record.span),
                format!("The folder `{folder}` isn't there."),
            ));
            continue;
        }
        for item in &list.items {
            let Some(fact) = include_fact(facts, folder, item) else {
                continue;
            };
            if !fact.exists {
                if let Some(attr) = item.attr("name") {
                    findings.push(Diagnostic::at(
                        "MF-X500",
                        Severity::Warning,
                        attr.span,
                        format!("`{folder}/{}` isn't there.", fact.name),
                    ));
                }
                continue;
            }
            let Some(detected) = fact.kind else {
                continue;
            };
            let Some(attr) = item.attr("type") else {
                continue;
            };
            if CeKind::from_include_type(&attr.value) == Some(detected) {
                continue;
            }
            let Some(expected) = detected.include_type() else {
                findings.push(Diagnostic::at(
                    "MF-X510",
                    Severity::Warning,
                    attr.span,
                    format!(
                        "`{}` holds {}, which economy core can't load as a `<file>`.",
                        fact.name,
                        detected.label().to_ascii_lowercase()
                    ),
                ));
                continue;
            };
            findings.push(Diagnostic::at(
                "MF-X510",
                Severity::Warning,
                attr.span,
                format!(
                    "`{}` holds {}, but its type says `{}`. Write `{expected}`.",
                    fact.name,
                    detected.label().to_ascii_lowercase(),
                    attr.value
                ),
            ));
        }
    }
    findings.sort_by_key(span_start);
    findings
}

fn include_fact<'a>(
    facts: &'a [IncludeFact],
    folder: &str,
    item: &ListItem,
) -> Option<&'a IncludeFact> {
    let name = item.attr("name")?.value.as_str();
    facts
        .iter()
        .find(|fact| fact.folder == folder && fact.name == name)
}
