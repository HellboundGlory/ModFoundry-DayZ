//! Findings for a layout file against the game's vocabulary and the files it references.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use modfoundry_core::text::edit_distance_ignore_ascii_case;
use modfoundry_core::{Diagnostic, Severity};

use crate::parse::{parse, Pos, Property, Value, Widget};
use crate::source::Source;
use crate::vocab::{pattern, Shape, Vocabulary};

/// The text does not parse.
pub const UNPARSED: &str = "MF-L101";
/// A widget class the game does not know.
pub const UNKNOWN_CLASS: &str = "MF-L102";
/// A property the widget's class does not take.
pub const UNKNOWN_PROPERTY: &str = "MF-L103";
/// A property value shaped differently from the game's layouts.
pub const BAD_VALUE: &str = "MF-L104";
/// Two siblings with one name.
pub const DUPLICATE_NAME: &str = "MF-L105";
/// A texture, image, font or style that does not resolve.
pub const UNRESOLVED: &str = "MF-L106";

// Vanilla never writes these defaults, so the vocabulary cannot learn them.
const ENGINE_DEFAULTS: [(&str, &str); 2] = [("halign", "left_ref"), ("valign", "top_ref")];

/// A file a layout points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Reference {
    /// A texture path, without its `{id}` prefix.
    Texture(String),
    /// An image in an imageset.
    Image { set: String, image: String },
    /// A font path.
    Font(String),
    /// A style name.
    Style(String),
}

impl Reference {
    fn kind(&self) -> ReferenceKind {
        match self {
            Reference::Texture(_) => ReferenceKind::Texture,
            Reference::Image { .. } => ReferenceKind::Image,
            Reference::Font(_) => ReferenceKind::Font,
            Reference::Style(_) => ReferenceKind::Style,
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ReferenceKind {
    Texture,
    Image,
    Font,
    Style,
}

/// Tells whether a reference exists in the game or the mod.
pub trait Resolve {
    /// Whether `reference` exists.
    fn resolves(&self, reference: &Reference) -> bool;
}

/// Checks one layout's text; findings come back in source order.
pub fn check(text: &str, vocab: &Vocabulary, resolve: &dyn Resolve) -> Vec<Diagnostic> {
    let mut checker = Checker {
        source: Source::new(text),
        vocab,
        resolve,
        found: Vec::new(),
    };
    match parse(text) {
        Ok(widgets) => checker.siblings(&widgets),
        Err(error) => checker.report(UNPARSED, Severity::Error, &error.pos, error.message),
    }
    let mut found = checker.found;
    found.sort_by_key(|diagnostic| diagnostic.span.map(|span| span.start));
    found
}

const SUGGESTION_DISTANCE: usize = 2;

struct Checker<'a> {
    source: Source<'a>,
    vocab: &'a Vocabulary,
    resolve: &'a dyn Resolve,
    found: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn report(&mut self, code: &str, severity: Severity, pos: &Pos, message: String) {
        let span = self.source.span_from(pos);
        self.found
            .push(Diagnostic::at(code, severity, span, message));
    }

    fn siblings(&mut self, widgets: &[Widget]) {
        self.repeated_names(widgets);
        for widget in widgets {
            self.widget(widget);
        }
    }

    fn repeated_names(&mut self, widgets: &[Widget]) {
        let mut first_lines: HashMap<&str, u32> = HashMap::new();
        for widget in widgets {
            let Some(name) = widget.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            match first_lines.get(name) {
                Some(line) => {
                    let message = format!("duplicate name `{name}`; first used on line {line}");
                    self.report(DUPLICATE_NAME, Severity::Warning, &widget.pos, message);
                }
                None => {
                    first_lines.insert(name, widget.pos.line);
                }
            }
        }
    }

    fn widget(&mut self, widget: &Widget) {
        if self.vocab.knows_class(&widget.class) {
            for property in &widget.properties {
                self.property(widget, property);
                self.references(property);
            }
        } else {
            self.unknown_class(widget);
        }
        self.siblings(&widget.children);
    }

    fn property(&mut self, widget: &Widget, property: &Property) {
        match self.vocab.classes.get(&widget.class) {
            Some(keys) => match keys.get(&property.key) {
                Some(shape) => self.shape_check(widget, property, shape),
                None => self.missing_key(widget, keys, property),
            },
            None => self.script_property(widget, property),
        }
    }

    fn shape_check(&mut self, widget: &Widget, property: &Property, shape: &Shape) {
        for problem in shape_problems(&widget.class, property, shape) {
            self.report(BAD_VALUE, Severity::Warning, &property.pos, problem);
        }
    }

    fn missing_key(
        &mut self,
        widget: &Widget,
        keys: &BTreeMap<String, Shape>,
        property: &Property,
    ) {
        let class = &widget.class;
        let key = &property.key;
        if self.used_by_any_class(key) {
            let message = format!("`{key}` is not used on `{class}` in the game's layouts");
            self.report(UNKNOWN_PROPERTY, Severity::Warning, &property.pos, message);
        } else {
            let hint = hint(closest(key, keys.keys()));
            let message = format!("unknown property `{key}` on `{class}`{hint}");
            self.report(UNKNOWN_PROPERTY, Severity::Error, &property.pos, message);
        }
    }

    fn script_property(&mut self, widget: &Widget, property: &Property) {
        if !self.used_by_any_class(&property.key) {
            let message = format!(
                "unknown property `{}` on script widget `{}`; no widget class has it",
                property.key, widget.class
            );
            self.report(UNKNOWN_PROPERTY, Severity::Error, &property.pos, message);
        }
    }

    fn references(&mut self, property: &Property) {
        let mut reported = BTreeSet::new();
        for reference in references_in(property) {
            if self.resolve.resolves(&reference) || !reported.insert(reference.kind()) {
                continue;
            }
            let message = unresolved_message(&reference);
            self.report(UNRESOLVED, Severity::Error, &property.pos, message);
        }
    }

    fn used_by_any_class(&self, key: &str) -> bool {
        self.vocab
            .classes
            .values()
            .any(|keys| keys.contains_key(key))
    }

    fn unknown_class(&mut self, widget: &Widget) {
        let known = self.vocab.classes.keys().chain(&self.vocab.script_widgets);
        let hint = hint(closest(&widget.class, known));
        let message = format!("unknown widget class `{}`{hint}", widget.class);
        self.report(UNKNOWN_CLASS, Severity::Error, &widget.pos, message);
    }
}

fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    candidates
        .map(|candidate| (edit_distance_ignore_ascii_case(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= SUGGESTION_DISTANCE)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

fn references_in(property: &Property) -> Vec<Reference> {
    let mut found = Vec::new();
    for value in &property.values {
        match value {
            Value::Str(text) if !text.is_empty() => {
                found.extend(texture(text));
                found.extend(image_in_set(text));
                if property.key == "font" {
                    found.push(Reference::Font(text.clone()));
                }
                if property.key == "style" {
                    found.push(Reference::Style(text.clone()));
                }
            }
            Value::Word(word) if property.key == "style" => {
                found.push(Reference::Style(word.clone()));
            }
            _ => {}
        }
    }
    found
}

/// `{16 hex digits}path`, the form a texture takes in a layout.
fn texture(text: &str) -> Option<Reference> {
    let (id, path) = text.strip_prefix('{')?.split_once('}')?;
    let hex = id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit());
    (hex && !path.is_empty()).then(|| Reference::Texture(path.to_owned()))
}

/// `set:<set> image:<image>`, the form an image takes in a layout.
fn image_in_set(text: &str) -> Option<Reference> {
    let (set, image) = text.strip_prefix("set:")?.split_once(" image:")?;
    if set.is_empty() || image.is_empty() {
        return None;
    }
    Some(Reference::Image {
        set: set.to_owned(),
        image: image.to_owned(),
    })
}

fn unresolved_message(reference: &Reference) -> String {
    match reference {
        Reference::Texture(path) => format!("texture `{path}` was not found"),
        Reference::Image { set, image } => format!("image `{image}` was not found in set `{set}`"),
        Reference::Font(path) => format!("font `{path}` was not found"),
        Reference::Style(name) => format!("style `{name}` was not found"),
    }
}

fn shape_problems(class: &str, property: &Property, shape: &Shape) -> Vec<String> {
    let key = &property.key;
    let found = pattern(&property.values);
    if !shape.patterns.contains(&found) {
        let expected = list(&shape.patterns);
        return vec![format!(
            "`{key}` on `{class}` expects value shape {expected}, found `{found}`"
        )];
    }
    // A style is a name looked up among the game's and the mod's styles, not a fixed word.
    let Some(words) = shape.words.as_ref().filter(|_| key != "style") else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for value in &property.values {
        let Value::Word(word) = value else {
            continue;
        };
        if words.contains(word) || is_engine_default(key, word) || !seen.insert(word) {
            continue;
        }
        problems.push(format!(
            "`{word}` is not a value `{key}` takes on `{class}`; expected one of {}",
            list(words)
        ));
    }
    problems
}

fn is_engine_default(key: &str, word: &str) -> bool {
    ENGINE_DEFAULTS
        .iter()
        .any(|&(default_key, default_word)| default_key == key && default_word == word)
}

fn list<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    items
        .into_iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn hint(suggestion: Option<&String>) -> String {
    suggestion.map_or_else(String::new, |name| format!("; did you mean `{name}`?"))
}
