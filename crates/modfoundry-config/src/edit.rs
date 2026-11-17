//! Structured config.cpp edits planned as byte splices over the original text.

use crate::ast::{ConfigClass, ConfigFile, ConfigProperty, Member, Value};
use crate::parser::parse;
use modfoundry_core::text::{is_c_identifier, line_end, line_range_with_eol, line_start};
use modfoundry_core::Span;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Class names from the file root down to a class; matching is case-insensitive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPath(pub Vec<String>);

impl ConfigPath {
    /// A path from its class names, root first.
    pub fn of(names: &[&str]) -> Self {
        Self(names.iter().map(|n| (*n).to_owned()).collect())
    }
}

/// What an edit addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Class(ConfigPath),
    Property { class: ConfigPath, name: String },
}

/// A value to write; numbers and identifiers are spelled exactly as given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewValue {
    Str(String),
    Number(String),
    Ident(String),
    Array(Vec<NewValue>),
}

/// Where a new member goes among its siblings; `Before`/`After` name a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    First,
    Last,
    Before(String),
    After(String),
}

/// One structured change to a config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    SetValue {
        property: Target,
        value: NewValue,
    },
    AddProperty {
        class: ConfigPath,
        name: String,
        value: NewValue,
        position: Position,
    },
    Remove {
        target: Target,
    },
    Rename {
        target: Target,
        new_name: String,
    },
    AddClass {
        parent: ConfigPath,
        name: String,
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        forward: bool,
        position: Position,
    },
    SetBase {
        class: ConfigPath,
        base: Option<String>,
    },
    ArrayInsert {
        property: Target,
        index: usize,
        value: NewValue,
    },
    ArrayRemove {
        property: Target,
        index: usize,
    },
    ArraySet {
        property: Target,
        index: usize,
        value: NewValue,
    },
    ArrayMove {
        property: Target,
        from: usize,
        to: usize,
    },
    /// Toggles the `[]` on a property's key; the value must already fit.
    SetArray {
        target: Target,
        array: bool,
    },
    /// Removes the `occurrence`-th (0-based) same-named sibling in `class`.
    RemoveAt {
        class: ConfigPath,
        name: String,
        occurrence: usize,
    },
}

/// A byte splice: replace `len` bytes at `offset` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub offset: usize,
    pub len: usize,
    pub text: String,
}

/// Why an edit can't be planned; the messages are shown to users as-is.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditError {
    #[error("{0} was not found")]
    NotFound(String),
    #[error("{0} is declared more than once here")]
    Ambiguous(String),
    #[error("{0} already exists here")]
    Exists(String),
    #[error("`{0}` isn't a valid name")]
    InvalidName(String),
    #[error("{0}")]
    InvalidValue(String),
    #[error("`{0}` isn't an array")]
    NotAnArray(String),
    #[error("Index {index} is out of range for an array of {len}")]
    IndexOutOfRange { len: usize, index: usize },
    #[error("The file has syntax errors. Fix them in Source before editing here.")]
    Unparseable,
    #[error("{0}")]
    ReadOnly(String),
}

/// Formatting conventions new text should follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    /// Indent of the members of the class being edited.
    pub indent: String,
    /// One nesting step, used for array items.
    pub unit: String,
    pub eol: &'static str,
    pub allman: bool,
    pub spaced_eq: bool,
    pub padded_braces: bool,
    pub multiline_arrays: bool,
}

const RESERVED_NAMES: [&str; 3] = ["class", "delete", "external"];
const DEFAULT_UNIT: &str = "    ";

/// Plans the byte splices that apply `op` to `text`.
pub fn plan(text: &str, op: &EditOp) -> Result<Vec<TextEdit>, EditError> {
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Err(EditError::Unparseable);
    }
    let edits = plan_in(text, &parsed.file, op)?;
    #[cfg(debug_assertions)]
    if let Err(problem) = check_postcondition_in(text, &parsed.file, op, &edits) {
        panic!("edit postcondition violated for {op:?}: {problem}");
    }
    Ok(edits)
}

/// Like [`plan`] for a tree already parsed from `text`; skips the syntax and postcondition checks.
pub fn plan_in(text: &str, file: &ConfigFile, op: &EditOp) -> Result<Vec<TextEdit>, EditError> {
    let mut edits = Planner { text, file }.plan(op)?;
    edits.retain(|e| text[e.offset..e.offset + e.len] != e.text);
    edits.sort_by_key(|e| e.offset);
    Ok(edits)
}

/// Applies `ops` in order, re-parsing between them.
pub fn plan_all(text: &str, ops: &[EditOp]) -> Result<String, EditError> {
    let mut current = text.to_owned();
    for op in ops {
        let edits = plan(&current, op)?;
        current = apply(&current, &edits);
    }
    Ok(current)
}

/// Applies non-overlapping edits to `text`.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_owned();
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.offset));
    for e in sorted {
        out.replace_range(e.offset..e.offset + e.len, &e.text);
    }
    out
}

/// Spells `value` as config source in `style`.
pub fn render_value(value: &NewValue, style: &Style) -> Result<String, EditError> {
    validate(value)?;
    match value {
        NewValue::Array(items) if !items.is_empty() && style.multiline_arrays => {
            let item_indent = format!("{}{}", style.indent, style.unit);
            let lines: Vec<String> = items
                .iter()
                .map(|i| format!("{item_indent}{}", render_inline(i, style.padded_braces)))
                .collect();
            let separator = format!(",{}", style.eol);
            Ok(format!(
                "{{{eol}{}{eol}{}}}",
                lines.join(&separator),
                style.indent,
                eol = style.eol
            ))
        }
        _ => Ok(render_inline(value, style.padded_braces)),
    }
}

/// The style of `class`'s body, else of the whole file, else `mf new`'s.
pub fn infer_style(text: &str, class: Option<&ConfigPath>) -> Style {
    let file = parse(text).file;
    let class = class.and_then(|path| Planner { text, file: &file }.walk(path).ok().flatten());
    style_for(text, &file, class)
}

/// Checks that `edits` leave a parseable file equal to the input with only `op` applied.
pub fn check_postcondition(text: &str, op: &EditOp, edits: &[TextEdit]) -> Result<(), String> {
    check_postcondition_in(text, &parse(text).file, op, edits)
}

fn check_postcondition_in(
    text: &str,
    before: &ConfigFile,
    op: &EditOp,
    edits: &[TextEdit],
) -> Result<(), String> {
    let after = apply(text, edits);
    let reparsed = parse(&after);
    if let Some(issue) = reparsed.issues.first() {
        return Err(format!(
            "the edited file no longer parses: {}",
            issue.message
        ));
    }
    let mut expected = nodes(&before.members);
    model(&mut expected, op)?;
    if expected == nodes(&reparsed.file.members) {
        Ok(())
    } else {
        Err("the edited tree differs from the input with the op applied".into())
    }
}

fn is_blank(s: &str) -> bool {
    s.chars().all(|c| c == ' ' || c == '\t')
}

fn own_line_before(t: &str, pos: usize) -> bool {
    is_blank(&t[line_start(t, pos)..pos])
}

fn ends_line_after(t: &str, pos: usize) -> bool {
    let rest = t[pos..line_end(t, pos)].trim();
    rest.is_empty() || rest.starts_with("//")
}

fn leading_ws(t: &str, pos: usize) -> &str {
    let line = &t[line_start(t, pos)..line_end(t, pos)];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn same_line(t: &str, a: usize, b: usize) -> bool {
    line_start(t, a) == line_start(t, b)
}

fn replace(span: Span, text: impl Into<String>) -> TextEdit {
    TextEdit {
        offset: span.start,
        len: span.end - span.start,
        text: text.into(),
    }
}

fn insert_at(offset: usize, text: impl Into<String>) -> TextEdit {
    replace(Span::new(offset, offset), text)
}

fn delete(start: usize, end: usize) -> TextEdit {
    replace(Span::new(start, end), "")
}

fn valid_name(name: &str) -> bool {
    is_c_identifier(name)
}

/// Whether `name` is a valid class or property name (reserved words aside).
pub fn check_member_name(name: &str) -> Result<(), EditError> {
    if valid_name(name) && !RESERVED_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(EditError::InvalidName(name.into()))
    }
}

/// Whether `name` is a valid base class name.
pub fn check_base_name(name: &str) -> Result<(), EditError> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(EditError::InvalidName(name.into()))
    }
}

fn valid_number(s: &str) -> bool {
    let b = s.strip_prefix('-').unwrap_or(s).as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int = digits(0);
    let mut i = int;
    if int == 0 {
        if b.get(i) != Some(&b'.') {
            return false;
        }
        i += 1;
        let fraction = digits(i);
        if fraction == 0 {
            return false;
        }
        i += fraction;
    } else if b.get(i) == Some(&b'.') {
        i += 1;
        i += digits(i);
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exponent = digits(i);
        if exponent == 0 {
            return false;
        }
        i += exponent;
    }
    i == b.len()
}

fn validate(value: &NewValue) -> Result<(), EditError> {
    match value {
        NewValue::Str(s) if s.contains('"') => {
            Err(EditError::InvalidValue("Strings can't contain \"".into()))
        }
        NewValue::Str(s) if s.contains(['\n', '\r']) => Err(EditError::InvalidValue(
            "Strings can't contain a line break".into(),
        )),
        NewValue::Number(n) if !valid_number(n) => Err(EditError::InvalidValue(format!(
            "`{n}` isn't a valid number"
        ))),
        NewValue::Ident(i) if !valid_name(i) => Err(EditError::InvalidValue(format!(
            "`{i}` isn't a valid identifier"
        ))),
        NewValue::Array(items) => items.iter().try_for_each(validate),
        _ => Ok(()),
    }
}

fn render_inline(value: &NewValue, padded: bool) -> String {
    match value {
        NewValue::Str(s) => format!("\"{s}\""),
        NewValue::Number(n) => n.clone(),
        NewValue::Ident(i) => i.clone(),
        NewValue::Array(items) if items.is_empty() => "{}".into(),
        NewValue::Array(items) => {
            let inner: Vec<String> = items.iter().map(|i| render_inline(i, padded)).collect();
            if padded {
                format!("{{ {} }}", inner.join(", "))
            } else {
                format!("{{{}}}", inner.join(", "))
            }
        }
    }
}

fn same_value(old: &Value, new: &NewValue) -> bool {
    match (old, new) {
        (Value::Str(a), NewValue::Str(b))
        | (Value::Number(a), NewValue::Number(b))
        | (Value::Ident(a), NewValue::Ident(b)) => a == b,
        (Value::Array(a), NewValue::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y))
        }
        _ => false,
    }
}

fn property_text(name: &str, value: &NewValue, style: &Style) -> Result<String, EditError> {
    let rendered = render_value(value, style)?;
    let brackets = if matches!(value, NewValue::Array(_)) {
        "[]"
    } else {
        ""
    };
    let eq = if style.spaced_eq { " = " } else { "=" };
    if rendered.starts_with('{') && rendered.contains('\n') {
        Ok(format!(
            "{name}{brackets}{}{}{}{rendered};",
            eq.trim_end(),
            style.eol,
            style.indent
        ))
    } else {
        Ok(format!("{name}{brackets}{eq}{rendered};"))
    }
}

fn class_text(
    name: &str,
    base: Option<&str>,
    forward: bool,
    style: &Style,
    colon: &(String, String),
) -> String {
    let base = base.map_or(String::new(), |b| format!("{}:{}{b}", colon.0, colon.1));
    let (eol, indent) = (style.eol, &style.indent);
    if forward {
        format!("class {name}{base};")
    } else if style.allman {
        format!("class {name}{base}{eol}{indent}{{{eol}{indent}}};")
    } else {
        format!("class {name}{base} {{{eol}{indent}}};")
    }
}

fn comma_after(t: &str, from: usize, limit: usize) -> Option<usize> {
    let b = t.as_bytes();
    let mut i = from;
    while i < limit {
        match b[i] {
            b',' => return Some(i),
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < limit && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => i += t[i + 2..].find("*/")? + 4,
            _ => return None,
        }
    }
    None
}

fn inline_gap(t: &str, items: &[Span]) -> String {
    if let [first, second, ..] = items {
        let between = &t[first.end..second.start];
        if let Some(comma) = between.find(',') {
            let after = &between[comma + 1..];
            if is_blank(after) {
                return after.to_owned();
            }
        }
    }
    " ".into()
}

fn index_for(names: &[&str], position: &Position) -> Result<usize, EditError> {
    let anchor = |wanted: &str| {
        let mut hits = names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.eq_ignore_ascii_case(wanted))
            .map(|(i, _)| i);
        let first = hits
            .next()
            .ok_or_else(|| EditError::NotFound(format!("Member `{wanted}`")))?;
        if hits.next().is_some() {
            return Err(EditError::Ambiguous(format!("Member `{wanted}`")));
        }
        Ok(first)
    };
    Ok(match position {
        Position::First => 0,
        Position::Last => names.len(),
        Position::Before(n) => anchor(n)?,
        Position::After(n) => anchor(n)? + 1,
    })
}

fn class_in<'a>(
    members: &'a [Member],
    name: &str,
    whole: &str,
) -> Result<&'a ConfigClass, EditError> {
    let mut hits = members.iter().filter_map(|m| match m {
        Member::Class(c) if c.name.eq_ignore_ascii_case(name) => Some(c),
        _ => None,
    });
    let first = hits
        .next()
        .ok_or_else(|| EditError::NotFound(format!("Class `{whole}`")))?;
    if hits.next().is_some() {
        return Err(EditError::Ambiguous(format!("Class `{whole}`")));
    }
    Ok(first)
}

fn property_in<'a>(class: &'a ConfigClass, name: &str) -> Result<&'a ConfigProperty, EditError> {
    let mut hits = class.members.iter().filter_map(|m| match m {
        Member::Property(p) if p.name.eq_ignore_ascii_case(name) => Some(p),
        _ => None,
    });
    let Some(first) = hits.next() else {
        let deleted = class
            .members
            .iter()
            .any(|m| matches!(m, Member::Delete { name: d, .. } if d.eq_ignore_ascii_case(name)));
        return Err(if deleted {
            EditError::ReadOnly(format!("`delete {name};` statements are edited in Source"))
        } else {
            EditError::NotFound(format!("Property `{name}`"))
        });
    };
    if hits.next().is_some() {
        return Err(EditError::Ambiguous(format!("Property `{name}`")));
    }
    Ok(first)
}

fn declared(members: &[Member], name: &str, except: Option<usize>) -> bool {
    members.iter().any(|m| {
        matches!(m, Member::Class(_) | Member::Property(_))
            && m.name().eq_ignore_ascii_case(name)
            && Some(m.statement().start) != except
    })
}

fn split_property(target: &Target) -> Result<(&ConfigPath, &str), EditError> {
    match target {
        Target::Property { class, name } => Ok((class, name)),
        Target::Class(_) => Err(EditError::InvalidValue(
            "This edit needs a property, not a class".into(),
        )),
    }
}

struct Body<'a> {
    members: &'a [Member],
    class: Option<&'a ConfigClass>,
}

struct ArrayRef<'a> {
    class: &'a ConfigClass,
    property: &'a ConfigProperty,
    open: Span,
    close: Span,
    items: &'a [Span],
    values: &'a [Value],
}

enum Slot {
    Line(usize),
    Inline {
        offset: usize,
        lead: &'static str,
        trail: &'static str,
    },
    Reflow,
}

struct Planner<'a> {
    text: &'a str,
    file: &'a ConfigFile,
}

impl<'a> Planner<'a> {
    fn plan(&self, op: &EditOp) -> Result<Vec<TextEdit>, EditError> {
        match op {
            EditOp::SetValue { property, value } => self.set_value(property, value),
            EditOp::AddProperty {
                class,
                name,
                value,
                position,
            } => self.add_property(class, name, value, position),
            EditOp::Remove { target } => self.remove(target),
            EditOp::Rename { target, new_name } => self.rename(target, new_name),
            EditOp::AddClass {
                parent,
                name,
                base,
                forward,
                position,
            } => self.add_class(parent, name, base.as_deref(), *forward, position),
            EditOp::SetBase { class, base } => self.set_base(class, base.as_deref()),
            EditOp::ArrayInsert {
                property,
                index,
                value,
            } => self.array_insert(property, *index, value),
            EditOp::ArrayRemove { property, index } => self.array_remove(property, *index),
            EditOp::ArraySet {
                property,
                index,
                value,
            } => self.array_set(property, *index, value),
            EditOp::ArrayMove { property, from, to } => self.array_move(property, *from, *to),
            EditOp::SetArray { target, array } => self.set_array(target, *array),
            EditOp::RemoveAt {
                class,
                name,
                occurrence,
            } => self.remove_at(class, name, *occurrence),
        }
    }

    fn walk(&self, path: &ConfigPath) -> Result<Option<&'a ConfigClass>, EditError> {
        let file = self.file;
        let mut members = file.members.as_slice();
        let mut current = None;
        for (i, name) in path.0.iter().enumerate() {
            let class = class_in(members, name, &path.0[..=i].join("/"))?;
            members = &class.members;
            current = Some(class);
        }
        Ok(current)
    }

    fn body(&self, path: &ConfigPath) -> Result<Body<'a>, EditError> {
        let file = self.file;
        let class = self.walk(path)?;
        Ok(Body {
            members: class.map_or(file.members.as_slice(), |c| c.members.as_slice()),
            class,
        })
    }

    fn class(&self, path: &ConfigPath) -> Result<&'a ConfigClass, EditError> {
        self.walk(path)?
            .ok_or_else(|| EditError::NotFound("Class path".into()))
    }

    fn writable_body(&self, path: &ConfigPath) -> Result<Body<'a>, EditError> {
        let body = self.body(path)?;
        if body.class.is_some_and(|c| c.forward) {
            return Err(EditError::ReadOnly(format!(
                "Class `{}` is a forward declaration and has no body",
                path.0.join("/")
            )));
        }
        Ok(body)
    }

    fn locate_class(&self, path: &ConfigPath) -> Result<(Body<'a>, &'a ConfigClass), EditError> {
        let (last, parent) = path
            .0
            .split_last()
            .ok_or_else(|| EditError::NotFound("Class path".into()))?;
        let body = self.body(&ConfigPath(parent.to_vec()))?;
        let class = class_in(body.members, last, &path.0.join("/"))?;
        Ok((body, class))
    }

    fn property(
        &self,
        target: &Target,
    ) -> Result<(&'a ConfigClass, &'a ConfigProperty), EditError> {
        let (class_path, name) = split_property(target)?;
        let class = self.class(class_path)?;
        Ok((class, property_in(class, name)?))
    }

    fn array(&self, target: &Target) -> Result<ArrayRef<'a>, EditError> {
        let (class, property) = self.property(target)?;
        match (&property.value, property.spans.array_braces) {
            (Some(Value::Array(values)), Some((open, close)))
                if values.len() == property.spans.value_items.len() =>
            {
                Ok(ArrayRef {
                    class,
                    property,
                    open,
                    close,
                    items: &property.spans.value_items,
                    values,
                })
            }
            _ => Err(EditError::NotAnArray(property.name.clone())),
        }
    }

    fn style(&self, class: Option<&ConfigClass>) -> Style {
        style_for(self.text, self.file, class)
    }

    fn adopt_array_layout(&self, style: &mut Style, p: &ConfigProperty) {
        let t = self.text;
        if own_line_before(t, p.spans.statement.start) {
            style.indent = leading_ws(t, p.spans.statement.start).to_owned();
        }
        let (Some((open, close)), Some(first)) =
            (p.spans.array_braces, p.spans.value_items.first())
        else {
            return;
        };
        let one_line = same_line(t, open.start, close.start);
        style.multiline_arrays = !one_line && own_line_before(t, first.start);
        if one_line {
            style.padded_braces = t[open.end..].starts_with([' ', '\t']);
        }
    }

    fn set_value(&self, target: &Target, value: &NewValue) -> Result<Vec<TextEdit>, EditError> {
        let (class, p) = self.property(target)?;
        let Some(span) = p.spans.value else {
            return Err(EditError::ReadOnly(format!(
                "`{}` has no value to change",
                p.name
            )));
        };
        if p.value.as_ref().is_some_and(|old| same_value(old, value)) {
            return Ok(vec![]);
        }
        let is_array = matches!(value, NewValue::Array(_));
        if is_array && !p.declared_array {
            return Err(EditError::InvalidValue(format!(
                "`{}` isn't declared as an array, so it can't take a list",
                p.name
            )));
        }
        if !is_array && p.declared_array {
            return Err(EditError::InvalidValue(format!(
                "`{}[]` is an array, so it needs a list",
                p.name
            )));
        }
        let rendered = if is_array {
            let mut style = self.style(Some(class));
            self.adopt_array_layout(&mut style, p);
            render_value(value, &style)?
        } else {
            validate(value)?;
            render_inline(value, true)
        };
        Ok(vec![replace(span, rendered)])
    }

    fn add_property(
        &self,
        class: &ConfigPath,
        name: &str,
        value: &NewValue,
        position: &Position,
    ) -> Result<Vec<TextEdit>, EditError> {
        check_member_name(name)?;
        let body = self.writable_body(class)?;
        if declared(body.members, name, None) {
            return Err(EditError::Exists(format!("`{name}`")));
        }
        let style = self.style(body.class);
        let item = property_text(name, value, &style)?;
        let index = index_for(&member_names(body.members), position)?;
        Ok(self.insert(&body, position, index, &style, &item))
    }

    fn add_class(
        &self,
        parent: &ConfigPath,
        name: &str,
        base: Option<&str>,
        forward: bool,
        position: &Position,
    ) -> Result<Vec<TextEdit>, EditError> {
        check_member_name(name)?;
        if let Some(base) = base {
            check_base_name(base)?;
        }
        let body = self.writable_body(parent)?;
        if declared(body.members, name, None) {
            return Err(EditError::Exists(format!("`{name}`")));
        }
        let style = self.style(body.class);
        let all = flatten(&self.file.members, true);
        let colon = colon_style(self.text, scope(self.file, body.class), &all);
        let item = class_text(name, base, forward, &style, &colon);
        let index = index_for(&member_names(body.members), position)?;
        Ok(self.insert(&body, position, index, &style, &item))
    }

    fn remove(&self, target: &Target) -> Result<Vec<TextEdit>, EditError> {
        let span = match target {
            Target::Class(path) => self.locate_class(path)?.1.spans.statement,
            Target::Property { .. } => self.property(target)?.1.spans.statement,
        };
        Ok(vec![self.removal(span)])
    }

    fn removal(&self, s: Span) -> TextEdit {
        let t = self.text;
        if own_line_before(t, s.start) && t[s.end..line_end(t, s.end)].trim().is_empty() {
            return delete(line_start(t, s.start), line_range_with_eol(t, s.end).end);
        }
        let trailing = t[s.end..].len() - t[s.end..].trim_start_matches([' ', '\t']).len();
        let end = s.end + trailing;
        let more_on_line = end < t.len() && !matches!(t.as_bytes()[end], b'\r' | b'\n');
        if more_on_line {
            delete(s.start, end)
        } else {
            let before = &t[..s.start];
            delete(
                s.start - (before.len() - before.trim_end_matches([' ', '\t']).len()),
                s.end,
            )
        }
    }

    fn rename(&self, target: &Target, new_name: &str) -> Result<Vec<TextEdit>, EditError> {
        check_member_name(new_name)?;
        let (siblings, name_span, current, statement_start) = match target {
            Target::Class(path) => {
                let (body, class) = self.locate_class(path)?;
                (
                    body.members,
                    class.spans.name,
                    &class.name,
                    class.spans.statement.start,
                )
            }
            Target::Property { .. } => {
                let (class, p) = self.property(target)?;
                (
                    class.members.as_slice(),
                    p.spans.name,
                    &p.name,
                    p.spans.statement.start,
                )
            }
        };
        if current == new_name {
            return Ok(vec![]);
        }
        if declared(siblings, new_name, Some(statement_start)) {
            return Err(EditError::Exists(format!("`{new_name}`")));
        }
        Ok(vec![replace(name_span, new_name)])
    }

    fn set_base(&self, path: &ConfigPath, base: Option<&str>) -> Result<Vec<TextEdit>, EditError> {
        if let Some(base) = base {
            check_base_name(base)?;
        }
        let t = self.text;
        let (body, class) = self.locate_class(path)?;
        Ok(match (base, class.spans.parents) {
            (None, None) => vec![],
            (None, Some(parents)) => {
                let gap = &t[class.spans.name.end..parents.start];
                let from = if is_blank(gap) {
                    class.spans.name.end
                } else {
                    parents.start
                };
                vec![delete(from, parents.end)]
            }
            (Some(base), Some(parents)) => {
                let after_colon = &t[parents.start + 1..parents.end];
                let gap = &after_colon
                    [..after_colon.len() - after_colon.trim_start_matches([' ', '\t']).len()];
                vec![replace(parents, format!(":{gap}{base}"))]
            }
            (Some(base), None) => {
                let all = flatten(&self.file.members, true);
                let (before, after) = colon_style(t, scope(self.file, body.class), &all);
                vec![insert_at(
                    class.spans.name.end,
                    format!("{before}:{after}{base}"),
                )]
            }
        })
    }

    fn insert(
        &self,
        body: &Body<'a>,
        position: &Position,
        index: usize,
        style: &Style,
        item: &str,
    ) -> Vec<TextEdit> {
        let t = self.text;
        match self.slot(body, position, index) {
            Slot::Line(offset) => {
                let unterminated = offset == t.len() && !t.is_empty() && !t.ends_with(['\n', '\r']);
                let text = if unterminated {
                    format!("{}{}{item}", style.eol, style.indent)
                } else {
                    format!("{}{item}{}", style.indent, style.eol)
                };
                vec![insert_at(offset, text)]
            }
            Slot::Inline {
                offset,
                lead,
                trail,
            } => vec![insert_at(offset, format!("{lead}{item}{trail}"))],
            Slot::Reflow => vec![self.reflow(body, index, style, item)],
        }
    }

    fn slot(&self, body: &Body<'a>, position: &Position, index: usize) -> Slot {
        let t = self.text;
        let members = body.members;
        if let Some(class) = body.class {
            let (Some(open), Some(close)) = (class.spans.open_brace, class.spans.close_brace)
            else {
                return Slot::Line(t.len());
            };
            match position {
                Position::First => {
                    return if ends_line_after(t, open.end) {
                        Slot::Line(line_range_with_eol(t, open.end).end)
                    } else {
                        self.fallback(body, open.end, " ", "")
                    };
                }
                Position::Last => {
                    return if own_line_before(t, close.start) {
                        Slot::Line(line_start(t, close.start))
                    } else {
                        let at = members.last().map_or(open.end, |m| m.statement().end);
                        self.fallback(body, at, " ", "")
                    };
                }
                _ => {}
            }
        } else if matches!(position, Position::First) && !members.is_empty() {
            return self.before_member(body, &members[0], false);
        }
        match position {
            Position::Before(_) => self.before_member(body, &members[index], true),
            Position::After(_) => self.after_member(body, &members[index - 1]),
            _ => members
                .last()
                .map_or(Slot::Line(t.len()), |last| self.after_member(body, last)),
        }
    }

    fn after_member(&self, body: &Body<'a>, m: &Member) -> Slot {
        let t = self.text;
        let end = m.statement().end;
        if ends_line_after(t, end) {
            Slot::Line(line_range_with_eol(t, end).end)
        } else {
            self.fallback(body, end, " ", "")
        }
    }

    fn before_member(&self, body: &Body<'a>, m: &Member, keep_comment_attached: bool) -> Slot {
        let t = self.text;
        let start = m.statement().start;
        if !own_line_before(t, start) {
            return self.fallback(body, start, "", " ");
        }
        let mut at = line_start(t, start);
        while keep_comment_attached && at > 0 {
            let above = line_start(t, at - 1);
            if t[above..at].trim_start().starts_with("//") {
                at = above;
            } else {
                break;
            }
        }
        Slot::Line(at)
    }

    fn fallback(
        &self,
        body: &Body<'a>,
        offset: usize,
        lead: &'static str,
        trail: &'static str,
    ) -> Slot {
        if self.reflowable(body) {
            Slot::Reflow
        } else {
            Slot::Inline {
                offset,
                lead,
                trail,
            }
        }
    }

    fn reflowable(&self, body: &Body<'a>) -> bool {
        let t = self.text;
        let Some(class) = body.class else {
            return false;
        };
        let (Some(open), Some(close)) = (class.spans.open_brace, class.spans.close_brace) else {
            return false;
        };
        if !same_line(t, open.start, close.start) {
            return false;
        }
        let mut previous = open.end;
        for m in body.members {
            let s = m.statement();
            if !is_blank(&t[previous..s.start]) {
                return false;
            }
            previous = s.end;
        }
        is_blank(&t[previous..close.start])
    }

    fn reflow(&self, body: &Body<'a>, index: usize, style: &Style, item: &str) -> TextEdit {
        let t = self.text;
        let class = body.class.expect("reflow needs a class body");
        let (open, close) = (
            class.spans.open_brace.expect("class body"),
            class.spans.close_brace.expect("class body"),
        );
        let mut parts: Vec<&str> = body
            .members
            .iter()
            .map(|m| &t[m.statement().start..m.statement().end])
            .collect();
        parts.insert(index, item);
        let mut inner = String::new();
        for part in parts {
            inner.push_str(style.eol);
            inner.push_str(&style.indent);
            inner.push_str(part);
        }
        inner.push_str(style.eol);
        inner.push_str(leading_ws(t, class.spans.statement.start));
        replace(Span::new(open.end, close.start), inner)
    }

    fn array_insert(
        &self,
        target: &Target,
        index: usize,
        value: &NewValue,
    ) -> Result<Vec<TextEdit>, EditError> {
        let a = self.array(target)?;
        let count = a.items.len();
        if index > count {
            return Err(EditError::IndexOutOfRange { len: count, index });
        }
        validate(value)?;
        let t = self.text;
        let mut style = self.style(Some(a.class));
        self.adopt_array_layout(&mut style, a.property);
        let item = render_inline(value, style.padded_braces);

        if count == 0 {
            let inner = if style.multiline_arrays {
                format!(
                    "{eol}{}{}{item}{eol}{}",
                    style.indent,
                    style.unit,
                    style.indent,
                    eol = style.eol
                )
            } else if style.padded_braces {
                format!(" {item} ")
            } else {
                item
            };
            return Ok(vec![replace(Span::new(a.open.end, a.close.start), inner)]);
        }
        if index < count {
            let next = a.items[index];
            if own_line_before(t, next.start) {
                let line = format!("{}{item},{}", leading_ws(t, next.start), style.eol);
                return Ok(vec![insert_at(line_start(t, next.start), line)]);
            }
            let gap = inline_gap(t, a.items);
            return Ok(vec![insert_at(next.start, format!("{item},{gap}"))]);
        }
        let last = a.items[count - 1];
        let comma = comma_after(t, last.end, a.close.start);
        let terminator_end = comma.map_or(last.end, |c| c + 1);
        if own_line_before(t, last.start) && ends_line_after(t, terminator_end) {
            let trailing = if comma.is_some() { "," } else { "" };
            let line = format!("{}{item}{trailing}{}", leading_ws(t, last.start), style.eol);
            let mut edits = vec![insert_at(line_range_with_eol(t, terminator_end).end, line)];
            if comma.is_none() {
                edits.push(insert_at(last.end, ","));
            }
            return Ok(edits);
        }
        let gap = inline_gap(t, a.items);
        Ok(vec![match comma {
            Some(c) => insert_at(c + 1, format!("{gap}{item},")),
            None => insert_at(last.end, format!(",{gap}{item}")),
        }])
    }

    fn array_remove(&self, target: &Target, index: usize) -> Result<Vec<TextEdit>, EditError> {
        let a = self.array(target)?;
        let count = a.items.len();
        if index >= count {
            return Err(EditError::IndexOutOfRange { len: count, index });
        }
        let t = self.text;
        let item = a.items[index];
        let comma = comma_after(t, item.end, a.close.start);
        let terminator_end = comma.map_or(item.end, |c| c + 1);

        if own_line_before(t, item.start) && ends_line_after(t, terminator_end) {
            let mut edits = vec![delete(
                line_start(t, item.start),
                line_range_with_eol(t, terminator_end).end,
            )];
            if index == count - 1 && index > 0 && comma.is_none() {
                if let Some(c) = comma_after(t, a.items[index - 1].end, item.start) {
                    edits.push(delete(c, c + 1));
                }
            }
            return Ok(edits);
        }
        if count == 1 {
            let bare =
                is_blank(&t[a.open.end..item.start]) && is_blank(&t[terminator_end..a.close.start]);
            return Ok(vec![if bare {
                delete(a.open.end, a.close.start)
            } else {
                delete(item.start, terminator_end)
            }]);
        }
        if index + 1 < count {
            let next = a.items[index + 1].start;
            let end = if is_blank(&t[terminator_end..next]) {
                next
            } else {
                terminator_end
            };
            return Ok(vec![delete(item.start, end)]);
        }
        let previous_end = a.items[index - 1].end;
        let only_separator = t[previous_end..item.start]
            .chars()
            .all(|c| c == ',' || c == ' ' || c == '\t');
        Ok(vec![if only_separator {
            delete(previous_end, item.end)
        } else {
            delete(item.start, terminator_end)
        }])
    }

    fn array_set(
        &self,
        target: &Target,
        index: usize,
        value: &NewValue,
    ) -> Result<Vec<TextEdit>, EditError> {
        let a = self.array(target)?;
        let count = a.items.len();
        if index >= count {
            return Err(EditError::IndexOutOfRange { len: count, index });
        }
        if matches!(a.values[index], Value::Array(_) | Value::Other(_)) {
            return Err(EditError::ReadOnly(
                "Nested lists and expressions can only be replaced as a whole".into(),
            ));
        }
        validate(value)?;
        let all = flatten(&self.file.members, true);
        let padded = padded_braces(self.text, &a.class.members, &all);
        Ok(vec![replace(a.items[index], render_inline(value, padded))])
    }

    fn array_move(
        &self,
        target: &Target,
        from: usize,
        to: usize,
    ) -> Result<Vec<TextEdit>, EditError> {
        let a = self.array(target)?;
        let count = a.items.len();
        for index in [from, to] {
            if index >= count {
                return Err(EditError::IndexOutOfRange { len: count, index });
            }
        }
        let mut order: Vec<usize> = (0..count).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        let t = self.text;
        Ok((from.min(to)..=from.max(to))
            .filter(|slot| order[*slot] != *slot)
            .map(|slot| {
                let source = a.items[order[slot]];
                replace(a.items[slot], &t[source.start..source.end])
            })
            .collect())
    }

    fn set_array(&self, target: &Target, array: bool) -> Result<Vec<TextEdit>, EditError> {
        let (_, p) = self.property(target)?;
        if p.declared_array == array {
            return Ok(vec![]);
        }
        let fits = match &p.value {
            None => true,
            Some(Value::Array(_)) => array,
            Some(_) => !array,
        };
        if !fits {
            let message = if array {
                format!("`{}` has a single value, so it can't be an array", p.name)
            } else {
                format!("`{}` holds a list, so it can't drop `[]`", p.name)
            };
            return Err(EditError::InvalidValue(message));
        }
        match (array, p.spans.brackets) {
            (true, _) => Ok(vec![insert_at(p.spans.name.end, "[]")]),
            (false, Some(brackets)) => Ok(vec![delete(brackets.start, brackets.end)]),
            (false, None) => Ok(vec![]),
        }
    }

    fn remove_at(
        &self,
        class: &ConfigPath,
        name: &str,
        occurrence: usize,
    ) -> Result<Vec<TextEdit>, EditError> {
        let body = self.body(class)?;
        let span = body
            .members
            .iter()
            .filter(|m| !matches!(m, Member::Delete { .. }) && m.name().eq_ignore_ascii_case(name))
            .nth(occurrence)
            .map(|m| m.statement())
            .ok_or_else(|| {
                EditError::NotFound(format!("Member `{name}` occurrence {occurrence}"))
            })?;
        Ok(vec![self.removal(span)])
    }
}

fn member_names(members: &[Member]) -> Vec<&str> {
    members.iter().map(|m| m.name()).collect()
}

fn flatten(members: &[Member], deep: bool) -> Vec<&Member> {
    let mut out = Vec::new();
    for m in members {
        out.push(m);
        if let (true, Member::Class(c)) = (deep, m) {
            out.extend(flatten(&c.members, true));
        }
    }
    out
}

fn scope<'a>(file: &'a ConfigFile, class: Option<&'a ConfigClass>) -> &'a [Member] {
    class.map_or(file.members.as_slice(), |c| c.members.as_slice())
}

fn majority<'a>(
    members: impl Iterator<Item = &'a Member>,
    vote: &dyn Fn(&Member) -> Option<bool>,
) -> Option<bool> {
    let votes: Vec<bool> = members.filter_map(vote).collect();
    if votes.is_empty() {
        return None;
    }
    let yes = votes.iter().filter(|v| **v).count();
    Some(yes * 2 >= votes.len())
}

fn decide(
    scoped: &[Member],
    all: &[&Member],
    vote: &dyn Fn(&Member) -> Option<bool>,
) -> Option<bool> {
    majority(flatten(scoped, false).into_iter(), vote)
        .or_else(|| majority(all.iter().copied(), vote))
}

fn padded_braces(text: &str, scoped: &[Member], all: &[&Member]) -> bool {
    decide(scoped, all, &|m| match m {
        Member::Property(p) => {
            let (open, close) = p.spans.array_braces?;
            let has_items = !p.spans.value_items.is_empty();
            (has_items && same_line(text, open.start, close.start))
                .then(|| text[open.end..].starts_with([' ', '\t']))
        }
        _ => None,
    })
    .unwrap_or(true)
}

fn array_vote(text: &str, m: &Member, non_numeric_only: bool) -> Option<bool> {
    let Member::Property(p) = m else {
        return None;
    };
    let (open, close) = p.spans.array_braces?;
    let first = p.spans.value_items.first()?;
    let Some(Value::Array(items)) = &p.value else {
        return None;
    };
    if non_numeric_only
        && items
            .iter()
            .all(|i| matches!(i, Value::Number(_) | Value::Float(_)))
    {
        return None;
    }
    Some(!same_line(text, open.start, close.start) && own_line_before(text, first.start))
}

fn indent_unit(text: &str, all: &[&Member]) -> String {
    let mut tally: Vec<(String, usize)> = Vec::new();
    for &m in all {
        let Member::Class(c) = m else {
            continue;
        };
        let class_indent = leading_ws(text, c.spans.statement.start);
        for child in &c.members {
            let start = child.statement().start;
            if !own_line_before(text, start) {
                continue;
            }
            let child_indent = leading_ws(text, start);
            if child_indent.len() > class_indent.len() && child_indent.starts_with(class_indent) {
                let step = &child_indent[class_indent.len()..];
                match tally.iter_mut().find(|(s, _)| s == step) {
                    Some((_, n)) => *n += 1,
                    None => tally.push((step.to_owned(), 1)),
                }
            }
        }
    }
    let mut best: Option<&(String, usize)> = None;
    for entry in &tally {
        if best.is_none_or(|b| entry.1 > b.1) {
            best = Some(entry);
        }
    }
    best.map_or_else(|| DEFAULT_UNIT.to_owned(), |(s, _)| s.clone())
}

fn style_for(text: &str, file: &ConfigFile, class: Option<&ConfigClass>) -> Style {
    let all = flatten(&file.members, true);
    let scoped = scope(file, class);
    let unit = indent_unit(text, &all);
    let indent = scoped
        .iter()
        .map(|m| m.statement())
        .find(|s| own_line_before(text, s.start))
        .map(|s| leading_ws(text, s.start).to_owned())
        .unwrap_or_else(|| {
            class.map_or(String::new(), |c| {
                format!("{}{unit}", leading_ws(text, c.spans.statement.start))
            })
        });
    let allman = decide(scoped, &all, &|m| match m {
        Member::Class(c) => c.spans.open_brace.map(|o| own_line_before(text, o.start)),
        _ => None,
    });
    let spaced_eq = decide(scoped, &all, &|m| match m {
        Member::Property(p) if p.value.is_some() => {
            let name_end = p.spans.brackets.map_or(p.spans.name.end, |b| b.end);
            Some(p.spans.operator.start > name_end)
        }
        _ => None,
    });
    let multiline_arrays = decide(scoped, &all, &|m| array_vote(text, m, true))
        .or_else(|| decide(scoped, &all, &|m| array_vote(text, m, false)));
    Style {
        indent,
        unit,
        eol: modfoundry_core::text::dominant_eol(text),
        allman: allman.unwrap_or(true),
        spaced_eq: spaced_eq.unwrap_or(true),
        padded_braces: padded_braces(text, scoped, &all),
        multiline_arrays: multiline_arrays.unwrap_or(false),
    }
}

fn colon_style(text: &str, scoped: &[Member], all: &[&Member]) -> (String, String) {
    for m in flatten(scoped, false)
        .into_iter()
        .chain(all.iter().copied())
    {
        let Member::Class(c) = m else {
            continue;
        };
        let Some(parents) = c.spans.parents else {
            continue;
        };
        let before = &text[c.spans.name.end..parents.start];
        let after = &text[parents.start + 1..parents.end];
        let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
        let before = if is_blank(before) { before } else { "" };
        return (before.to_owned(), after[..gap].to_owned());
    }
    (String::new(), " ".into())
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Class {
        name: String,
        parents: Vec<String>,
        forward: bool,
        members: Vec<Node>,
    },
    Prop {
        name: String,
        declared_array: bool,
        append: bool,
        value: Option<Value>,
    },
    Delete(String),
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Node::Class { name, .. } | Node::Prop { name, .. } | Node::Delete(name) => name,
        }
    }
}

fn nodes(members: &[Member]) -> Vec<Node> {
    members
        .iter()
        .map(|m| match m {
            Member::Class(c) => Node::Class {
                name: c.name.clone(),
                parents: c.parents.clone(),
                forward: c.forward,
                members: nodes(&c.members),
            },
            Member::Property(p) => Node::Prop {
                name: p.name.clone(),
                declared_array: p.declared_array,
                append: p.array_append,
                value: p.value.clone(),
            },
            Member::Delete { name, .. } => Node::Delete(name.clone()),
        })
        .collect()
}

fn to_value(v: &NewValue) -> Value {
    match v {
        NewValue::Str(s) => Value::Str(s.clone()),
        NewValue::Number(n) => Value::Number(n.clone()),
        NewValue::Ident(i) => Value::Ident(i.clone()),
        NewValue::Array(items) => Value::Array(items.iter().map(to_value).collect()),
    }
}

fn model_class<'a>(
    mut current: &'a mut Vec<Node>,
    path: &[String],
) -> Result<&'a mut Vec<Node>, String> {
    for name in path {
        current = current
            .iter_mut()
            .find_map(|n| match n {
                Node::Class {
                    name: n, members, ..
                } if n.eq_ignore_ascii_case(name) => Some(members),
                _ => None,
            })
            .ok_or_else(|| format!("model: class {name} is missing"))?;
    }
    Ok(current)
}

fn model_member<'a>(
    members: &'a mut [Node],
    name: &str,
    class: bool,
) -> Result<&'a mut Node, String> {
    members
        .iter_mut()
        .find(|n| {
            n.name().eq_ignore_ascii_case(name)
                && matches!(n, Node::Class { .. }) == class
                && !matches!(n, Node::Delete(_))
        })
        .ok_or_else(|| format!("model: {name} is missing"))
}

fn model_array<'a>(root: &'a mut Vec<Node>, target: &Target) -> Result<&'a mut Vec<Value>, String> {
    let Target::Property { class, name } = target else {
        return Err("model: array ops need a property".into());
    };
    let members = model_class(root, &class.0)?;
    match model_member(members, name, false)? {
        Node::Prop {
            value: Some(Value::Array(items)),
            ..
        } => Ok(items),
        _ => Err(format!("model: {name} is not an array")),
    }
}

fn model_remove(
    root: &mut Vec<Node>,
    path: &[String],
    name: &str,
    class: bool,
) -> Result<(), String> {
    let members = model_class(root, path)?;
    let at = members
        .iter()
        .position(|n| {
            n.name().eq_ignore_ascii_case(name)
                && matches!(n, Node::Class { .. }) == class
                && !matches!(n, Node::Delete(_))
        })
        .ok_or_else(|| format!("model: {name} is missing"))?;
    members.remove(at);
    Ok(())
}

fn model(root: &mut Vec<Node>, op: &EditOp) -> Result<(), String> {
    let position_index = |members: &[Node], position: &Position| {
        let names: Vec<&str> = members.iter().map(Node::name).collect();
        index_for(&names, position).map_err(|e| e.to_string())
    };
    match op {
        EditOp::SetValue {
            property: Target::Property { class, name },
            value,
        } => {
            let members = model_class(root, &class.0)?;
            if let Node::Prop { value: slot, .. } = model_member(members, name, false)? {
                *slot = Some(to_value(value));
            }
        }
        EditOp::AddProperty {
            class,
            name,
            value,
            position,
        } => {
            let members = model_class(root, &class.0)?;
            let at = position_index(members, position)?;
            members.insert(
                at,
                Node::Prop {
                    name: name.clone(),
                    declared_array: matches!(value, NewValue::Array(_)),
                    append: false,
                    value: Some(to_value(value)),
                },
            );
        }
        EditOp::AddClass {
            parent,
            name,
            base,
            forward,
            position,
        } => {
            let members = model_class(root, &parent.0)?;
            let at = position_index(members, position)?;
            members.insert(
                at,
                Node::Class {
                    name: name.clone(),
                    parents: base.iter().cloned().collect(),
                    forward: *forward,
                    members: Vec::new(),
                },
            );
        }
        EditOp::Remove {
            target: Target::Class(path),
        } => {
            let (last, parent) = path.0.split_last().ok_or("model: empty class path")?;
            model_remove(root, parent, last, true)?;
        }
        EditOp::Remove {
            target: Target::Property { class, name },
        } => model_remove(root, &class.0, name, false)?,
        EditOp::Rename { target, new_name } => match target {
            Target::Class(path) => {
                let (last, parent) = path.0.split_last().ok_or("model: empty class path")?;
                let members = model_class(root, parent)?;
                if let Node::Class { name, .. } = model_member(members, last, true)? {
                    *name = new_name.clone();
                }
            }
            Target::Property { class, name } => {
                let members = model_class(root, &class.0)?;
                if let Node::Prop { name: slot, .. } = model_member(members, name, false)? {
                    *slot = new_name.clone();
                }
            }
        },
        EditOp::SetBase { class, base } => {
            let (last, parent) = class.0.split_last().ok_or("model: empty class path")?;
            let members = model_class(root, parent)?;
            if let Node::Class { parents, .. } = model_member(members, last, true)? {
                *parents = base.iter().cloned().collect();
            }
        }
        EditOp::ArrayInsert {
            property,
            index,
            value,
        } => {
            model_array(root, property)?.insert(*index, to_value(value));
        }
        EditOp::ArrayRemove { property, index } => {
            model_array(root, property)?.remove(*index);
        }
        EditOp::ArraySet {
            property,
            index,
            value,
        } => {
            model_array(root, property)?[*index] = to_value(value);
        }
        EditOp::ArrayMove { property, from, to } => {
            let items = model_array(root, property)?;
            let moved = items.remove(*from);
            items.insert(*to, moved);
        }
        EditOp::SetValue { .. } => return Err("model: set needs a property".into()),
        EditOp::SetArray {
            target: Target::Property { class, name },
            array,
        } => {
            let members = model_class(root, &class.0)?;
            if let Node::Prop { declared_array, .. } = model_member(members, name, false)? {
                *declared_array = *array;
            }
        }
        EditOp::SetArray { .. } => return Err("model: set needs a property".into()),
        EditOp::RemoveAt {
            class,
            name,
            occurrence,
        } => {
            let members = model_class(root, &class.0)?;
            let at = members
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    !matches!(n, Node::Delete(_)) && n.name().eq_ignore_ascii_case(name)
                })
                .map(|(i, _)| i)
                .nth(*occurrence)
                .ok_or_else(|| format!("model: {name} occurrence {occurrence} is missing"))?;
            members.remove(at);
        }
    }
    Ok(())
}
