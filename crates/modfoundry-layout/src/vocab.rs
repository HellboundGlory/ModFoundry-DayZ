//! The widget classes and properties a game's layouts use, learned from the game itself.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::parse::{Value, Widget};

/// More distinct bare words than this for one property and any word is accepted.
pub const MAX_WORDS: usize = 32;

/// What one property's values looked like in the learned layouts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Every value pattern seen (see [`pattern`]).
    pub patterns: BTreeSet<String>,
    /// The bare words seen; `None` once more than [`MAX_WORDS`] distinct words were seen.
    pub words: Option<BTreeSet<String>>,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            patterns: BTreeSet::new(),
            words: Some(BTreeSet::new()),
        }
    }
}

/// Known widget classes with their properties, plus widget classes declared in script.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vocabulary {
    /// Properties seen on each widget class, by layout class name (`TextWidgetClass`).
    pub classes: BTreeMap<String, BTreeMap<String, Shape>>,
    /// Layout class names of the script classes that derive from `Widget` (`RenderTargetWidgetClass`).
    pub script_widgets: BTreeSet<String>,
}

impl Shape {
    fn add_word(&mut self, word: &str) {
        let Some(words) = self.words.as_mut() else {
            return;
        };
        words.insert(word.to_owned());
        if words.len() > MAX_WORDS {
            self.words = None;
        }
    }

    fn merged_with(mut self, other: &Shape) -> Shape {
        self.patterns.extend(other.patterns.iter().cloned());
        self.words = match (self.words.take(), &other.words) {
            (Some(mut words), Some(other_words)) => {
                words.extend(other_words.iter().cloned());
                (words.len() <= MAX_WORDS).then_some(words)
            }
            _ => None,
        };
        self
    }
}

impl Vocabulary {
    /// Records every widget and its properties, children included.
    pub fn learn(&mut self, widgets: &[Widget]) {
        for widget in widgets {
            let properties = self.classes.entry(widget.class.clone()).or_default();
            for property in &widget.properties {
                if property.values.is_empty() {
                    continue;
                }
                let shape = properties.entry(property.key.clone()).or_default();
                shape.patterns.insert(pattern(&property.values));
                for value in &property.values {
                    if let Value::Word(word) = value {
                        shape.add_word(word);
                    }
                }
            }
            self.learn(&widget.children);
        }
    }

    /// Adds each script class whose chain of bases reaches `Widget`, given as (name, base).
    pub fn add_script_widgets(&mut self, classes: &[(String, Option<String>)]) {
        // A `modded class` repeats a name without its base; the declared base wins.
        let mut bases: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for (name, base) in classes {
            let known = bases.entry(name.as_str()).or_default();
            if known.is_none() {
                *known = base.as_deref();
            }
        }
        for (name, _) in classes {
            if name != "Widget" && reaches_widget(name, &bases) {
                self.script_widgets.insert(format!("{name}Class"));
            }
        }
    }

    /// The property's shape merged over every class that has it; `None` when no class has it.
    pub fn shape_anywhere(&self, key: &str) -> Option<Shape> {
        let mut shapes = self
            .classes
            .values()
            .filter_map(|properties| properties.get(key))
            .cloned();
        let first = shapes.next()?;
        Some(shapes.fold(first, |merged, shape| merged.merged_with(&shape)))
    }

    /// Whether the class is a learned layout class or a script widget class.
    pub fn knows_class(&self, class: &str) -> bool {
        self.classes.contains_key(class) || self.script_widgets.contains(class)
    }
}

fn reaches_widget(name: &str, bases: &BTreeMap<&str, Option<&str>>) -> bool {
    let mut seen = BTreeSet::new();
    let mut current = name;
    while seen.insert(current) {
        match bases.get(current).copied().flatten() {
            Some("Widget") => return true,
            Some(base) => current = base,
            None => return false,
        }
    }
    false
}

/// One letter per value: `n` number, `w` bare word, `s` string.
pub fn pattern(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| match value {
            Value::Number(_) => 'n',
            Value::Word(_) => 'w',
            Value::Str(_) => 's',
        })
        .collect()
}
