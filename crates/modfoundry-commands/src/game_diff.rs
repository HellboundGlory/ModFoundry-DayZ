//! Compares two game builds and checks mods against the changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use modfoundry_gamediff::config::{config_diff, ClassRef, ConfigDiff, ConfigModel};
use modfoundry_gamediff::hits::{Hit, HitKind, Level, ModConfig, ModInput};
use modfoundry_gamediff::hits_data::{config_hits, layout_hits};
use modfoundry_gamediff::hits_script::script_hits;
use modfoundry_gamediff::layout::{
    layout_diff, LayoutDiff, LayoutFileChange, LayoutModel, WidgetChange, WidgetRef,
};
use modfoundry_gamediff::script::{Place, ScriptModel};
use modfoundry_gamediff::script_diff::{
    script_diff, Changes, ClassChange, EnumChange, ItemChange, ItemRef, MethodChange,
    MethodChangeKind, ScriptDiff,
};
use modfoundry_gamediff::{join_non_empty, ValueChange};
use serde::{Deserialize, Serialize};

use crate::config_host::{config_file_kind, pbo_configs, Kind};
use crate::game_side::{read_archives, read_folder, read_side, Inputs, FOLDER_DEPTH};
use crate::game_snapshot::{is_snapshot_path, load_snapshot, snapshot_side};
use crate::sources::{has_ext, mod_display_name};
use crate::walk::walk_files_to;

/// What `game.diff` compares and checks.
pub struct GameDiffRequest {
    pub old: String,
    pub new: String,
    pub against: Vec<PathBuf>,
    /// The saved builds folder; without one, only a snapshot `.json` or `.json.gz` path is read as a snapshot.
    pub snapshots: Option<PathBuf>,
}

/// One side as the report names it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideInfo {
    pub arg: String,
    pub kind: String,
    pub build: Option<String>,
    pub warnings: Vec<String>,
}

/// One mod that was checked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModInfo {
    pub name: String,
    pub path: PathBuf,
    pub scripts: usize,
    pub configs: usize,
    pub warnings: Vec<String>,
}

/// How many changes and hits the report holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub scripts: usize,
    pub configs: usize,
    pub layouts: usize,
    pub breaks: usize,
    pub check: usize,
}

/// Everything `game.diff` found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameDiffReport {
    pub old: SideInfo,
    pub new: SideInfo,
    pub scripts: ScriptDiff,
    pub configs: ConfigDiff,
    pub layouts: LayoutDiff,
    pub mods: Vec<ModInfo>,
    pub hits: Vec<Hit>,
    pub summary: Summary,
}

struct Side {
    info: SideInfo,
    scripts: ScriptModel,
    configs: ConfigModel,
    layouts: LayoutModel,
}

/// Compare two game builds and check `against` mods against the changes.
pub fn game_diff(request: &GameDiffRequest) -> Result<GameDiffReport, String> {
    let snapshots = request.snapshots.as_deref();
    let old = resolve_side(&request.old, snapshots, "old side")?;
    let new = resolve_side(&request.new, snapshots, "new side")?;
    let scripts = script_diff(&old.scripts, &new.scripts);
    let configs = config_diff(&old.configs, &new.configs);
    let layouts = layout_diff(&old.layouts, &new.layouts);

    let mut read = Vec::new();
    for root in &request.against {
        // Every read_mod error already names the folder.
        read.push(read_mod(root)?);
    }
    name_shared(&mut read);
    let mut mods = Vec::new();
    let mut hits = Vec::new();
    for (input, info) in read {
        let outlines = input.outlines();
        hits.extend(script_hits(
            &old.scripts,
            &new.scripts,
            &scripts,
            &input,
            &outlines,
        ));
        hits.extend(config_hits(&old.configs, &configs, &input));
        hits.extend(layout_hits(&layouts, &input, &outlines));
        mods.push(info);
    }
    hits.sort_by(hit_order);

    let summary = Summary {
        scripts: script_changes(&scripts),
        configs: counts(&configs.added, &configs.removed, &configs.changed),
        layouts: counts(&layouts.added, &layouts.removed, &layouts.changed),
        breaks: hits.iter().filter(|hit| hit.level == Level::Breaks).count(),
        check: hits.iter().filter(|hit| hit.level == Level::Check).count(),
    };
    Ok(GameDiffReport {
        old: old.info,
        new: new.info,
        scripts,
        configs,
        layouts,
        mods,
        hits,
        summary,
    })
}

fn hit_order(left: &Hit, right: &Hit) -> std::cmp::Ordering {
    left.level
        .cmp(&right.level)
        .then(left.diag_only.cmp(&right.diag_only))
        .then_with(|| left.mod_name.cmp(&right.mod_name))
        .then_with(|| left.place.file.cmp(&right.place.file))
        .then(left.place.line.cmp(&right.place.line))
        .then(left.kind.cmp(&right.kind))
}

fn resolve_side(arg: &str, snapshots: Option<&Path>, label: &str) -> Result<Side, String> {
    let snapshot = match snapshots {
        Some(dir) => snapshot_side(arg, dir),
        None => {
            let path = Path::new(arg);
            (path.is_file() && is_snapshot_path(path)).then(|| load_snapshot(path))
        }
    };
    if let Some(snapshot) = snapshot {
        let snapshot = snapshot.map_err(|err| format!("{label}: {err}"))?;
        return Ok(Side {
            info: SideInfo {
                arg: arg.to_owned(),
                kind: "snapshot".to_owned(),
                build: Some(snapshot.build),
                warnings: Vec::new(),
            },
            scripts: snapshot.scripts,
            configs: snapshot.configs,
            layouts: snapshot.layouts,
        });
    }
    let side = read_side(Path::new(arg)).map_err(|err| format!("{label}: {err}"))?;
    Ok(Side {
        info: SideInfo {
            arg: arg.to_owned(),
            kind: side.kind.as_str().to_owned(),
            build: side.build,
            warnings: side.warnings,
        },
        scripts: side.scripts,
        configs: side.configs,
        layouts: side.layouts,
    })
}

/// Read one mod folder: its loose scripts and configs, or its PBOs when it has none.
pub fn read_mod(root: &Path) -> Result<(ModInput, ModInfo), String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let mut warnings = Vec::new();
    let mut inputs = Inputs::default();
    // A mod's own files keep their paths on disk, where an agent opens them.
    let files = read_folder(
        root,
        true,
        |file| crate::walk::rel_path(root, file),
        &mut inputs,
        &mut warnings,
    );
    let mut configs = mod_configs(root, &files, &mut warnings);
    if inputs.scripts.is_empty() && configs.is_empty() {
        let (pbos, _) = walk_files_to(root, FOLDER_DEPTH, true, |_, name| {
            has_ext(Path::new(name), "pbo")
        });
        read_archives(&pbos, &mut inputs, &mut warnings);
        configs = pbo_configs(&pbos, &mut warnings)
            .into_iter()
            .map(|source| ModConfig {
                path: source.display,
                file: source.file,
                text: None,
            })
            .collect();
    }
    if inputs.scripts.is_empty() && configs.is_empty() {
        let mut error = format!("no scripts or configs under {}", root.display());
        for warning in &warnings {
            error.push_str(&format!("; {warning}"));
        }
        return Err(error);
    }
    let name = mod_display_name(root).unwrap_or_else(|| folder_name(root));
    let info = ModInfo {
        name: name.clone(),
        path: root.to_path_buf(),
        scripts: inputs.scripts.len(),
        configs: configs.len(),
        warnings,
    };
    let input = ModInput {
        name,
        scripts: inputs.scripts,
        configs,
    };
    Ok((input, info))
}

fn mod_configs(root: &Path, files: &[PathBuf], warnings: &mut Vec<String>) -> Vec<ModConfig> {
    let mut configs = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(kind) = config_file_kind(&name) else {
            continue;
        };
        let cpp = kind == Kind::Cpp;
        match read_mod_config(root, path, cpp, warnings) {
            Ok(config) => configs.push(config),
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
    configs
}

fn read_mod_config(
    root: &Path,
    path: &Path,
    cpp: bool,
    warnings: &mut Vec<String>,
) -> Result<ModConfig, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let rel = crate::walk::rel_path(root, path);
    if cpp {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let parsed = modfoundry_config::parser::parse(&text);
        // The parser recovers past an issue, as config.index reads it; keep what it read.
        if let Some(issue) = parsed.issues.first() {
            warnings.push(format!(
                "{}: {} parse issue(s), first: {}",
                path.display(),
                parsed.issues.len(),
                issue.message
            ));
        }
        Ok(ModConfig {
            path: rel,
            file: parsed.file,
            text: Some(text),
        })
    } else {
        Ok(ModConfig {
            path: rel,
            file: modfoundry_config::rap::read_rap(&bytes).map_err(|err| err.to_string())?,
            text: None,
        })
    }
}

fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// Hits are grouped by mod name, so two mods that share one each take their path as given.
pub(crate) fn name_shared(mods: &mut [(ModInput, ModInfo)]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (input, _) in mods.iter() {
        *counts.entry(input.name.clone()).or_default() += 1;
    }
    for (input, info) in mods {
        if counts[&input.name] > 1 {
            input.name = format!("{} ({})", input.name, info.path.display());
            info.name = input.name.clone();
        }
    }
}

/// The text report: the two sides, one section per model, the Diag-only items last,
/// the hits per mod, then the counts.
pub fn render_text(report: &GameDiffReport) -> String {
    let mut lines = vec![side_line("old", &report.old), side_line("new", &report.new)];
    let mut diag = Vec::new();
    for section in [
        script_section(&report.scripts),
        config_section(&report.configs),
        layout_section(&report.layouts),
    ] {
        if !section.items.is_empty() {
            lines.push(section.title.to_owned());
            lines.extend(section.items);
        }
        diag.extend(section.diag);
    }
    if !diag.is_empty() {
        lines.push("Diag only".to_owned());
        lines.extend(diag);
    }
    push_hits(&mut lines, &report.hits);
    for info in &report.mods {
        if !report.hits.iter().any(|hit| hit.mod_name == info.name) {
            lines.push(format!(
                "No hits in {} ({} scripts, {} configs)",
                info.name, info.scripts, info.configs
            ));
        }
    }
    lines.push(counts_line(&report.summary));
    lines.join("\n")
}

fn side_line(label: &str, side: &SideInfo) -> String {
    let build = side.build.as_deref().unwrap_or("no build");
    format!("{label}: {build} ({} {})", side.kind, side.arg)
}

fn counts_line(summary: &Summary) -> String {
    format!(
        "{} script, {} config and {} layout changes; {} breaks, {} to check",
        summary.scripts, summary.configs, summary.layouts, summary.breaks, summary.check,
    )
}

struct Section {
    title: &'static str,
    items: Vec<String>,
    diag: Vec<String>,
}

impl Section {
    fn new(title: &'static str) -> Self {
        Section {
            title,
            items: Vec::new(),
            diag: Vec::new(),
        }
    }

    fn push(&mut self, depth: usize, class: Option<&str>, text: &str, diag_only: bool) {
        if diag_only {
            let class = class
                .map(|name| format!("class {name}  "))
                .unwrap_or_default();
            self.diag.push(format!("  {}  {class}{text}", self.title));
        } else {
            self.items.push(format!("{}{text}", "  ".repeat(depth)));
        }
    }
}

fn script_section(diff: &ScriptDiff) -> Section {
    let mut section = Section::new("Scripts");
    push_added_removed(
        &mut section,
        1,
        None,
        "class",
        &diff.classes_added,
        &diff.classes_removed,
    );
    for change in &diff.classes_changed {
        push_class_change(&mut section, change);
    }
    push_method_changes(&mut section, &diff.functions, None, "method");
    push_item_changes(&mut section, &diff.globals, None, "global");
    push_enum_changes(&mut section, &diff.enums);
    push_item_changes(&mut section, &diff.typedefs, None, "typedef");
    section
}

fn push_added_removed(
    section: &mut Section,
    depth: usize,
    class: Option<&str>,
    noun: &str,
    added: &[ItemRef],
    removed: &[ItemRef],
) {
    for (what, items) in [("added", added), ("removed", removed)] {
        for item in items {
            let text = item_text(&format!("{noun} {what}"), item);
            section.push(depth, class, &text, item.diag_only);
        }
    }
}

fn push_class_change(section: &mut Section, change: &ClassChange) {
    let class = change.name.as_str();
    // Members that only Diag builds compile name their class in the Diag-only list, so a class with
    // nothing else changed gets no heading of its own.
    if change.diag_only || has_plain_change(change) {
        section.push(
            1,
            None,
            &parts(&["class changed", class, &place_text(change.place.as_ref())]),
            change.diag_only,
        );
    }
    for named in [&change.base, &change.conditions].into_iter().flatten() {
        section.push(2, Some(class), &named_value(named), change.diag_only);
    }
    push_method_changes(section, &change.methods, Some(class), "method");
    push_item_changes(section, &change.fields, Some(class), "field");
}

fn has_plain_change(change: &ClassChange) -> bool {
    let methods = &change.methods;
    let fields = &change.fields;
    change.base.is_some()
        || change.conditions.is_some()
        || methods
            .added
            .iter()
            .chain(&methods.removed)
            .any(|item| !item.diag_only)
        || methods.changed.iter().any(|item| !item.diag_only)
        || fields
            .added
            .iter()
            .chain(&fields.removed)
            .any(|item| !item.diag_only)
        || fields.changed.iter().any(|item| !item.diag_only)
}

/// A changed item's sub-line; in the Diag-only list it names its item, as it stands apart there.
fn sub_line(name: &str, change: &ValueChange, diag_only: bool) -> String {
    if diag_only {
        format!("{name}  {}", named_value(change))
    } else {
        named_value(change)
    }
}

fn push_method_changes(
    section: &mut Section,
    changes: &Changes<MethodChange>,
    class: Option<&str>,
    noun: &str,
) {
    let depth = member_depth(class);
    push_added_removed(
        section,
        depth,
        class,
        noun,
        &changes.added,
        &changes.removed,
    );
    for change in &changes.changed {
        let kinds: Vec<&str> = change
            .kinds
            .iter()
            .map(|kind| method_kind_word(*kind))
            .collect();
        let mut text = parts(&[
            &format!("{noun} changed"),
            &change.name,
            &place_text(Some(&change.place)),
            &kinds.join(", "),
        ]);
        if change.kinds.contains(&MethodChangeKind::Signature) {
            text.push_str(&format!(
                "  {} -> {}",
                change.old_signature, change.new_signature
            ));
        }
        section.push(depth, class, &text, change.diag_only);
    }
}

fn member_depth(class: Option<&str>) -> usize {
    if class.is_some() {
        2
    } else {
        1
    }
}

fn push_item_changes(
    section: &mut Section,
    changes: &Changes<ItemChange>,
    class: Option<&str>,
    noun: &str,
) {
    let depth = member_depth(class);
    push_added_removed(
        section,
        depth,
        class,
        noun,
        &changes.added,
        &changes.removed,
    );
    for change in &changes.changed {
        let head = parts(&[
            &format!("{noun} changed"),
            &change.name,
            &place_text(Some(&change.place)),
        ]);
        section.push(depth, class, &head, change.diag_only);
        for part in &change.changes {
            let line = sub_line(&change.name, part, change.diag_only);
            section.push(depth + 1, class, &line, change.diag_only);
        }
    }
}

fn push_enum_changes(section: &mut Section, changes: &Changes<EnumChange>) {
    push_added_removed(section, 1, None, "enum", &changes.added, &changes.removed);
    for change in &changes.changed {
        let head = parts(&[
            "enum changed",
            &change.name,
            &place_text(Some(&change.place)),
        ]);
        section.push(1, None, &head, change.diag_only);
        for named in change.base.iter().chain(&change.members) {
            let line = sub_line(&change.name, named, change.diag_only);
            section.push(2, None, &line, change.diag_only);
        }
    }
}

fn config_section(diff: &ConfigDiff) -> Section {
    let mut section = Section::new("Configs");
    for class in &diff.added {
        section.push(1, None, &class_ref_text("class added", class), false);
    }
    for class in &diff.removed {
        section.push(1, None, &class_ref_text("class removed", class), false);
    }
    for change in &diff.changed {
        let path = change.path.join("/");
        section.push(1, None, &parts(&["class changed", &path]), false);
        for named in change.parent.iter().chain(&change.properties) {
            section.push(2, None, &named_value(named), false);
        }
    }
    section
}

fn class_ref_text(what: &str, class: &ClassRef) -> String {
    parts(&[what, &class.path.join("/"), &class.sources.join(", ")])
}

fn layout_section(diff: &LayoutDiff) -> Section {
    let mut section = Section::new("Layouts");
    for path in &diff.added {
        section.push(1, None, &parts(&["file added", path]), false);
    }
    for path in &diff.removed {
        section.push(1, None, &parts(&["file removed", path]), false);
    }
    for change in &diff.changed {
        push_layout_change(&mut section, change);
    }
    section
}

fn push_layout_change(section: &mut Section, change: &LayoutFileChange) {
    let mut head = parts(&["file changed", &change.path]);
    if change.unparsed {
        head.push_str("  (not parsed)");
    }
    section.push(1, None, &head, false);
    for widget in &change.widgets_added {
        section.push(2, None, &widget_text("widget added", widget), false);
    }
    for widget in &change.widgets_removed {
        section.push(2, None, &widget_text("widget removed", widget), false);
    }
    for widget in &change.widgets_changed {
        push_widget_change(section, widget);
    }
}

fn widget_text(what: &str, widget: &WidgetRef) -> String {
    parts(&[
        what,
        &widget.path,
        &format!("{}:{}", widget.class, widget.line),
    ])
}

fn push_widget_change(section: &mut Section, change: &WidgetChange) {
    let head = parts(&["widget changed", &change.path, &change.line.to_string()]);
    section.push(2, None, &head, false);
    if let Some(class) = &change.class {
        section.push(3, None, &named_value(class), false);
    }
    for property in &change.properties {
        section.push(3, None, &named_value(property), false);
    }
}

fn push_hits(lines: &mut Vec<String>, hits: &[Hit]) {
    let mut blocks: Vec<(&str, Vec<&Hit>)> = Vec::new();
    for hit in hits {
        match blocks.iter_mut().find(|(name, _)| *name == hit.mod_name) {
            Some((_, block)) => block.push(hit),
            None => blocks.push((&hit.mod_name, vec![hit])),
        }
    }
    for (name, block) in blocks {
        lines.push(format!("Hits in {name}"));
        for hit in block {
            let mut line = parts(&[
                level_word(hit.level),
                &format!("{}:{}", hit.place.file, hit.place.line),
                kind_word(hit.kind),
                &format!("{}: {}", hit.vanilla, hit.detail),
            ]);
            if hit.maybe {
                line.push_str(" (maybe)");
            }
            if hit.diag_only {
                line.push_str(" (Diag only)");
            }
            lines.push(format!("  {line}"));
        }
    }
}

fn level_word(level: Level) -> &'static str {
    match level {
        Level::Breaks => "breaks",
        Level::Check => "check",
    }
}

fn method_kind_word(kind: MethodChangeKind) -> &'static str {
    match kind {
        MethodChangeKind::Signature => "signature",
        MethodChangeKind::ParamsRenamed => "params renamed",
        MethodChangeKind::Defaults => "defaults",
        MethodChangeKind::Body => "body",
    }
}

fn kind_word(kind: HitKind) -> &'static str {
    match kind {
        HitKind::ModdedClassRemoved => "modded class removed",
        HitKind::BaseClassRemoved => "base class removed",
        HitKind::BaseClassChanged => "base class changed",
        HitKind::OverrideRemoved => "override removed",
        HitKind::OverrideSignature => "override signature",
        HitKind::OverrideDefaults => "override defaults",
        HitKind::OverrideBody => "override body",
        HitKind::CallRemoved => "call removed",
        HitKind::CallSignature => "call signature",
        HitKind::ConfigParentRemoved => "config parent removed",
        HitKind::ConfigParentChanged => "config parent changed",
        HitKind::ConfigPatchRemoved => "config patch removed",
        HitKind::ConfigPatchChanged => "config patch changed",
        HitKind::LayoutRemoved => "layout removed",
        HitKind::LayoutChanged => "layout changed",
    }
}

fn item_text(what: &str, item: &ItemRef) -> String {
    parts(&[what, &item.name, &place_text(item.place.as_ref())])
}

fn named_value(change: &ValueChange) -> String {
    let before = change.old.as_deref().unwrap_or("(none)");
    let after = change.new.as_deref().unwrap_or("(none)");
    format!("{}: {before} -> {after}", change.name)
}

fn place_text(place: Option<&Place>) -> String {
    place
        .map(|place| format!("{}:{}", place.file, place.line))
        .unwrap_or_default()
}

fn parts(parts: &[&str]) -> String {
    join_non_empty(parts, "  ")
}

fn counts<A, B, C>(added: &[A], removed: &[B], changed: &[C]) -> usize {
    added.len() + removed.len() + changed.len()
}

fn change_count<T>(changes: &Changes<T>) -> usize {
    counts(&changes.added, &changes.removed, &changes.changed)
}

fn script_changes(diff: &ScriptDiff) -> usize {
    counts(
        &diff.classes_added,
        &diff.classes_removed,
        &diff.classes_changed,
    ) + change_count(&diff.functions)
        + change_count(&diff.globals)
        + change_count(&diff.enums)
        + change_count(&diff.typedefs)
}
