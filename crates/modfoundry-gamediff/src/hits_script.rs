//! Mod scripts that use a vanilla class or method the new build removed or changed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use modfoundry_script::outline::{ClassOutline, FileOutline, MethodOutline};

use crate::hits::{sort_by_place, Hit, HitKind, ModInput};
use crate::script::{method_head, ClassModel, MethodModel, Place, ScriptModel};
use crate::script_diff::{
    is_diag_only, ClassChange, ItemRef, MethodChange, MethodChangeKind, ScriptDiff,
};

/// Every place in one mod's scripts that a script change may break; `outlines` are `input.outlines()`.
pub fn script_hits(
    old: &ScriptModel,
    new: &ScriptModel,
    diff: &ScriptDiff,
    input: &ModInput,
    outlines: &[FileOutline],
) -> Vec<Hit> {
    let index = Index::new(old, new, diff);
    let mod_classes = mod_classes(outlines, old);

    let mut hits = Vec::new();
    for item in &mod_classes {
        let Some(target) = item.target.as_deref() else {
            continue;
        };
        if let Some(removed) = index.removed.get(target) {
            removed_class_hit(item, target, removed.diag_only, &input.name, &mut hits);
            continue;
        }
        base_class_hit(item, target, &index, &input.name, &mut hits);
        override_hits(item, target, &index, &input.name, &mut hits);
    }
    call_hits(&index, outlines, &input.name, &mut hits);

    sort_by_place(&mut hits);
    hits
}

struct Index<'a> {
    old: &'a ScriptModel,
    new: &'a ScriptModel,
    diff: &'a ScriptDiff,
    removed: HashMap<&'a str, &'a ItemRef>,
    changed: HashMap<&'a str, &'a ClassChange>,
    /// Old classes' method keys by head; `#ifdef` arms of one method share a head.
    old_heads: HashMap<&'a str, HashMap<String, Vec<&'a str>>>,
    new_heads: HashMap<&'a str, HashSet<String>>,
}

impl<'a> Index<'a> {
    fn new(old: &'a ScriptModel, new: &'a ScriptModel, diff: &'a ScriptDiff) -> Self {
        let old_heads = old
            .classes
            .iter()
            .map(|(name, class)| {
                let mut heads: HashMap<String, Vec<&str>> = HashMap::new();
                for (key, method) in &class.methods {
                    heads.entry(method.head()).or_default().push(key.as_str());
                }
                (name.as_str(), heads)
            })
            .collect();
        let new_heads = new
            .classes
            .iter()
            .map(|(name, class)| {
                (
                    name.as_str(),
                    class.methods.values().map(MethodModel::head).collect(),
                )
            })
            .collect();
        Index {
            old,
            new,
            diff,
            removed: diff
                .classes_removed
                .iter()
                .map(|item| (item.name.as_str(), item))
                .collect(),
            changed: diff
                .classes_changed
                .iter()
                .map(|change| (change.name.as_str(), change))
                .collect(),
            old_heads,
            new_heads,
        }
    }

    fn overridden(
        &self,
        chain: &[&'a ClassModel],
        head: &str,
    ) -> Option<(&'a ClassModel, &[&'a str])> {
        chain.iter().find_map(|class| {
            let keys = self.old_heads.get(class.name.as_str())?.get(head)?;
            Some((*class, keys.as_slice()))
        })
    }

    fn declared_in_new(&self, chain: &[&ClassModel], head: &str) -> bool {
        chain.iter().any(|class| {
            self.new_heads
                .get(class.name.as_str())
                .is_some_and(|heads| heads.contains(head))
        })
    }
}

struct ModClass<'a> {
    path: &'a str,
    class: &'a ClassOutline,
    target: Option<String>,
}

impl ModClass<'_> {
    fn place(&self) -> Place {
        Place {
            file: self.path.to_owned(),
            line: self.class.lines[0],
        }
    }

    fn method_place(&self, method: &MethodOutline) -> Place {
        Place {
            file: self.path.to_owned(),
            line: method.lines[0],
        }
    }
}

fn removed_class_hit(
    item: &ModClass,
    target: &str,
    diag_only: bool,
    mod_name: &str,
    hits: &mut Vec<Hit>,
) {
    let (kind, detail) = if item.class.modded {
        (
            HitKind::ModdedClassRemoved,
            format!("modded class {target} is gone from the new build"),
        )
    } else {
        (
            HitKind::BaseClassRemoved,
            format!(
                "class {} extends {target}, which is gone from the new build",
                item.class.name
            ),
        )
    };
    hits.push(Hit {
        diag_only,
        ..Hit::new(kind, mod_name, item.place(), target.to_owned(), detail)
    });
}

fn base_class_hit(
    item: &ModClass,
    target: &str,
    index: &Index,
    mod_name: &str,
    hits: &mut Vec<Hit>,
) {
    let Some(change) = index.changed.get(target) else {
        return;
    };
    let Some(base) = &change.base else {
        return;
    };
    let from = base.old.as_deref().unwrap_or("nothing");
    let to = base.new.as_deref().unwrap_or("nothing");
    let detail = if item.class.modded {
        format!("modded class {target}: its parent changed from {from} to {to}")
    } else {
        format!(
            "class {} extends {target}, whose parent changed from {from} to {to}",
            item.class.name
        )
    };
    hits.push(Hit {
        diag_only: change.diag_only,
        ..Hit::new(
            HitKind::BaseClassChanged,
            mod_name,
            item.place(),
            target.to_owned(),
            detail,
        )
    });
}

fn override_hits(
    item: &ModClass,
    target: &str,
    index: &Index,
    mod_name: &str,
    hits: &mut Vec<Hit>,
) {
    let old_chain = chain(index.old, target);
    let new_chain = chain(index.new, target);
    for method in &item.class.methods {
        let head = head_of_outline(method);
        let Some((owner, keys)) = index.overridden(&old_chain, &head) else {
            continue;
        };
        let vanilla = format!("{}.{head}", owner.name);
        let owner_removed = index.removed.get(owner.name.as_str()).copied();
        let change = index.changed.get(owner.name.as_str());
        let strongest = keys
            .iter()
            .filter_map(|key| {
                let removed = change
                    .and_then(|change| change.methods.removed.iter().find(|item| item.key == *key))
                    .or(owner_removed);
                if let Some(removed) = removed {
                    if index.declared_in_new(&new_chain, &head) {
                        return None;
                    }
                    let detail = format!("overrides {vanilla}, which is gone from the new build");
                    return Some((HitKind::OverrideRemoved, detail, removed.diag_only));
                }
                let changed = change?
                    .methods
                    .changed
                    .iter()
                    .find(|change| change.old_key == *key)?;
                let (kind, detail) = override_change(method, &vanilla, changed)?;
                Some((kind, detail, changed.diag_only))
            })
            .min_by_key(|(kind, _, _)| *kind);
        let Some((kind, detail, diag_only)) = strongest else {
            continue;
        };
        hits.push(Hit {
            diag_only,
            ..Hit::new(kind, mod_name, item.method_place(method), vanilla, detail)
        });
    }
}

/// The hit a change to the overridden method gives, strongest kind first; a renamed parameter alone gives none.
fn override_change(
    method: &MethodOutline,
    vanilla: &str,
    change: &MethodChange,
) -> Option<(HitKind, String)> {
    let kinds = &change.kinds;
    if kinds.contains(&MethodChangeKind::Signature) {
        return Some((
            HitKind::OverrideSignature,
            format!(
                "overrides {vanilla}, whose signature is now {}",
                change.new_signature
            ),
        ));
    }
    if kinds.contains(&MethodChangeKind::Defaults) {
        return Some((
            HitKind::OverrideDefaults,
            format!(
                "overrides {vanilla}, whose defaults are now {}",
                change.new_signature
            ),
        ));
    }
    if !kinds.contains(&MethodChangeKind::Body) {
        return None;
    }
    let calls_super = method
        .calls
        .iter()
        .any(|call| call.receiver.as_deref() == Some("super") && call.name == method.name);
    let detail = if calls_super {
        format!("overrides {vanilla}, whose body changed")
    } else {
        format!("overrides {vanilla}, whose body changed and never calls super")
    };
    Some((HitKind::OverrideBody, detail))
}

/// Calls in the mod whose name only one vanilla class or free function has, where it went away or changed shape.
fn call_hits(index: &Index, outlines: &[FileOutline], mod_name: &str, hits: &mut Vec<Hit>) {
    let targets = call_targets(index);
    if targets.is_empty() {
        return;
    }
    // A name the mod declares itself may be what the call means; an override of a removed method has its own hit.
    let own: BTreeSet<&str> = outlines
        .iter()
        .flat_map(|file| {
            file.classes
                .iter()
                .flat_map(|class| &class.methods)
                .chain(&file.functions)
        })
        .map(|method| method.name.as_str())
        .collect();

    let mut seen = BTreeSet::new();
    for file in outlines {
        let methods = file.classes.iter().flat_map(|class| &class.methods);
        for method in methods.chain(&file.functions) {
            for call in &method.calls {
                if call.receiver.as_deref() == Some("super") || own.contains(call.name.as_str()) {
                    continue;
                }
                let Some(target) = targets.get(call.name.as_str()) else {
                    continue;
                };
                if !seen.insert((file.path.as_str(), call.line, call.name.as_str())) {
                    continue;
                }
                let vanilla = &target.vanilla;
                let detail = if target.kind == HitKind::CallRemoved {
                    format!("calls {vanilla}, which is gone from the new build")
                } else {
                    format!("calls {vanilla}, whose signature changed")
                };
                hits.push(Hit {
                    maybe: true,
                    diag_only: target.diag_only,
                    ..Hit::new(
                        target.kind,
                        mod_name,
                        Place {
                            file: file.path.clone(),
                            line: call.line,
                        },
                        vanilla.clone(),
                        detail,
                    )
                });
            }
        }
    }
}

struct CallTarget {
    kind: HitKind,
    vanilla: String,
    diag_only: bool,
}

fn call_targets<'a>(index: &'a Index<'a>) -> BTreeMap<&'a str, CallTarget> {
    let owners = owners(index);
    let mut targets: BTreeMap<&str, CallTarget> = BTreeMap::new();
    let mut add = |name: &'a str, kind: HitKind, vanilla: String, diag_only: bool| {
        targets.entry(name).or_insert(CallTarget {
            kind,
            vanilla,
            diag_only,
        });
    };
    let new_names: HashSet<&str> = index
        .new
        .classes
        .values()
        .flat_map(|class| class.methods.values())
        .map(|method| method.name.as_str())
        .collect();
    for item in &index.diff.classes_removed {
        let Some(class) = index.old.classes.get(&item.name) else {
            continue;
        };
        for method in class.methods.values() {
            // A name the new build still declares somewhere most likely moved with its class.
            if new_names.contains(method.name.as_str()) {
                continue;
            }
            let vanilla = format!("{}.{}", class.name, method.name);
            let diag_only = item.diag_only || is_diag_only(&method.guards);
            add(&method.name, HitKind::CallRemoved, vanilla, diag_only);
        }
    }
    for change in &index.diff.classes_changed {
        let new_chain = chain(index.new, &change.name);
        for item in &change.methods.removed {
            let kept = new_chain.iter().any(|class| {
                class
                    .methods
                    .values()
                    .any(|method| method.name == item.name)
            });
            if !kept {
                let vanilla = format!("{}.{}", change.name, item.name);
                add(&item.name, HitKind::CallRemoved, vanilla, item.diag_only);
            }
        }
        for changed in &change.methods.changed {
            if changed.kinds.contains(&MethodChangeKind::Signature) {
                let vanilla = format!("{}.{}", change.name, changed.name);
                add(
                    &changed.name,
                    HitKind::CallSignature,
                    vanilla,
                    changed.diag_only,
                );
            }
        }
    }
    for item in &index.diff.functions.removed {
        let kept = index
            .new
            .functions
            .values()
            .any(|function| function.name == item.name);
        if !kept {
            add(
                &item.name,
                HitKind::CallRemoved,
                item.name.clone(),
                item.diag_only,
            );
        }
    }
    for changed in &index.diff.functions.changed {
        if changed.kinds.contains(&MethodChangeKind::Signature) {
            let vanilla = changed.name.clone();
            add(
                &changed.name,
                HitKind::CallSignature,
                vanilla,
                changed.diag_only,
            );
        }
    }
    targets.retain(|name, _| owners.get(name) == Some(&1));
    targets
}

/// How many vanilla owners declare each method name: one per class, plus the free functions as one more.
fn owners<'a>(index: &'a Index<'a>) -> HashMap<&'a str, usize> {
    let mut owners: HashMap<&str, usize> = HashMap::new();
    for class in index.old.classes.values() {
        let names: BTreeSet<&str> = class
            .methods
            .values()
            .map(|method| method.name.as_str())
            .collect();
        for name in names {
            *owners.entry(name).or_default() += 1;
        }
    }
    let functions: BTreeSet<&str> = index
        .old
        .functions
        .values()
        .map(|function| function.name.as_str())
        .collect();
    for name in functions {
        *owners.entry(name).or_default() += 1;
    }
    owners
}

fn mod_classes<'a>(outlines: &'a [FileOutline], old: &ScriptModel) -> Vec<ModClass<'a>> {
    let plain = plain_classes(outlines, old);
    outlines
        .iter()
        .flat_map(|file| {
            file.classes.iter().map(|class| ModClass {
                path: &file.path,
                class,
                target: vanilla_target(class, old, &plain),
            })
        })
        .collect()
}

/// The mod's plain classes by name, leaving out the names the old build has.
fn plain_classes<'a>(
    outlines: &'a [FileOutline],
    old: &ScriptModel,
) -> HashMap<&'a str, &'a ClassOutline> {
    let mut plain = HashMap::new();
    for class in outlines.iter().flat_map(|file| &file.classes) {
        if !class.modded && !old.classes.contains_key(&class.name) {
            plain.entry(class.name.as_str()).or_insert(class);
        }
    }
    plain
}

fn vanilla_target(
    class: &ClassOutline,
    old: &ScriptModel,
    plain: &HashMap<&str, &ClassOutline>,
) -> Option<String> {
    if class.modded {
        return old
            .classes
            .contains_key(&class.name)
            .then(|| class.name.clone());
    }
    if old.classes.contains_key(&class.name) {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut current = base_name(class.base.as_deref()?)?;
    loop {
        if !seen.insert(current) {
            return None;
        }
        if old.classes.contains_key(current) {
            return Some(current.to_owned());
        }
        current = base_name(plain.get(current)?.base.as_deref()?)?;
    }
}

fn chain<'a>(model: &'a ScriptModel, start: &str) -> Vec<&'a ClassModel> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = start;
    while seen.insert(current.to_owned()) {
        let Some(class) = model.classes.get(current) else {
            break;
        };
        chain.push(class);
        let Some(base) = class.base.as_deref().and_then(base_name) else {
            break;
        };
        current = base;
    }
    chain
}

fn base_name(base: &str) -> Option<&str> {
    let name = base.split('<').next().unwrap_or("").trim();
    (!name.is_empty()).then_some(name)
}

/// A method's name and parameter types, as Enforce identifies an overload.
pub fn head_of_outline(method: &MethodOutline) -> String {
    method_head(
        &method.name,
        method.destructor,
        method
            .params
            .iter()
            .map(|param| (param.ty.as_str(), param.array.as_slice())),
    )
}
