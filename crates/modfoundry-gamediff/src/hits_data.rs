//! Mod configs and layout names that use a vanilla config class or layout the new build removed or changed.

use std::collections::{HashMap, HashSet};

use modfoundry_config::ast::Member;
use modfoundry_script::outline::FileOutline;

use crate::config::{class_key, ClassChange, ConfigDiff, ConfigModel};
use crate::hits::{sort_by_place, Hit, HitKind, ModInput};
use crate::layout::{LayoutDiff, LayoutFileChange};
use crate::script::Place;

struct Decl {
    path: Vec<String>,
    scope: Vec<String>,
    parent: Option<String>,
    /// Lower-case names of the properties it sets or deletes directly.
    properties: Vec<String>,
    /// 1-based line of the `class` keyword, 0 when the file has no text.
    line: usize,
    /// `class Name: Parent;`, which only names a parent.
    forward: bool,
}

impl Decl {
    fn key(&self) -> String {
        class_key(&self.path)
    }

    fn sets(&self, property: &str) -> bool {
        self.properties.iter().any(|set| set == property)
    }
}

/// Every mod config class that patches or extends a removed or changed vanilla class.
pub fn config_hits(old: &ConfigModel, diff: &ConfigDiff, input: &ModInput) -> Vec<Hit> {
    let removed: HashSet<String> = diff.removed.iter().map(|c| class_key(&c.path)).collect();
    let changed: HashMap<String, &ClassChange> = diff
        .changed
        .iter()
        .map(|change| (class_key(&change.path), change))
        .collect();
    let mut files: Vec<Vec<Decl>> = input
        .configs
        .iter()
        .map(|config| declarations(&config.file, config.text.as_deref()))
        .collect();
    shared_parents(&mut files);
    let declared = declared(&files);

    let mut hits = Vec::new();
    for (config, decls) in input.configs.iter().zip(&files) {
        for decl in decls.iter().filter(|decl| !decl.forward) {
            let place = Place {
                file: config.path.clone(),
                line: decl.line,
            };
            let key = decl.key();
            if let Some(vanilla) = old.classes.get(&key) {
                let vanilla = vanilla.path.join("/");
                if removed.contains(&key) {
                    hits.push(Hit::new(
                        HitKind::ConfigPatchRemoved,
                        &input.name,
                        place,
                        vanilla,
                        "the class is gone from the new build".to_owned(),
                    ));
                } else if let Some(change) = changed.get(&key) {
                    let detail = patch_detail(change, decl);
                    if let Some(detail) = detail {
                        hits.push(Hit::new(
                            HitKind::ConfigPatchChanged,
                            &input.name,
                            place,
                            vanilla,
                            detail,
                        ));
                    }
                }
                continue;
            }
            let chain = parent_chain(decl, old, &declared);
            if let Some((kind, target, detail)) = child_hit(decl, &chain, old, &removed, &changed) {
                let vanilla = old.classes[target].path.join("/");
                hits.push(Hit::new(kind, &input.name, place, vanilla, detail));
            }
        }
    }
    sort_by_place(&mut hits);
    hits
}

/// Every mod script string naming a removed or changed vanilla layout; `outlines` are `input.outlines()`.
pub fn layout_hits(diff: &LayoutDiff, input: &ModInput, outlines: &[FileOutline]) -> Vec<Hit> {
    let removed: HashMap<String, &str> = diff
        .removed
        .iter()
        .map(|path| (normalise_path(path), path.as_str()))
        .collect();
    let changed: HashMap<String, &LayoutFileChange> = diff
        .changed
        .iter()
        .map(|change| (normalise_path(&change.path), change))
        .collect();

    let mut hits = Vec::new();
    for file in outlines {
        for (text, line) in candidates(file) {
            let Some(text) = layout_name(text) else {
                continue;
            };
            let key = normalise_path(text);
            let place = Place {
                file: file.path.clone(),
                line,
            };
            if let Some(vanilla) = removed.get(&key) {
                hits.push(Hit::new(
                    HitKind::LayoutRemoved,
                    &input.name,
                    place,
                    (*vanilla).to_owned(),
                    "the file is gone from the new build".to_owned(),
                ));
            } else if let Some(change) = changed.get(&key) {
                hits.push(Hit::new(
                    HitKind::LayoutChanged,
                    &input.name,
                    place,
                    change.path.clone(),
                    changed_detail(change),
                ));
            }
        }
    }
    sort_by_place(&mut hits);
    hits
}

fn declarations(file: &modfoundry_config::ast::ConfigFile, text: Option<&str>) -> Vec<Decl> {
    let newlines: Vec<usize> = text
        .map(|text| text.match_indices('\n').map(|(at, _)| at).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk(
        &file.members,
        &mut path,
        text.is_some().then_some(&newlines[..]),
        &mut out,
    );
    out
}

fn walk(
    members: &[Member],
    path: &mut Vec<String>,
    newlines: Option<&[usize]>,
    out: &mut Vec<Decl>,
) {
    for member in members {
        let Member::Class(class) = member else {
            continue;
        };
        path.push(class.name.clone());
        out.push(Decl {
            path: path.clone(),
            scope: path[..path.len() - 1].to_vec(),
            parent: class.parents.first().cloned(),
            properties: class
                .members
                .iter()
                .filter_map(|member| match member {
                    Member::Property(property) => Some(property.name.to_ascii_lowercase()),
                    // As the model reads it: a deleted name is no longer inherited.
                    Member::Delete { name, .. } => Some(name.to_ascii_lowercase()),
                    _ => None,
                })
                .collect(),
            line: newlines.map_or(0, |newlines| {
                newlines.partition_point(|&newline| newline < class.span.start) + 1
            }),
            forward: class.forward,
        });
        walk(&class.members, path, newlines, out);
        path.pop();
    }
}

/// The game merges every declaration of a class, so one that names no parent takes the parent another one names,
/// a forward declaration's only when no full one names any.
fn shared_parents(files: &mut [Vec<Decl>]) {
    let mut parents: HashMap<String, String> = HashMap::new();
    for forward in [false, true] {
        for decl in files
            .iter()
            .flatten()
            .filter(|decl| decl.forward == forward)
        {
            if let Some(parent) = &decl.parent {
                parents.entry(decl.key()).or_insert_with(|| parent.clone());
            }
        }
    }
    for decl in files.iter_mut().flatten() {
        if decl.parent.is_none() {
            decl.parent = parents.get(&decl.key()).cloned();
        }
    }
}

fn declared(files: &[Vec<Decl>]) -> HashMap<String, &Decl> {
    let mut map = HashMap::new();
    for decl in files.iter().flatten().filter(|decl| !decl.forward) {
        map.entry(decl.key()).or_insert(decl);
    }
    map
}

enum Link<'a> {
    Mod(&'a Decl),
    Vanilla(String),
}

/// The parents of a mod class, nearest first: the mod's own classes, then vanilla ones, stopping at a loop.
fn parent_chain<'a>(
    decl: &Decl,
    old: &ConfigModel,
    declared: &HashMap<String, &'a Decl>,
) -> Vec<Link<'a>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut next = decl
        .parent
        .clone()
        .map(|parent| (parent, decl.scope.clone(), true));
    while let Some((parent, scope, in_mod)) = next.take() {
        let Some(link) = resolve(&parent, &scope, old, in_mod.then_some(declared)) else {
            break;
        };
        next = match &link {
            Link::Mod(found) => found
                .parent
                .clone()
                .map(|parent| (parent, found.scope.clone(), true)),
            Link::Vanilla(key) => {
                let class = &old.classes[key];
                let scope = class.path[..class.path.len() - 1].to_vec();
                class.parent.clone().map(|parent| (parent, scope, false))
            }
        };
        let key = match &link {
            Link::Mod(found) => found.key(),
            Link::Vanilla(key) => key.clone(),
        };
        if !seen.insert(key) {
            break;
        }
        chain.push(link);
    }
    chain
}

/// The class a parent name means, from the scope outward; vanilla classes win over the mod's.
fn resolve<'a>(
    parent: &str,
    scope: &[String],
    old: &ConfigModel,
    declared: Option<&HashMap<String, &'a Decl>>,
) -> Option<Link<'a>> {
    for end in (0..=scope.len()).rev() {
        let mut path = scope[..end].to_vec();
        path.push(parent.to_owned());
        let key = class_key(&path);
        if old.classes.contains_key(&key) {
            return Some(Link::Vanilla(key));
        }
        if let Some(found) = declared.and_then(|declared| declared.get(&key)) {
            return Some(Link::Mod(found));
        }
    }
    None
}

/// The first vanilla parent whose removal or change reaches this class, with the properties it inherits from it.
fn child_hit<'c>(
    decl: &Decl,
    chain: &'c [Link],
    old: &ConfigModel,
    removed: &HashSet<String>,
    changed: &HashMap<String, &ClassChange>,
) -> Option<(HitKind, &'c str, String)> {
    let mut set: HashSet<String> = decl.properties.iter().cloned().collect();
    let mut first_vanilla = true;
    for link in chain {
        let key = match link {
            Link::Mod(found) => {
                set.extend(found.properties.iter().cloned());
                continue;
            }
            Link::Vanilla(key) => key,
        };
        if removed.contains(key) {
            let detail = "the parent is gone from the new build".to_owned();
            return first_vanilla.then_some((HitKind::ConfigParentRemoved, key.as_str(), detail));
        }
        if let Some(change) = changed.get(key) {
            let inherited = affected(change, |name| !set.contains(name));
            if let Some(detail) = describe(change, inherited, "inherits") {
                return Some((HitKind::ConfigParentChanged, key.as_str(), detail));
            }
        }
        set.extend(old.classes[key].properties.keys().cloned());
        first_vanilla = false;
    }
    None
}

fn patch_detail(change: &ClassChange, decl: &Decl) -> Option<String> {
    let properties = affected(change, |name| decl.sets(name));
    describe(change, properties, "sets")
}

fn affected(change: &ClassChange, keep: impl Fn(&str) -> bool) -> Vec<String> {
    change
        .properties
        .iter()
        .filter(|property| keep(&property.name.to_ascii_lowercase()))
        .map(|property| property.name.clone())
        .collect()
}

fn describe(change: &ClassChange, properties: Vec<String>, verb: &str) -> Option<String> {
    match (change.parent.is_some(), properties.is_empty()) {
        (false, true) => None,
        (true, true) => Some("the parent changed".to_owned()),
        (false, false) => Some(format!("{verb} {}", properties.join(", "))),
        (true, false) => Some(format!(
            "the parent changed; {verb} {}",
            properties.join(", ")
        )),
    }
}

/// Every string a mod script writes: in method bodies, default parameter values and field and global values.
fn candidates(file: &FileOutline) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let methods = file.classes.iter().flat_map(|class| &class.methods);
    for method in methods.chain(&file.functions) {
        out.extend(
            method
                .strings
                .iter()
                .map(|text| (text.text.as_str(), text.line)),
        );
        for param in &method.params {
            out.extend(literals(param.default.as_deref()).map(|text| (text, method.decl_line)));
        }
    }
    let fields = file.classes.iter().flat_map(|class| &class.fields);
    for field in fields.chain(&file.globals) {
        out.extend(literals(field.value.as_deref()).map(|text| (text, field.line)));
    }
    out
}

/// The double-quoted literals in a value's text, escapes as written.
fn literals(value: Option<&str>) -> impl Iterator<Item = &str> {
    let mut rest = value.unwrap_or("");
    std::iter::from_fn(move || {
        let start = rest.find('"')? + 1;
        let mut escaped = false;
        let len = rest[start..].find(|c: char| {
            let end = c == '"' && !escaped;
            escaped = c == '\\' && !escaped;
            end
        })?;
        let text = &rest[start..start + len];
        rest = &rest[start + len + 1..];
        Some(text)
    })
}

fn layout_name(text: &str) -> Option<&str> {
    text.to_ascii_lowercase()
        .ends_with(".layout")
        .then_some(text)
}

fn normalise_path(path: &str) -> String {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    parts.join("/")
}

fn changed_detail(change: &LayoutFileChange) -> String {
    if change.unparsed {
        return "the file no longer parses".to_owned();
    }
    format!(
        "{} widgets removed, {} added, {} changed",
        change.widgets_removed.len(),
        change.widgets_added.len(),
        change.widgets_changed.len()
    )
}
