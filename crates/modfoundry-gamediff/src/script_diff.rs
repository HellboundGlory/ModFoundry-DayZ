//! What changed between two builds' script models: classes, members, enums, typedefs and free functions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::script::{
    dims, ClassModel, EnumModel, FieldModel, MethodModel, ParamModel, Place, ScriptModel,
    TypedefModel,
};
use crate::{join_non_empty, only_in, optional_change, ValueChange};

/// What differs between two builds' script models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptDiff {
    pub classes_added: Vec<ItemRef>,
    pub classes_removed: Vec<ItemRef>,
    pub classes_changed: Vec<ClassChange>,
    pub functions: Changes<MethodChange>,
    pub globals: Changes<ItemChange>,
    pub enums: Changes<EnumChange>,
    pub typedefs: Changes<ItemChange>,
}

/// Items added, removed and changed under one key space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes<T> {
    pub added: Vec<ItemRef>,
    pub removed: Vec<ItemRef>,
    pub changed: Vec<T>,
}

impl<T> Changes<T> {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

impl<T> Default for Changes<T> {
    fn default() -> Self {
        Changes {
            added: Vec::new(),
            removed: Vec::new(),
            changed: Vec::new(),
        }
    }
}

/// An item by its key, where it is declared and whether only Diag builds compile it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    pub key: String,
    pub name: String,
    pub place: Option<Place>,
    pub diag_only: bool,
}

/// A class present in both builds whose parent or members differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassChange {
    pub name: String,
    pub place: Option<Place>,
    pub diag_only: bool,
    pub base: Option<ValueChange>,
    /// The class's own `#ifdef` conditions, when they differ.
    pub conditions: Option<ValueChange>,
    pub methods: Changes<MethodChange>,
    pub fields: Changes<ItemChange>,
}

/// A method present in both builds whose signature or body changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodChange {
    pub key: String,
    pub old_key: String,
    pub name: String,
    pub place: Place,
    pub diag_only: bool,
    pub kinds: Vec<MethodChangeKind>,
    pub old_signature: String,
    pub new_signature: String,
}

/// What differs about a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodChangeKind {
    Signature,
    ParamsRenamed,
    Defaults,
    Body,
}

/// A field, global or typedef present in both builds with the parts that differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemChange {
    pub key: String,
    pub name: String,
    pub place: Place,
    pub diag_only: bool,
    pub changes: Vec<ValueChange>,
}

/// An enum present in both builds whose base or member values differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumChange {
    pub key: String,
    pub name: String,
    pub place: Place,
    pub diag_only: bool,
    pub base: Option<ValueChange>,
    pub members: Vec<ValueChange>,
}

/// What differs between two builds' script models, in key order.
pub fn script_diff(old: &ScriptModel, new: &ScriptModel) -> ScriptDiff {
    let (classes_added, classes_removed) = item_refs(&old.classes, &new.classes, class_ref);
    ScriptDiff {
        classes_added,
        classes_removed,
        classes_changed: class_changes(old, new),
        functions: method_changes(&old.functions, &new.functions),
        globals: item_changes(&old.globals, &new.globals, field_changes),
        enums: enum_changes(&old.enums, &new.enums),
        typedefs: item_changes(&old.typedefs, &new.typedefs, typedef_changes),
    }
}

/// A method's signature as Enforce writes it.
pub fn signature_text(method: &MethodModel) -> String {
    let params: Vec<String> = method.params.iter().map(param_text).collect();
    let head = format!(
        "{}{}({})",
        if method.destructor { "~" } else { "" },
        method.name,
        params.join(", ")
    );
    join_non_empty(&[&method.modifiers.join(" "), &method.ret, &head], " ")
}

fn param_text(param: &ParamModel) -> String {
    let name = format!("{}{}", param.name, dims(&param.array));
    let base = join_non_empty(&[&param.modifiers.join(" "), &param.ty, &name], " ");
    match &param.default {
        Some(default) => format!("{base} = {default}"),
        None => base,
    }
}

/// Whether a build compiles the item only into Diag builds.
pub fn is_diag_only(guards: &[String]) -> bool {
    guards
        .iter()
        .any(|guard| guard == "DIAG_DEVELOPER" || guard == "DEVELOPER")
}

fn item_refs<T>(
    old: &BTreeMap<String, T>,
    new: &BTreeMap<String, T>,
    reference: impl Fn(&str, &T) -> ItemRef,
) -> (Vec<ItemRef>, Vec<ItemRef>) {
    let added = only_in(new, old)
        .map(|(key, item)| reference(key, item))
        .collect();
    let removed = only_in(old, new)
        .map(|(key, item)| reference(key, item))
        .collect();
    (added, removed)
}

fn class_ref(key: &str, class: &ClassModel) -> ItemRef {
    ItemRef {
        key: key.to_owned(),
        name: class.name.clone(),
        place: class_place(class),
        diag_only: is_diag_only(&class.guards),
    }
}

fn class_place(class: &ClassModel) -> Option<Place> {
    class
        .place
        .clone()
        .or_else(|| class.modded_in.first().cloned())
}

fn class_changes(old: &ScriptModel, new: &ScriptModel) -> Vec<ClassChange> {
    new.classes
        .iter()
        .filter_map(|(key, new_class)| {
            let old_class = old.classes.get(key)?;
            let base = optional_change("base", old_class.base.clone(), new_class.base.clone());
            let conditions = optional_change(
                "conditions",
                conditions_text(&old_class.guards),
                conditions_text(&new_class.guards),
            );
            let methods = method_changes(&old_class.methods, &new_class.methods);
            let fields = item_changes(&old_class.fields, &new_class.fields, field_changes);
            let any =
                base.is_some() || conditions.is_some() || !methods.is_empty() || !fields.is_empty();
            any.then(|| ClassChange {
                name: new_class.name.clone(),
                place: class_place(new_class),
                diag_only: is_diag_only(&new_class.guards),
                base,
                conditions,
                methods,
                fields,
            })
        })
        .collect()
}

fn conditions_text(guards: &[String]) -> Option<String> {
    (!guards.is_empty()).then(|| guards.join(" && "))
}

/// What every compared item shares: a name, where it is written and the conditions it sits under.
trait Item {
    fn name(&self) -> &str;
    fn place(&self) -> &Place;
    fn guards(&self) -> &[String];
}

impl Item for FieldModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn place(&self) -> &Place {
        &self.place
    }
    fn guards(&self) -> &[String] {
        &self.guards
    }
}

impl Item for TypedefModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn place(&self) -> &Place {
        &self.place
    }
    fn guards(&self) -> &[String] {
        &self.guards
    }
}

impl Item for EnumModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn place(&self) -> &Place {
        &self.place
    }
    fn guards(&self) -> &[String] {
        &self.guards
    }
}

impl Item for MethodModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn place(&self) -> &Place {
        &self.place
    }
    fn guards(&self) -> &[String] {
        &self.guards
    }
}

fn item_changes<T: Item>(
    old: &BTreeMap<String, T>,
    new: &BTreeMap<String, T>,
    parts: impl Fn(&T, &T) -> Vec<ValueChange>,
) -> Changes<ItemChange> {
    keyed_changes(old, new, |key, old_item, new_item| {
        let changes = parts(old_item, new_item);
        (!changes.is_empty()).then(|| ItemChange {
            key: key.to_owned(),
            name: new_item.name().to_owned(),
            place: new_item.place().clone(),
            diag_only: is_diag_only(new_item.guards()),
            changes,
        })
    })
}

/// Keys on one side only are added or removed; a key on both sides is changed when `changed` says so.
fn keyed_changes<T: Item, C>(
    old: &BTreeMap<String, T>,
    new: &BTreeMap<String, T>,
    changed: impl Fn(&str, &T, &T) -> Option<C>,
) -> Changes<C> {
    let mut changes = Changes::default();
    for (key, new_item) in new {
        match old.get(key) {
            Some(old_item) => changes.changed.extend(changed(key, old_item, new_item)),
            None => changes.added.push(item_ref(key, new_item)),
        }
    }
    changes.removed = only_in(old, new)
        .map(|(key, item)| item_ref(key, item))
        .collect();
    changes
}

fn item_ref<T: Item>(key: &str, item: &T) -> ItemRef {
    ItemRef {
        key: key.to_owned(),
        name: item.name().to_owned(),
        place: Some(item.place().clone()),
        diag_only: is_diag_only(item.guards()),
    }
}

fn field_changes(old: &FieldModel, new: &FieldModel) -> Vec<ValueChange> {
    let parts = [
        value_change("type", &field_type(old), &field_type(new)),
        value_change(
            "modifiers",
            &old.modifiers.join(" "),
            &new.modifiers.join(" "),
        ),
        optional_change("value", old.value.clone(), new.value.clone()),
    ];
    parts.into_iter().flatten().collect()
}

fn typedef_changes(old: &TypedefModel, new: &TypedefModel) -> Vec<ValueChange> {
    value_change("target", &old.target, &new.target)
        .into_iter()
        .collect()
}

fn field_type(field: &FieldModel) -> String {
    format!("{}{}", field.ty, dims(&field.array))
}

fn value_change(name: &str, old: &str, new: &str) -> Option<ValueChange> {
    optional_change(name, Some(old.to_owned()), Some(new.to_owned()))
}

fn enum_changes(
    old: &BTreeMap<String, EnumModel>,
    new: &BTreeMap<String, EnumModel>,
) -> Changes<EnumChange> {
    keyed_changes(old, new, |key, old_enum, new_enum| {
        let base = optional_change("base", old_enum.base.clone(), new_enum.base.clone());
        let members = member_changes(old_enum, new_enum);
        (base.is_some() || !members.is_empty()).then(|| EnumChange {
            key: key.to_owned(),
            name: new_enum.name.clone(),
            place: new_enum.place.clone(),
            diag_only: is_diag_only(&new_enum.guards),
            base,
            members,
        })
    })
}

fn member_changes(old: &EnumModel, new: &EnumModel) -> Vec<ValueChange> {
    let old_values = effective_values(old);
    let new_values = effective_values(new);
    let mut changes: Vec<ValueChange> = new
        .members
        .iter()
        .zip(&new_values)
        .filter_map(|(name, value)| {
            let before = old
                .members
                .iter()
                .position(|old_name| old_name == name)
                .map(|at| old_values[at].clone());
            let same = before.as_deref().is_some_and(|old| same_value(old, value));
            (!same).then(|| ValueChange {
                name: name.clone(),
                old: before,
                new: Some(value.clone()),
            })
        })
        .collect();
    for (name, value) in old.members.iter().zip(&old_values) {
        if !new.members.contains(name) {
            changes.push(ValueChange {
                name: name.clone(),
                old: Some(value.clone()),
                new: None,
            });
        }
    }
    changes
}

fn effective_values(enumeration: &EnumModel) -> Vec<String> {
    let mut values = Vec::new();
    let mut last: Option<(usize, &str)> = None;
    for (index, own) in enumeration.values.iter().enumerate() {
        let value = match own {
            Some(text) => {
                last = Some((index, text));
                text.clone()
            }
            None => match last {
                Some((at, text)) => counted_on(text, index - at),
                None => index.to_string(),
            },
        };
        values.push(value);
    }
    values
}

fn counted_on(value: &str, step: usize) -> String {
    let counted = parse_integer(value).and_then(|number| number.checked_add(step as i64));
    match counted {
        Some(number) => number.to_string(),
        None => format!("{value} + {step}"),
    }
}

/// Equal text, or the same integer written another way (`16` and `0x10`).
fn same_value(old: &str, new: &str) -> bool {
    old == new || parse_integer(old).is_some_and(|number| parse_integer(new) == Some(number))
}

fn parse_integer(text: &str) -> Option<i64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

fn method_changes(
    old: &BTreeMap<String, MethodModel>,
    new: &BTreeMap<String, MethodModel>,
) -> Changes<MethodChange> {
    let mut changes = Changes::default();
    let mut old_only: Vec<&String> = Vec::new();
    let mut new_only: Vec<&String> = Vec::new();
    for key in old.keys() {
        match new.get(key) {
            Some(new_method) => {
                let kinds = method_kinds(&old[key], new_method);
                if !kinds.is_empty() {
                    changes
                        .changed
                        .push(method_change(key, key, &old[key], new_method, kinds));
                }
            }
            None => old_only.push(key),
        }
    }
    new_only.extend(only_in(new, old).map(|(key, _)| key));

    let pairs = pair_by_name(&old_only, &new_only, old, new);
    for (old_key, new_key) in &pairs {
        let old_method = &old[*old_key];
        let new_method = &new[*new_key];
        let kinds = method_kinds(old_method, new_method);
        changes.changed.push(method_change(
            old_key, new_key, old_method, new_method, kinds,
        ));
    }

    let paired_old: BTreeSet<&String> = pairs.iter().map(|(old_key, _)| *old_key).collect();
    let paired_new: BTreeSet<&String> = pairs.iter().map(|(_, new_key)| *new_key).collect();
    for key in old_only {
        if !paired_old.contains(key) {
            changes.removed.push(item_ref(key, &old[key]));
        }
    }
    for key in new_only {
        if !paired_new.contains(key) {
            changes.added.push(item_ref(key, &new[key]));
        }
    }
    changes
        .changed
        .sort_by(|left, right| left.key.cmp(&right.key));
    changes
}

fn method_change(
    old_key: &str,
    new_key: &str,
    old_method: &MethodModel,
    new_method: &MethodModel,
    kinds: Vec<MethodChangeKind>,
) -> MethodChange {
    MethodChange {
        key: new_key.to_owned(),
        old_key: old_key.to_owned(),
        name: new_method.name.clone(),
        place: new_method.place.clone(),
        diag_only: is_diag_only(&new_method.guards),
        kinds,
        old_signature: signature_text(old_method),
        new_signature: signature_text(new_method),
    }
}

fn pair_by_name<'a>(
    old_only: &[&'a String],
    new_only: &[&'a String],
    old: &'a BTreeMap<String, MethodModel>,
    new: &'a BTreeMap<String, MethodModel>,
) -> Vec<(&'a String, &'a String)> {
    let mut pairs = Vec::new();
    for old_key in old_only {
        let old_method = &old[*old_key];
        let old_matches = old_only
            .iter()
            .filter(|other| same_method(old_key, old_method, other, &old[**other]));
        if old_matches.count() != 1 {
            continue;
        }
        let new_matches: Vec<&String> = new_only
            .iter()
            .filter(|other| same_method(old_key, old_method, other, &new[**other]))
            .copied()
            .collect();
        if let [new_key] = new_matches[..] {
            pairs.push((*old_key, new_key));
        }
    }
    pairs
}

fn same_method(key: &str, method: &MethodModel, other_key: &str, other: &MethodModel) -> bool {
    method.name == other.name
        && method.destructor == other.destructor
        && key_tail(key) == key_tail(other_key)
}

fn key_tail(key: &str) -> &str {
    // An array size inside the parameters can hold its own `)`.
    key.rsplit_once(')').map_or("", |(_, tail)| tail)
}

fn method_kinds(old: &MethodModel, new: &MethodModel) -> Vec<MethodChangeKind> {
    let mut kinds = Vec::new();
    let signatures_differ = old.ret != new.ret
        || modifiers_changed(&old.modifiers, &new.modifiers)
        || old.params.len() != new.params.len()
        || old
            .params
            .iter()
            .zip(&new.params)
            .any(|(a, b)| a.ty != b.ty || a.modifiers != b.modifiers || a.array != b.array);
    if signatures_differ {
        kinds.push(MethodChangeKind::Signature);
    }
    if old
        .params
        .iter()
        .zip(&new.params)
        .any(|(a, b)| a.name != b.name)
    {
        kinds.push(MethodChangeKind::ParamsRenamed);
    }
    if old
        .params
        .iter()
        .zip(&new.params)
        .any(|(a, b)| a.default != b.default)
    {
        kinds.push(MethodChangeKind::Defaults);
    }
    if old.body != new.body {
        kinds.push(MethodChangeKind::Body);
    }
    kinds
}

fn modifiers_changed(old: &[String], new: &[String]) -> bool {
    fn kept(modifiers: &[String]) -> BTreeSet<&str> {
        modifiers
            .iter()
            .map(String::as_str)
            .filter(|modifier| *modifier != "override")
            .collect()
    }
    kept(old) != kept(new)
}
