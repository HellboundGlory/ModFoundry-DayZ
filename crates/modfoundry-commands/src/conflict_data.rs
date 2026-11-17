//! What a set of mods' configs and packed files have in common: shared `CfgPatches` names and prefixes, config
//! entries several mods set, and file paths shipped twice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use modfoundry_config::ast::{ConfigClass, Member};
use modfoundry_config::model::{DayZConfig, PatchesEntry};
use modfoundry_core::diagnostic::{Diagnostic, Severity};
use modfoundry_gamediff::hits::ModConfig;
use modfoundry_pbo::PboArchive;
use serde::Serialize;

use crate::build_ops::default_name;
use crate::game_side::FOLDER_DEPTH;
use crate::prefix_marker::{is_prefix_marker, marker_prefix};
use crate::search_index::pbo_prefix;
use crate::sources::has_ext;
use crate::walk::{entry_path, rel_path, walk_files_to};

/// One thing several mods share, with every place it was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
    /// The mods involved, in load order.
    pub mods: Vec<String>,
    pub places: Vec<FindingPlace>,
}

/// Where one mod's side of a finding is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingPlace {
    #[serde(rename = "mod")]
    pub mod_name: String,
    /// The file, relative to the mod folder, or `<pbo>/<entry>` inside a PBO.
    pub path: String,
    /// One-based, when known.
    pub line: Option<usize>,
}

/// A mod's prefixes and shipped file paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Packing {
    pub addons: Vec<PackedAddon>,
    pub warnings: Vec<String>,
}

/// One addon a mod ships: a PBO, or a folder of loose files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedAddon {
    /// `/`-separated, no slash at either end, as written; empty when the PBO names none.
    pub prefix: String,
    /// Relative to the mod folder; empty for the mod folder itself.
    pub base: String,
    /// Entry paths inside the addon, `/`-separated.
    pub files: Vec<String>,
}

/// One mod as the data checks see it.
#[derive(Debug, Clone, Copy)]
pub struct DataMod<'a> {
    pub name: &'a str,
    pub configs: &'a [ModConfig],
    pub packing: &'a Packing,
}

/// The data side of a conflict check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DataConflicts {
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
}

/// Read a mod folder's prefixes and file paths: its PBO headers, or `$PBOPREFIX$` and loose files.
pub fn read_packing(root: &Path) -> Packing {
    let (files, skipped) = walk_files_to(root, FOLDER_DEPTH, true, |_, _| true);
    let mut packing = Packing::default();
    for folder in skipped {
        packing.warnings.push(format!(
            "{}: deeper than {FOLDER_DEPTH} folders, not read",
            folder.display()
        ));
    }
    let containers: Vec<&Path> = files
        .iter()
        .filter(|path| has_ext(path, "pbo") || has_ext(path, "ebo"))
        .map(PathBuf::as_path)
        .collect();
    if !containers.is_empty() {
        read_container_packing(root, &containers, &mut packing);
    } else {
        read_source_packing(root, &files, &mut packing);
    }
    packing
}

fn read_container_packing(root: &Path, containers: &[&Path], packing: &mut Packing) {
    for path in containers {
        if has_ext(path, "ebo") {
            packing.warnings.push(format!(
                "{}: .ebo is not supported (protected container)",
                path.display()
            ));
            continue;
        }
        match PboArchive::open(path) {
            Ok(archive) => packing.addons.push(PackedAddon {
                prefix: pbo_prefix(&archive),
                base: rel_path(root, path),
                files: archive
                    .entries()
                    .iter()
                    .map(|entry| entry.name_str().replace('\\', "/"))
                    .collect(),
            }),
            Err(err) => packing.warnings.push(format!("{}: {err}", path.display())),
        }
    }
}

fn read_source_packing(root: &Path, files: &[PathBuf], packing: &mut Packing) {
    let mut addons: Vec<PackedAddon> = vec![PackedAddon {
        prefix: default_name(root),
        base: String::new(),
        files: Vec::new(),
    }];
    let mut markers: Vec<&PathBuf> = Vec::new();
    let mut root_marked = false;
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !is_prefix_marker(&name) {
            continue;
        }
        markers.push(path);
        let base = parent_rel(root, path);
        match marker_prefix(path) {
            Some(prefix) if base.is_empty() && !root_marked => {
                root_marked = true;
                addons[0].prefix = prefix;
            }
            Some(prefix) => {
                if !addons.iter().any(|addon| addon.base == base) {
                    addons.push(PackedAddon {
                        prefix,
                        base,
                        files: Vec::new(),
                    });
                }
            }
            None => packing
                .warnings
                .push(format!("{}: no prefix in it", rel_path(root, path))),
        }
    }

    for path in files {
        if markers.contains(&path) || is_excluded(&rel_path(root, path)) {
            continue;
        }
        let rel = rel_path(root, path);
        let Some(owner) = addons
            .iter_mut()
            .filter(|addon| under(&rel, &addon.base))
            .max_by_key(|addon| addon.base.len())
        else {
            continue;
        };
        owner.files.push(entry_below(&rel, &owner.base));
    }

    addons.retain(|addon| !addon.files.is_empty());
    addons.sort_by(|a, b| a.base.cmp(&b.base));
    packing.addons.extend(addons);
}

/// The folder a file path is in, relative to the mod folder.
fn parent_rel(root: &Path, path: &Path) -> String {
    path.parent()
        .map(|parent| rel_path(root, parent))
        .unwrap_or_default()
}

/// Whether `file` (relative) sits in `folder` (relative; empty = the mod folder).
fn under(file: &str, folder: &str) -> bool {
    folder.is_empty()
        || file
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `file`'s path below `folder`, with `/` separators.
fn entry_below(file: &str, folder: &str) -> String {
    file.strip_prefix(folder)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(file)
        .to_string()
}

/// The names `mf build` leaves out of a packed addon.
fn is_excluded(rel: &str) -> bool {
    const TOP_FOLDERS: [&str; 4] = ["keys", "builds", "target", "workshop"];
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let first = rel.split('/').next().unwrap_or(rel);
    name.starts_with('.')
        || has_ext(Path::new(name), "biprivatekey")
        || TOP_FOLDERS
            .iter()
            .any(|folder| first.eq_ignore_ascii_case(folder))
        || (!rel.contains('/')
            && ["mod.cpp", "meta.cpp"]
                .iter()
                .any(|kept| name.eq_ignore_ascii_case(kept)))
}

/// The order the game loads `mods` in, as indexes into `mods`, starting from the given order.
pub fn load_order(mods: &[DataMod]) -> Vec<usize> {
    let requires = requirements(mods);
    let groups = components(&requires);
    let mut group_of = vec![0; mods.len()];
    for (group, members) in groups.iter().enumerate() {
        for &i in members {
            group_of[i] = group;
        }
    }
    let mut placed = vec![false; mods.len()];
    let mut order = Vec::with_capacity(mods.len());
    while order.len() < mods.len() {
        let ready = |i: usize| {
            !placed[i]
                && requires[i]
                    .iter()
                    .all(|&j| group_of[j] == group_of[i] || placed[j])
        };
        let Some(next) = (0..mods.len())
            .find(|&i| ready(i))
            .or_else(|| (0..mods.len()).find(|&i| !placed[i]))
        else {
            break;
        };
        placed[next] = true;
        order.push(next);
    }
    order
}

/// The strongly connected groups of the requirement graph, each in given order.
fn components(requires: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct Tarjan<'a> {
        needs: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, v: usize) {
            self.index[v] = Some(self.next);
            self.low[v] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on_stack[v] = true;
            for &w in &self.needs[v] {
                match self.index[w] {
                    None => {
                        self.visit(w);
                        self.low[v] = self.low[v].min(self.low[w]);
                    }
                    Some(w_index) if self.on_stack[w] => self.low[v] = self.low[v].min(w_index),
                    _ => {}
                }
            }
            if self.index[v] == Some(self.low[v]) {
                let mut members = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack[w] = false;
                    members.push(w);
                    if w == v {
                        break;
                    }
                }
                members.sort_unstable();
                self.out.push(members);
            }
        }
    }
    let mut tarjan = Tarjan {
        needs: requires,
        index: vec![None; requires.len()],
        low: vec![0; requires.len()],
        stack: Vec::new(),
        on_stack: vec![false; requires.len()],
        next: 0,
        out: Vec::new(),
    };
    for v in 0..requires.len() {
        if tarjan.index[v].is_none() {
            tarjan.visit(v);
        }
    }
    tarjan.out
}

fn patch_names(config: &ModConfig) -> Vec<PatchesEntry> {
    DayZConfig::from_file(&config.file).patches
}

/// For each mod, the other mods whose `CfgPatches` class names it requires, sorted.
fn requirements(mods: &[DataMod]) -> Vec<Vec<usize>> {
    let patches: Vec<Vec<PatchesEntry>> = mods
        .iter()
        .map(|m| m.configs.iter().flat_map(patch_names).collect())
        .collect();
    let declared: Vec<Vec<String>> = patches
        .iter()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.name.to_ascii_lowercase())
                .collect()
        })
        .collect();
    patches
        .iter()
        .enumerate()
        .map(|(i, entries)| {
            let wanted: Vec<String> = entries
                .iter()
                .flat_map(|entry| &entry.required_addons)
                .map(|name| name.to_ascii_lowercase())
                .collect();
            let mut needs: Vec<usize> = declared
                .iter()
                .enumerate()
                .filter(|&(j, names)| j != i && names.iter().any(|name| wanted.contains(name)))
                .map(|(j, _)| j)
                .collect();
            needs.sort_unstable();
            needs
        })
        .collect()
}

/// What the mods' configs and packed files share; `order` is [`load_order`]'s answer.
pub fn data_conflicts(mods: &[DataMod], order: &[usize]) -> DataConflicts {
    let requires = requirements(mods);
    let mut conflicts = DataConflicts {
        warnings: cycle_warnings(mods, order, &requires),
        findings: Vec::new(),
    };
    conflicts.findings.extend(shared_patch_names(mods, order));
    let (prefixes, reported) = shared_prefixes(mods, order);
    conflicts.findings.extend(prefixes);
    conflicts.findings.extend(nested_prefixes(mods, order));
    conflicts
        .findings
        .extend(shared_paths(mods, order, &reported));
    conflicts
        .findings
        .extend(shared_class_properties(mods, order, &requires));
    for (mod_name, packing) in mods
        .iter()
        .map(|m| (m.name, m.packing))
        .filter(|(_, packing)| !packing.warnings.is_empty())
    {
        for warning in &packing.warnings {
            conflicts.warnings.push(format!("{mod_name}: {warning}"));
        }
    }
    sort_findings(&mut conflicts.findings, mods, order);
    conflicts
}

/// Each required patch a mod names whose own mod the order still loads later.
fn cycle_warnings(mods: &[DataMod], order: &[usize], requires: &[Vec<usize>]) -> Vec<String> {
    let mut position = vec![0; mods.len()];
    for (at, &i) in order.iter().enumerate() {
        position[i] = at;
    }
    let mut pairs: Vec<(usize, usize)> = (0..mods.len())
        .flat_map(|i| requires[i].iter().map(move |&j| (i, j)))
        .filter(|&(i, j)| position[i] < position[j])
        .collect();
    pairs.sort_unstable();
    pairs
        .into_iter()
        .map(|(i, j)| {
            format!(
                "{} requires {}, which loads after it (requiredAddons cycle); the given order is kept for them",
                mods[i].name, mods[j].name
            )
        })
        .collect()
}

/// One error per `CfgPatches` class name two or more mods declare.
fn shared_patch_names(mods: &[DataMod], order: &[usize]) -> Vec<Finding> {
    let mut declared: Vec<Declaration> = Vec::new();
    for (index, m) in mods.iter().enumerate() {
        for config in m.configs.iter() {
            let newlines = newline_offsets(config);
            for patch in patch_names(config) {
                declared.push(Declaration {
                    key: patch.name.to_ascii_lowercase(),
                    spelling: patch.name,
                    module: index,
                    place: place(
                        m.name,
                        &config.path,
                        line_of(config, &newlines, patch.span.start),
                    ),
                });
            }
        }
    }
    let mut findings = Vec::new();
    for members in group_by_key(&declared, |d| d.key.as_str()) {
        let mut owners: Vec<usize> = members.iter().map(|d| d.module).collect();
        owners.dedup();
        let involved: Vec<usize> = order
            .iter()
            .copied()
            .filter(|i| owners.contains(i))
            .collect();
        if involved.len() < 2 {
            continue;
        }
        let names: Vec<&str> = involved.iter().map(|&i| mods[i].name).collect();
        findings.push(Finding {
            diagnostic: Diagnostic::new(
                "MF-M201",
                Severity::Error,
                format!(
                    "CfgPatches class \"{}\" is declared by {}",
                    members
                        .iter()
                        .find(|d| d.module == involved[0])
                        .map(|d| d.spelling.as_str())
                        .unwrap_or_default(),
                    names.join(", ")
                ),
            ),
            mods: names.iter().map(|n| n.to_string()).collect(),
            places: involved
                .iter()
                .flat_map(|&i| members.iter().filter(move |d| d.module == i))
                .map(|d| d.place.clone())
                .collect(),
        });
    }
    findings
}

struct Declaration {
    key: String,
    spelling: String,
    module: usize,
    place: FindingPlace,
}

/// An addon of a mod, with its prefix as written and the key two prefixes are compared by.
struct PrefixOwner<'a> {
    spelling: &'a str,
    key: String,
    module: usize,
    addon: &'a PackedAddon,
}

fn prefix_owners<'a>(mods: &[DataMod<'a>]) -> Vec<PrefixOwner<'a>> {
    mods.iter()
        .enumerate()
        .flat_map(|(module, m)| {
            m.packing.addons.iter().map(move |addon| PrefixOwner {
                spelling: &addon.prefix,
                key: addon.prefix.replace('\\', "/").to_ascii_lowercase(),
                module,
                addon,
            })
        })
        .collect()
}

/// The order `owners`' mods appear in, so a finding lists them as the game loads them.
fn in_load_order<'a, 'b>(
    owners: &'b [&PrefixOwner<'a>],
    order: &[usize],
) -> Vec<&'b PrefixOwner<'a>> {
    let mut sorted: Vec<&'b PrefixOwner<'a>> = owners.to_vec();
    sorted.sort_by_key(|owner| {
        (
            order
                .iter()
                .position(|&i| i == owner.module)
                .unwrap_or(usize::MAX),
            owner.addon.base.as_str(),
        )
    });
    sorted
}

fn addon_place(owner: &PrefixOwner, mods: &[DataMod]) -> FindingPlace {
    FindingPlace {
        mod_name: mods[owner.module].name.to_string(),
        path: if owner.addon.base.is_empty() {
            ".".to_string()
        } else {
            owner.addon.base.clone()
        },
        line: None,
    }
}

/// One error per non-empty prefix two or more mods use, and the keys it reported.
fn shared_prefixes(mods: &[DataMod], order: &[usize]) -> (Vec<Finding>, Vec<String>) {
    let owners = prefix_owners(mods);
    let mut keys: Vec<&str> = Vec::new();
    for owner in owners.iter().filter(|owner| !owner.key.is_empty()) {
        if !keys.contains(&owner.key.as_str()) {
            keys.push(&owner.key);
        }
    }
    let mut findings = Vec::new();
    let mut reported = Vec::new();
    for key in keys {
        let held: Vec<&PrefixOwner> = owners.iter().filter(|owner| owner.key == key).collect();
        let mut modules: Vec<usize> = held.iter().map(|owner| owner.module).collect();
        modules.dedup();
        if modules.len() < 2 {
            continue;
        }
        let sorted = in_load_order(&held, order);
        reported.push(key.to_string());
        findings.push(Finding {
            diagnostic: Diagnostic::new(
                "MF-M202",
                Severity::Error,
                format!(
                    "PBO prefix \"{}\" is used by {}",
                    sorted[0].spelling,
                    mod_names(&sorted, mods).join(", ")
                ),
            ),
            mods: mod_names(&sorted, mods),
            places: sorted
                .iter()
                .map(|owner| addon_place(owner, mods))
                .collect(),
        });
    }
    (findings, reported)
}

fn mod_names(owners: &[&PrefixOwner], mods: &[DataMod]) -> Vec<String> {
    names_once(owners.iter().map(|owner| owner.module), mods)
}

/// The mods' names in the order given, a run of one mod named once.
fn names_once(modules: impl Iterator<Item = usize>, mods: &[DataMod]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for module in modules {
        let name = mods[module].name.to_string();
        if names.last() != Some(&name) {
            names.push(name);
        }
    }
    names
}

/// One warning per (outer, inner) prefix pair where one mod's prefix sits inside another's.
fn nested_prefixes(mods: &[DataMod], order: &[usize]) -> Vec<Finding> {
    let owners = prefix_owners(mods);
    let mut pairs: Vec<(String, String)> = Vec::new();
    for inner in &owners {
        for outer in &owners {
            if inner.module == outer.module
                || inner.key.is_empty()
                || outer.key.is_empty()
                || inner.key == outer.key
                || !inner.key.starts_with(&format!("{}/", outer.key))
            {
                continue;
            }
            let pair = (outer.key.clone(), inner.key.clone());
            if !pairs.contains(&pair) {
                pairs.push(pair);
            }
        }
    }
    let mut findings = Vec::new();
    for (outer_key, inner_key) in pairs {
        let held: Vec<&PrefixOwner> = owners
            .iter()
            .filter(|owner| owner.key == outer_key || owner.key == inner_key)
            .collect();
        let sorted = in_load_order(&held, order);
        let Some((outer, inner)) = sorted
            .iter()
            .filter(|owner| owner.key == outer_key)
            .flat_map(|outer| {
                sorted
                    .iter()
                    .filter(|inner| inner.key == inner_key && inner.module != outer.module)
                    .map(move |inner| (outer, inner))
            })
            .next()
        else {
            continue;
        };
        findings.push(Finding {
            diagnostic: Diagnostic::new(
                "MF-M203",
                Severity::Warning,
                format!(
                    "PBO prefix \"{}\" of {} is inside \"{}\" of {}",
                    inner.spelling,
                    mods[inner.module].name,
                    outer.spelling,
                    mods[outer.module].name
                ),
            ),
            mods: mod_names(&sorted, mods),
            places: sorted
                .iter()
                .map(|owner| addon_place(owner, mods))
                .collect(),
        });
    }
    findings
}

/// One error per game path two or more mods pack, except under a prefix already reported twice.
fn shared_paths(mods: &[DataMod], order: &[usize], reported: &[String]) -> Vec<Finding> {
    let owners = prefix_owners(mods);
    let mut holders: Vec<(String, &PrefixOwner, String, String)> = Vec::new();
    for owner in &owners {
        let prefix = norm_prefix(owner.spelling);
        for entry in &owner.addon.files {
            let entry = entry.replace('\\', "/");
            let path = entry_path(&prefix, &entry);
            holders.push((path.to_ascii_lowercase(), owner, entry, path));
        }
    }
    let reported: Vec<String> = reported.iter().map(|prefix| format!("{prefix}/")).collect();
    let mut findings = Vec::new();
    for held in group_by_key(&holders, |(key, ..)| key.as_str()) {
        let key = &held[0].0;
        if reported
            .iter()
            .any(|prefix| key.starts_with(prefix.as_str()))
        {
            continue;
        }
        let modules: std::collections::BTreeSet<usize> =
            held.iter().map(|(_, owner, _, _)| owner.module).collect();
        if modules.len() < 2 {
            continue;
        }
        let mut sorted = held.clone();
        sorted.sort_by_key(|(_, owner, _, _)| {
            order
                .iter()
                .position(|&i| i == owner.module)
                .unwrap_or(usize::MAX)
        });
        let names = names_once(sorted.iter().map(|(_, owner, _, _)| owner.module), mods);
        findings.push(Finding {
            diagnostic: Diagnostic::new(
                "MF-M204",
                Severity::Error,
                format!("{} is packed by {}", sorted[0].3, names.join(", ")),
            ),
            mods: names,
            places: sorted
                .iter()
                .map(|(_, owner, entry, _)| FindingPlace {
                    mod_name: mods[owner.module].name.to_string(),
                    path: entry_path(&owner.addon.base, entry),
                    line: None,
                })
                .collect(),
        });
    }
    findings
}

/// One property a class sets, with the mod that sets it and where.
struct SetProperty {
    property: String,
    spelling: String,
    place: FindingPlace,
}

/// One class a mod's configs declare, with every property it sets or deletes.
struct SetClass {
    key: String,
    path: String,
    module: usize,
    properties: Vec<SetProperty>,
}

fn set_classes(mods: &[DataMod]) -> Vec<SetClass> {
    let mut classes = Vec::new();
    for (module, m) in mods.iter().enumerate() {
        for config in m.configs.iter() {
            let newlines = newline_offsets(config);
            let mut path = Vec::new();
            walk_classes(
                &config.file.members,
                &mut path,
                &mut |class_path: &[String], class: &ConfigClass| {
                    if class.forward
                        || class_path.len() < 2
                        || class_path[0].eq_ignore_ascii_case("CfgPatches")
                    {
                        return;
                    }
                    let place = place(
                        m.name,
                        &config.path,
                        line_of(config, &newlines, class.span.start),
                    );
                    let properties = class
                        .members
                        .iter()
                        .filter_map(|member| match member {
                            Member::Property(property) if !property.array_append => {
                                Some(property.name.as_str())
                            }
                            Member::Delete { name, .. } => Some(name.as_str()),
                            _ => None,
                        })
                        .map(|name| SetProperty {
                            property: name.to_ascii_lowercase(),
                            spelling: name.to_string(),
                            place: place.clone(),
                        })
                        .collect();
                    classes.push(SetClass {
                        key: class_path.join("/").to_ascii_lowercase(),
                        path: class_path.join("/"),
                        module,
                        properties,
                    });
                },
            );
        }
    }
    classes
}

fn walk_classes(
    members: &[Member],
    path: &mut Vec<String>,
    visit: &mut impl FnMut(&[String], &ConfigClass),
) {
    for member in members {
        if let Member::Class(class) = member {
            path.push(class.name.clone());
            visit(path, class);
            walk_classes(&class.members, path, visit);
            path.pop();
        }
    }
}

/// One finding per class two or more mods set the same property on, with the last setter winning.
fn shared_class_properties(
    mods: &[DataMod],
    order: &[usize],
    requires: &[Vec<usize>],
) -> Vec<Finding> {
    let classes = set_classes(mods);
    let mut findings = Vec::new();
    for members in group_by_key(&classes, |class| class.key.as_str()) {
        let properties = conflicting_properties(&members);
        if properties.is_empty() {
            continue;
        }
        let Some(finding) = class_finding(mods, order, requires, &members, &properties) else {
            continue;
        };
        findings.push(finding);
    }
    findings
}

/// The lower-cased property names two or more distinct mods set or delete.
fn conflicting_properties(members: &[&SetClass]) -> Vec<String> {
    let mut properties: Vec<String> = Vec::new();
    for class in members {
        for property in &class.properties {
            if properties.contains(&property.property) {
                continue;
            }
            let setters: std::collections::BTreeSet<usize> = members
                .iter()
                .filter(|other| {
                    other
                        .properties
                        .iter()
                        .any(|p| p.property == property.property)
                })
                .map(|other| other.module)
                .collect();
            if setters.len() > 1 {
                properties.push(property.property.clone());
            }
        }
    }
    properties.sort();
    properties
}

fn class_finding(
    mods: &[DataMod],
    order: &[usize],
    requires: &[Vec<usize>],
    members: &[&SetClass],
    properties: &[String],
) -> Option<Finding> {
    let sets_conflict = |class: &SetClass| {
        class
            .properties
            .iter()
            .any(|property| properties.contains(&property.property))
    };
    let involved: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| {
            members
                .iter()
                .any(|class| class.module == i && sets_conflict(class))
        })
        .collect();
    if involved.len() < 2 {
        return None;
    }
    let setter = |module: usize, property: &str| {
        members
            .iter()
            .rev()
            .filter(|class| class.module == module)
            .find_map(|class| {
                class
                    .properties
                    .iter()
                    .rev()
                    .find(|p| p.property == property)
            })
    };
    let mut described = Vec::new();
    for property in properties {
        let winner = *involved
            .iter()
            .rev()
            .find(|&&i| setter(i, property).is_some())?;
        let spelling = setter(winner, property)
            .map(|p| p.spelling.as_str())
            .unwrap_or(property);
        described.push(format!("{spelling} ({} wins)", mods[winner].name));
    }
    let path = members
        .first()
        .map(|class| class.path.as_str())
        .unwrap_or_default();
    let deliberate = involved
        .windows(2)
        .all(|pair| reaches(requires, pair[1], pair[0]));
    let code = if deliberate { "MF-M206" } else { "MF-M205" };
    let severity = if deliberate {
        Severity::Hint
    } else {
        Severity::Warning
    };
    Some(Finding {
        diagnostic: Diagnostic::new(
            code,
            severity,
            format!("{path}: {} set by more than one mod", described.join(", ")),
        ),
        mods: involved.iter().map(|&i| mods[i].name.to_string()).collect(),
        places: involved
            .iter()
            .map(|&i| {
                members
                    .iter()
                    .find(|class| class.module == i && sets_conflict(class))
                    .map(|class| class.properties[0].place.clone())
                    .unwrap_or_else(|| place(mods[i].name, "", None))
            })
            .collect(),
    })
}

/// Whether mod `from` requires mod `to`, directly or through other mods.
fn reaches(requires: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; requires.len()];
    let mut stack = vec![from];
    while let Some(next) = stack.pop() {
        if next == to {
            return true;
        }
        if std::mem::replace(&mut seen[next], true) {
            continue;
        }
        stack.extend_from_slice(&requires[next]);
    }
    false
}

/// A prefix with `/` separators and no slash at either end.
fn norm_prefix(prefix: &str) -> String {
    prefix.replace('\\', "/").trim_matches('/').to_string()
}

fn place(mod_name: &str, path: &str, line: Option<usize>) -> FindingPlace {
    FindingPlace {
        mod_name: mod_name.to_string(),
        path: path.to_string(),
        line,
    }
}

/// A config's newline offsets, so a class's line is one binary search.
fn newline_offsets(config: &ModConfig) -> Vec<usize> {
    config
        .text
        .as_deref()
        .map(|text| text.match_indices('\n').map(|(at, _)| at).collect())
        .unwrap_or_default()
}

/// The one-based line of byte `at`; `None` for a `config.bin`.
fn line_of(config: &ModConfig, newlines: &[usize], at: usize) -> Option<usize> {
    config
        .text
        .as_ref()
        .map(|_| newlines.partition_point(|&newline| newline < at) + 1)
}

/// Findings sorted by code, then the first place's mod position, path and line, then the message.
fn sort_findings(findings: &mut [Finding], mods: &[DataMod], order: &[usize]) {
    let position = |name: &str| {
        order
            .iter()
            .position(|&i| mods[i].name == name)
            .unwrap_or(usize::MAX)
    };
    findings.sort_by(|a, b| {
        let a_first = a.places.first();
        let b_first = b.places.first();
        a.diagnostic
            .code
            .cmp(&b.diagnostic.code)
            .then_with(|| {
                a_first
                    .map(|p| position(&p.mod_name))
                    .cmp(&b_first.map(|p| position(&p.mod_name)))
            })
            .then_with(|| a_first.map(|p| &p.path).cmp(&b_first.map(|p| &p.path)))
            .then_with(|| {
                a_first
                    .and_then(|p| p.line)
                    .cmp(&b_first.and_then(|p| p.line))
            })
            .then_with(|| a.diagnostic.message.cmp(&b.diagnostic.message))
    });
}

/// Items grouped by key, groups in the order their keys first appear.
fn group_by_key<T>(items: &[T], key: impl Fn(&T) -> &str) -> Vec<Vec<&T>> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<&T>> = Vec::new();
    for item in items {
        match index.get(key(item)) {
            Some(&at) => groups[at].push(item),
            None => {
                index.insert(key(item), groups.len());
                groups.push(vec![item]);
            }
        }
    }
    groups
}
