//! One game build's config classes merged across files, and a compare of two builds.

use std::collections::{BTreeMap, HashMap};

use modfoundry_config::ast::{ConfigClass, ConfigFile, ConfigProperty, Member, Value};
use serde::{Deserialize, Serialize};

use crate::{map_changes, only_in, optional_change, ValueChange};

/// Every class of a build, keyed by its lower-case path joined with `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigModel {
    pub classes: BTreeMap<String, ConfigClassModel>,
}

/// One class path with the parent and property values it ends up with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigClassModel {
    pub path: Vec<String>,
    pub parent: Option<String>,
    /// Keyed by lower-case property name.
    pub properties: BTreeMap<String, PropertyModel>,
    /// Every source that defines it with a body, in load order.
    pub sources: Vec<String>,
}

/// One property's rendered value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyModel {
    pub name: String,
    pub value: String,
    pub append: bool,
    pub source: String,
}

/// What differs between two builds' configs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDiff {
    pub added: Vec<ClassRef>,
    pub removed: Vec<ClassRef>,
    pub changed: Vec<ClassChange>,
}

/// A class named by its path and the sources that define it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassRef {
    pub path: Vec<String>,
    pub sources: Vec<String>,
}

/// A class present in both builds whose parent or properties differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassChange {
    pub path: Vec<String>,
    pub parent: Option<ValueChange>,
    pub properties: Vec<ValueChange>,
}

/// One line of a property's value, the same for its text and binarized form.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Str(text) | Value::Ident(text) => format!("\"{}\"", text.replace('"', "\"\"")),
        Value::Number(literal) => rendered_number(literal),
        Value::Float(number) => number.to_string(),
        Value::Other(text) => text.clone(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(value_text).collect();
            format!("{{{}}}", items.join(","))
        }
    }
}

fn rendered_number(literal: &str) -> String {
    let trimmed = literal.trim();
    if let Ok(integer) = trimmed.parse::<i64>() {
        return integer.to_string();
    }
    match trimmed.parse::<f32>() {
        Ok(float) => float.to_string(),
        Err(_) => literal.to_owned(),
    }
}

/// Merges the parsed configs of a build into one class model, later sources winning.
pub fn config_model(sources: &[(String, ConfigFile)]) -> ConfigModel {
    let mut merger = Merger::default();
    for (name, file) in sources {
        let mut path = Vec::new();
        merger.members(&file.members, &mut path, name);
    }
    merger.model
}

#[derive(Default)]
struct Merger {
    model: ConfigModel,
    /// Parents named only by a forward declaration, for a class not yet defined.
    forward_parents: HashMap<String, String>,
}

impl Merger {
    fn members(&mut self, members: &[Member], path: &mut Vec<String>, source: &str) {
        for member in members {
            match member {
                Member::Class(class) => {
                    path.push(class.name.clone());
                    if class.forward {
                        self.forward(class, &class_key(path));
                    } else {
                        self.class(class, path, source);
                        self.members(&class.members, path, source);
                    }
                    path.pop();
                }
                Member::Property(property) if !path.is_empty() => {
                    if let Some(class) = self.model.classes.get_mut(&class_key(path)) {
                        class.define_property(property, source);
                    }
                }
                Member::Delete { name, .. } if !path.is_empty() => {
                    self.delete(&class_key(path), name);
                }
                _ => {}
            }
        }
    }

    // As the game does: a forward declaration's parent applies only to a class that has none.
    fn forward(&mut self, class: &ConfigClass, key: &str) {
        let Some(parent) = class.parents.first() else {
            return;
        };
        match self.model.classes.get_mut(key) {
            Some(known) => {
                known.parent.get_or_insert_with(|| parent.clone());
            }
            None => {
                self.forward_parents
                    .entry(key.to_owned())
                    .or_insert_with(|| parent.clone());
            }
        }
    }

    fn class(&mut self, class: &ConfigClass, path: &[String], source: &str) {
        let key = class_key(path);
        let pending = self.forward_parents.remove(&key);
        let entry = self
            .model
            .classes
            .entry(key)
            .or_insert_with(|| ConfigClassModel {
                path: path.to_vec(),
                parent: pending,
                properties: BTreeMap::new(),
                sources: Vec::new(),
            });
        entry.define(class, source);
    }

    fn delete(&mut self, class_key: &str, name: &str) {
        if let Some(class) = self.model.classes.get_mut(class_key) {
            class.properties.remove(&lower(name));
        }
        let removed = format!("{class_key}/{}", lower(name));
        let children = format!("{removed}/");
        self.model.classes.remove(&removed);
        let doomed: Vec<String> = self
            .model
            .classes
            .range(children.clone()..)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(&children))
            .cloned()
            .collect();
        for key in doomed {
            self.model.classes.remove(&key);
        }
    }
}

fn joined_arrays(first: &str, second: &str) -> String {
    let inner = |text: &str| -> String {
        let text = text.strip_prefix('{').unwrap_or(text);
        text.strip_suffix('}').unwrap_or(text).to_owned()
    };
    let items: Vec<String> = [inner(first), inner(second)]
        .into_iter()
        .filter(|items| !items.is_empty())
        .collect();
    format!("{{{}}}", items.join(","))
}

/// The key a class path is stored under.
pub fn class_key(path: &[String]) -> String {
    let parts: Vec<String> = path.iter().map(|name| lower(name)).collect();
    parts.join("/")
}

fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl ConfigClassModel {
    fn define(&mut self, class: &ConfigClass, source: &str) {
        if !self.sources.iter().any(|known| known == source) {
            self.sources.push(source.to_owned());
        }
        if let Some(parent) = class.parents.first() {
            self.parent = Some(parent.clone());
        }
    }

    fn define_property(&mut self, property: &ConfigProperty, source: &str) {
        let value = property.value.as_ref().map(value_text).unwrap_or_default();
        let key = lower(&property.name);
        if property.array_append {
            if let Some(known) = self.properties.get_mut(&key) {
                known.value = joined_arrays(&known.value, &value);
                known.source = source.to_owned();
                return;
            }
        }
        self.properties.insert(
            key,
            PropertyModel {
                name: property.name.clone(),
                value,
                append: property.array_append,
                source: source.to_owned(),
            },
        );
    }
}

/// What differs between two models, in key order.
pub fn config_diff(old: &ConfigModel, new: &ConfigModel) -> ConfigDiff {
    let added = only_in(&new.classes, &old.classes)
        .map(|(_, class)| class_ref(class))
        .collect();
    let removed = only_in(&old.classes, &new.classes)
        .map(|(_, class)| class_ref(class))
        .collect();

    let mut changed = Vec::new();
    for (key, new_class) in &new.classes {
        let Some(old_class) = old.classes.get(key) else {
            continue;
        };
        let parent = parent_change(old_class.parent.as_deref(), new_class.parent.as_deref());
        let properties = map_changes(
            &old_class.properties,
            &new_class.properties,
            |_, property| property.name.clone(),
            rendered_value,
        );
        if parent.is_some() || !properties.is_empty() {
            changed.push(ClassChange {
                path: new_class.path.clone(),
                parent,
                properties,
            });
        }
    }

    ConfigDiff {
        added,
        removed,
        changed,
    }
}

fn class_ref(class: &ConfigClassModel) -> ClassRef {
    ClassRef {
        path: class.path.clone(),
        sources: class.sources.clone(),
    }
}

fn parent_change(old: Option<&str>, new: Option<&str>) -> Option<ValueChange> {
    let same = match (old, new) {
        (None, None) => true,
        (Some(old), Some(new)) => old.eq_ignore_ascii_case(new),
        _ => false,
    };
    if same {
        return None;
    }
    optional_change("parent", old.map(str::to_owned), new.map(str::to_owned))
}

fn rendered_value(property: &PropertyModel) -> String {
    if property.append {
        format!("+= {}", property.value)
    } else {
        property.value.clone()
    }
}
