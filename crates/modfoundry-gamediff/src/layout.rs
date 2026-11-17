//! A flattened model of one build's layouts, and a compare of two models.

use crate::{map_changes, only_in, short_hash, ValueChange};
use modfoundry_layout::parse::{self, Property, Value, Widget};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Every layout file of one build, keyed by its lower-case path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutModel {
    pub files: BTreeMap<String, LayoutFile>,
}

/// One layout file: its text hash, its parse error if any, and its widgets by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutFile {
    pub path: String,
    pub hash: String,
    pub error: Option<String>,
    pub widgets: BTreeMap<String, WidgetModel>,
}

/// One widget with its position in the tree flattened into a name path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetModel {
    pub path: String,
    pub class: String,
    pub line: u32,
    pub properties: BTreeMap<String, String>,
}

/// What differs between two layout models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<LayoutFileChange>,
}

/// The widgets and widget properties that differ in one changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutFileChange {
    pub path: String,
    pub unparsed: bool,
    pub widgets_added: Vec<WidgetRef>,
    pub widgets_removed: Vec<WidgetRef>,
    pub widgets_changed: Vec<WidgetChange>,
}

/// A widget named by path, with its class and line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetRef {
    pub path: String,
    pub class: String,
    pub line: u32,
}

/// A widget present in both builds whose class or properties differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetChange {
    pub path: String,
    pub line: u32,
    pub class: Option<ValueChange>,
    pub properties: Vec<ValueChange>,
}

/// Flatten each layout file into its widgets by name path.
pub fn layout_model(files: &[(String, String)]) -> LayoutModel {
    let mut model = LayoutModel::default();
    for (path, text) in files {
        let path = path.replace('\\', "/");
        let file = layout_file(path.clone(), text);
        model.files.insert(path.to_lowercase(), file);
    }
    model
}

/// Compare two models by file, then by widget, then by property.
pub fn layout_diff(old: &LayoutModel, new: &LayoutModel) -> LayoutDiff {
    let mut diff = LayoutDiff::default();
    for (key, old_file) in &old.files {
        match new.files.get(key) {
            None => diff.removed.push(old_file.path.clone()),
            Some(new_file) if old_file.hash != new_file.hash => {
                if let Some(change) = file_change(old_file, new_file) {
                    diff.changed.push(change);
                }
            }
            Some(_) => {}
        }
    }
    for (_, new_file) in only_in(&new.files, &old.files) {
        diff.added.push(new_file.path.clone());
    }
    diff
}

fn layout_file(path: String, text: &str) -> LayoutFile {
    let hash = short_hash(text.as_bytes());
    match parse::parse(text) {
        Ok(widgets) => {
            let mut flat = BTreeMap::new();
            flatten(&widgets, "", &mut flat);
            LayoutFile {
                path,
                hash,
                error: None,
                widgets: flat,
            }
        }
        Err(error) => LayoutFile {
            path,
            hash,
            error: Some(error.to_string()),
            widgets: BTreeMap::new(),
        },
    }
}

fn flatten(widgets: &[Widget], prefix: &str, out: &mut BTreeMap<String, WidgetModel>) {
    for (widget, segment) in widgets.iter().zip(segments(widgets)) {
        let path = if prefix.is_empty() {
            segment
        } else {
            format!("{prefix}/{segment}")
        };
        out.insert(
            path.clone(),
            WidgetModel {
                path: path.clone(),
                class: widget.class.clone(),
                line: widget.pos.line,
                properties: properties(&widget.properties),
            },
        );
        flatten(&widget.children, &path, out);
    }
}

fn segments(widgets: &[Widget]) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    widgets
        .iter()
        .map(|widget| {
            let base = widget.name.clone().unwrap_or_else(|| widget.class.clone());
            let count = seen.entry(base.clone()).or_default();
            *count += 1;
            match *count {
                1 => base,
                n => format!("{base}#{n}"),
            }
        })
        .collect()
}

fn properties(list: &[Property]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for property in list {
        map.insert(property.key.clone(), property_text(&property.values));
    }
    map
}

fn property_text(values: &[Value]) -> String {
    let parts: Vec<String> = values
        .iter()
        .map(|value| match value {
            Value::Number(text) | Value::Word(text) => text.clone(),
            Value::Str(text) => format!("\"{text}\""),
        })
        .collect();
    parts.join(" ")
}

fn file_change(old: &LayoutFile, new: &LayoutFile) -> Option<LayoutFileChange> {
    if old.error.is_some() || new.error.is_some() {
        return Some(LayoutFileChange {
            path: new.path.clone(),
            unparsed: true,
            widgets_added: Vec::new(),
            widgets_removed: Vec::new(),
            widgets_changed: Vec::new(),
        });
    }
    let mut widgets_added = Vec::new();
    let mut widgets_removed = Vec::new();
    let mut widgets_changed = Vec::new();
    for (path, old_widget) in &old.widgets {
        match new.widgets.get(path) {
            None => widgets_removed.push(widget_ref(old_widget)),
            Some(new_widget) => {
                if let Some(change) = widget_change(old_widget, new_widget) {
                    widgets_changed.push(change);
                }
            }
        }
    }
    for (_, new_widget) in only_in(&new.widgets, &old.widgets) {
        widgets_added.push(widget_ref(new_widget));
    }
    let any =
        !widgets_added.is_empty() || !widgets_removed.is_empty() || !widgets_changed.is_empty();
    any.then(|| LayoutFileChange {
        path: new.path.clone(),
        unparsed: false,
        widgets_added,
        widgets_removed,
        widgets_changed,
    })
}

fn widget_ref(widget: &WidgetModel) -> WidgetRef {
    WidgetRef {
        path: widget.path.clone(),
        class: widget.class.clone(),
        line: widget.line,
    }
}

fn widget_change(old: &WidgetModel, new: &WidgetModel) -> Option<WidgetChange> {
    let class = (old.class != new.class).then(|| ValueChange {
        name: "class".to_owned(),
        old: Some(old.class.clone()),
        new: Some(new.class.clone()),
    });
    let properties = map_changes(
        &old.properties,
        &new.properties,
        |key, _| key.clone(),
        String::clone,
    );
    (class.is_some() || !properties.is_empty()).then(|| WidgetChange {
        path: new.path.clone(),
        line: new.line,
        class,
        properties,
    })
}
