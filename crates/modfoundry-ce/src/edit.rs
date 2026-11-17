//! The XML edit engine: an op is planned into byte splices over the original
//! text; the tree is only read, never re-printed.

use serde::{Deserialize, Serialize};

use crate::xml::{
    escape_attr, escape_text, is_name_char, is_name_start, is_ws, NodeId, NodeKind, XmlDoc,
};
use modfoundry_core::text::{dominant_eol, line_end, line_range_with_eol, line_start};

/// One byte splice over the original text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub offset: usize,
    pub len: usize,
    pub text: String,
}

/// How one path step picks among the same-named child elements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    Nth(usize),
    Attr { name: String, value: String },
}

/// One element step: a name and the pick that selects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub pick: Pick,
}

/// A path from the document's top level; empty means the top level itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct XmlPath(pub Vec<Step>);

/// An element to build from scratch.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewElement {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: Option<String>,
    pub children: Vec<NewElement>,
}

/// Where a new element lands among its parent's child elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    First,
    Last,
    Index(usize),
}

/// The formatting a file already uses, so inserted text matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub unit: String,
    pub eol: &'static str,
    pub self_close: &'static str,
}

/// One XML-level change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XmlOp {
    SetAttr {
        element: XmlPath,
        name: String,
        value: String,
    },
    RemoveAttr {
        element: XmlPath,
        name: String,
    },
    SetText {
        element: XmlPath,
        text: String,
    },
    RenameElement {
        element: XmlPath,
        name: String,
    },
    InsertElement {
        parent: XmlPath,
        at: Position,
        element: NewElement,
    },
    RemoveElement {
        element: XmlPath,
    },
    WrapTop {
        name: String,
    },
}

/// Why an op could not be planned; `Display` is the user-facing line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Ambiguous(String),
    #[error("{0}")]
    Exists(String),
    #[error("{0}")]
    InvalidName(String),
    #[error("{0}")]
    InvalidValue(String),
    #[error("{0}")]
    HasChildren(String),
    #[error("Only {len} match, so index {index} is out of range.")]
    IndexOutOfRange { len: usize, index: usize },
    #[error("Fix the syntax errors first.")]
    Unparseable,
    #[error("{0}")]
    Unsupported(String),
}

/// Resolves `path` to the element it names.
pub fn resolve(doc: &XmlDoc, path: &XmlPath) -> Result<NodeId, EditError> {
    if path.0.is_empty() {
        return Err(EditError::NotFound("Nothing is selected.".to_string()));
    }
    let mut current: Option<NodeId> = None;
    for step in &path.0 {
        let candidates = match current {
            Some(id) => doc.child_elements(id),
            None => doc.top_elements(),
        };
        let named: Vec<NodeId> = candidates
            .into_iter()
            .filter(|id| doc.element(*id).is_some_and(|e| e.name == step.name))
            .collect();
        current = Some(match &step.pick {
            Pick::Nth(index) => match named.get(*index) {
                Some(&id) => id,
                None if named.is_empty() => {
                    return Err(EditError::NotFound(format!("No `<{}>` here.", step.name)));
                }
                None => {
                    return Err(EditError::IndexOutOfRange {
                        len: named.len(),
                        index: *index,
                    })
                }
            },
            Pick::Attr { name, value } => {
                let matches: Vec<NodeId> = named
                    .iter()
                    .copied()
                    .filter(|id| {
                        doc.attr(*id, name)
                            .is_some_and(|a| a.value.eq_ignore_ascii_case(value))
                    })
                    .collect();
                match matches.as_slice() {
                    [id] => *id,
                    [] => {
                        return Err(EditError::NotFound(format!(
                            "No `<{}>` named `{value}` here.",
                            step.name
                        )))
                    }
                    _ => {
                        return Err(EditError::Ambiguous(format!(
                            "Two `<{}>` elements match `{value}`.",
                            step.name
                        )))
                    }
                }
            }
        });
    }
    Ok(current.expect("a non-empty path resolves to a node"))
}

/// Builds the `Nth`-only path that names `node`.
pub fn path_to(doc: &XmlDoc, node: NodeId) -> XmlPath {
    let mut steps = Vec::new();
    let mut current = Some(node);
    while let Some(id) = current {
        let Some(element) = doc.element(id) else {
            break;
        };
        let siblings = match doc.nodes[id].parent {
            Some(parent) => doc.child_elements(parent),
            None => doc.top_elements(),
        };
        let index = siblings
            .iter()
            .take_while(|&&sibling| sibling != id)
            .filter(|&&sibling| doc.element(sibling).is_some_and(|e| e.name == element.name))
            .count();
        steps.push(Step {
            name: element.name.clone(),
            pick: Pick::Nth(index),
        });
        current = doc.nodes[id].parent;
    }
    steps.reverse();
    XmlPath(steps)
}

/// Parses `text` and plans `op` over it.
pub fn plan(text: &str, op: &XmlOp) -> Result<Vec<TextEdit>, EditError> {
    let doc = crate::xml::parse(text);
    plan_in(text, &doc, op)
}

/// Plans `op` over an already parsed document.
pub fn plan_in(text: &str, doc: &XmlDoc, op: &XmlOp) -> Result<Vec<TextEdit>, EditError> {
    if doc.blocking() {
        return Err(EditError::Unparseable);
    }
    match op {
        XmlOp::SetAttr {
            element,
            name,
            value,
        } => {
            check_name(name)?;
            let id = resolve(doc, element)?;
            let el = doc.element(id).expect("resolve returns an element");
            match el.attrs.iter().find(|attr| attr.name == *name) {
                Some(attr) => Ok(vec![TextEdit {
                    offset: attr.value_span.start,
                    len: attr.value_span.end - attr.value_span.start,
                    text: match attr.quote {
                        Some(quote) => escape_attr(value, quote),
                        None => format!("\"{}\"", escape_attr(value, '"')),
                    },
                }]),
                None => {
                    let at = el.attrs.last().map_or(el.name_span.end, |a| a.span.end);
                    Ok(vec![TextEdit {
                        offset: at,
                        len: 0,
                        text: format!(" {name}=\"{}\"", escape_attr(value, '"')),
                    }])
                }
            }
        }
        XmlOp::RemoveAttr { element, name } => {
            let id = resolve(doc, element)?;
            let el = doc.element(id).expect("resolve returns an element");
            let index = el
                .attrs
                .iter()
                .position(|attr| attr.name == *name)
                .ok_or_else(|| {
                    EditError::NotFound(format!("`<{}>` has no `{name}` attribute.", el.name))
                })?;
            let start = if index == 0 {
                el.name_span.end
            } else {
                el.attrs[index - 1].span.end
            };
            let attr = &el.attrs[index];
            Ok(vec![TextEdit {
                offset: start,
                len: attr.span.end - start,
                text: String::new(),
            }])
        }
        XmlOp::SetText {
            element,
            text: value,
        } => {
            let id = resolve(doc, element)?;
            let el = doc.element(id).expect("resolve returns an element");
            if el.self_closing {
                let mut start = el.open.end - 2;
                let bytes = text.as_bytes();
                while start > el.open.start && is_ws(bytes[start - 1]) {
                    start -= 1;
                }
                Ok(vec![TextEdit {
                    offset: start,
                    len: el.open.end - start,
                    text: format!(">{}</{}>", escape_text(value), el.name),
                }])
            } else if el
                .children
                .iter()
                .any(|child| matches!(doc.nodes[*child].kind, NodeKind::Element(_)))
            {
                Err(EditError::HasChildren(format!(
                    "`<{}>` has child elements, so its text can't be set.",
                    el.name
                )))
            } else if let Some(span) = doc.text_span(id) {
                Ok(vec![TextEdit {
                    offset: span.start,
                    len: span.end - span.start,
                    text: escape_text(value),
                }])
            } else if !doc.text_of(id).is_empty() {
                Err(EditError::Unsupported(format!(
                    "`<{}>` mixes its value with other content; edit it in Source.",
                    el.name
                )))
            } else {
                Ok(vec![TextEdit {
                    offset: el.open.end,
                    len: 0,
                    text: escape_text(value),
                }])
            }
        }
        XmlOp::RenameElement { element, name } => {
            check_name(name)?;
            let id = resolve(doc, element)?;
            let el = doc.element(id).expect("resolve returns an element");
            let mut edits = vec![TextEdit {
                offset: el.name_span.start,
                len: el.name_span.end - el.name_span.start,
                text: name.clone(),
            }];
            if let Some(close) = el.close {
                edits.push(TextEdit {
                    offset: close.start + 2,
                    len: el.name.len(),
                    text: name.clone(),
                });
            }
            Ok(edits)
        }
        XmlOp::InsertElement {
            parent,
            at,
            element,
        } => plan_insert(text, doc, parent, at, element),
        XmlOp::RemoveElement { element } => {
            let id = resolve(doc, element)?;
            let span = doc.nodes[id].span;
            let first_line = line_start(text, span.start);
            let last_line = line_end(text, span.end);
            let bare = text[first_line..span.start]
                .bytes()
                .chain(text[span.end..last_line].bytes())
                .all(|b| b == b' ' || b == b'\t');
            if bare {
                let end = line_range_with_eol(text, last_line).end;
                Ok(vec![TextEdit {
                    offset: first_line,
                    len: end - first_line,
                    text: String::new(),
                }])
            } else {
                Ok(vec![TextEdit {
                    offset: span.start,
                    len: span.end - span.start,
                    text: String::new(),
                }])
            }
        }
        XmlOp::WrapTop { name } => {
            check_name(name)?;
            let tops = doc.top_elements();
            let first = *tops
                .first()
                .ok_or_else(|| EditError::NotFound("There is nothing to wrap.".to_string()))?;
            let last = *tops.last().expect("tops is non-empty");
            if tops.len() == 1 && doc.element(first).is_some_and(|el| el.name == *name) {
                return Err(EditError::Exists(format!(
                    "The file is already wrapped in `<{name}>`."
                )));
            }
            let eol = infer_style(text, doc).eol;
            let offset = top_offset(doc, line_start(text, doc.nodes[first].span.start));
            Ok(vec![
                TextEdit {
                    offset,
                    len: 0,
                    text: format!("<{name}>{eol}"),
                },
                TextEdit {
                    offset: doc.nodes[last].span.end,
                    len: 0,
                    text: format!("{eol}</{name}>"),
                },
            ])
        }
    }
}

fn plan_insert(
    text: &str,
    doc: &XmlDoc,
    parent: &XmlPath,
    at: &Position,
    element: &NewElement,
) -> Result<Vec<TextEdit>, EditError> {
    let style = infer_style(text, doc);
    if parent.0.is_empty() {
        return plan_insert_top(text, doc, at, element, &style);
    }
    let id = resolve(doc, parent)?;
    let el = doc.element(id).expect("resolve returns an element");
    let parent_indent = line_indent(text, doc.nodes[id].span.start);
    let child_indent = format!("{parent_indent}{}", style.unit);

    if el.self_closing {
        let mut ws = el.open.end - 2;
        while ws > el.open.start && is_ws(text.as_bytes()[ws - 1]) {
            ws -= 1;
        }
        let rendered = render(element, &child_indent, &style)?;
        let replacement = format!(
            ">{}{rendered}{}{parent_indent}</{}>",
            style.eol, style.eol, el.name
        );
        return Ok(vec![TextEdit {
            offset: ws,
            len: el.open.end - ws,
            text: replacement,
        }]);
    }

    let children = doc.child_elements(id);
    if !children.is_empty() {
        let first = children[0];
        let own_line = text[el.open.end..doc.nodes[first].span.start].contains('\n');
        if own_line {
            let indent = line_indent(text, doc.nodes[first].span.start);
            let rendered = render(element, indent, &style)?;
            let offset = match at {
                Position::Last => {
                    line_range_with_eol(
                        text,
                        doc.nodes[*children.last().expect("non-empty")].span.end,
                    )
                    .end
                }
                Position::First => {
                    let line = line_start(text, doc.nodes[first].span.start);
                    extend_over_comments(text, doc, line)
                }
                Position::Index(index) => {
                    if *index > children.len() {
                        return Err(EditError::IndexOutOfRange {
                            len: children.len(),
                            index: *index,
                        });
                    }
                    if *index == children.len() {
                        line_range_with_eol(
                            text,
                            doc.nodes[*children.last().expect("non-empty")].span.end,
                        )
                        .end
                    } else {
                        let line = line_start(text, doc.nodes[children[*index]].span.start);
                        extend_over_comments(text, doc, line)
                    }
                }
            };
            return Ok(vec![TextEdit {
                offset,
                len: 0,
                text: appended_text(text, offset, &rendered, style.eol),
            }]);
        }
        let offset = match at {
            Position::Last => close_start(el)?,
            Position::First => doc.nodes[first].span.start,
            Position::Index(index) => {
                if *index > children.len() {
                    return Err(EditError::IndexOutOfRange {
                        len: children.len(),
                        index: *index,
                    });
                }
                if *index == children.len() {
                    close_start(el)?
                } else {
                    doc.nodes[children[*index]].span.start
                }
            }
        };
        return Ok(vec![TextEdit {
            offset,
            len: 0,
            text: render(element, "", &style)?,
        }]);
    }

    let close = el
        .close
        .ok_or_else(|| EditError::Unsupported(format!("`<{}>` has no end tag.", el.name)))?;
    let rendered = render(element, &child_indent, &style)?;
    if text[el.open.end..close.start].contains('\n') {
        Ok(vec![TextEdit {
            offset: line_range_with_eol(text, el.open.end).end,
            len: 0,
            text: format!("{rendered}{}", style.eol),
        }])
    } else {
        Ok(vec![TextEdit {
            offset: el.open.end,
            len: close.start - el.open.end,
            text: format!("{}{rendered}{}{parent_indent}", style.eol, style.eol),
        }])
    }
}

fn close_start(el: &crate::xml::Element) -> Result<usize, EditError> {
    el.close
        .map(|close| close.start)
        .ok_or_else(|| EditError::Unsupported(format!("`<{}>` has no end tag.", el.name)))
}

fn plan_insert_top(
    text: &str,
    doc: &XmlDoc,
    at: &Position,
    element: &NewElement,
    style: &Style,
) -> Result<Vec<TextEdit>, EditError> {
    let tops = doc.top_elements();
    let rendered = render(element, "", style)?;
    let offset = match at {
        Position::Last => {
            let last = *tops
                .last()
                .ok_or_else(|| EditError::NotFound("Nothing is selected.".to_string()))?;
            line_range_with_eol(text, doc.nodes[last].span.end).end
        }
        Position::First => {
            let first = *tops
                .first()
                .ok_or_else(|| EditError::NotFound("Nothing is selected.".to_string()))?;
            let line = line_start(text, doc.nodes[first].span.start);
            extend_over_comments(text, doc, line)
        }
        Position::Index(index) => {
            if *index > tops.len() {
                return Err(EditError::IndexOutOfRange {
                    len: tops.len(),
                    index: *index,
                });
            }
            if *index == tops.len() {
                let last = *tops.last().expect("non-empty");
                line_range_with_eol(text, doc.nodes[last].span.end).end
            } else {
                let line = line_start(text, doc.nodes[tops[*index]].span.start);
                extend_over_comments(text, doc, line)
            }
        }
    };
    let offset = top_offset(doc, offset);
    Ok(vec![TextEdit {
        offset,
        len: 0,
        text: appended_text(text, offset, &rendered, style.eol),
    }])
}

/// An offset at the start of the file never lands before a leading BOM, which
/// has to stay first.
fn top_offset(doc: &XmlDoc, offset: usize) -> usize {
    if doc.bom {
        offset.max('\u{FEFF}'.len_utf8())
    } else {
        offset
    }
}

/// The text of an appended element: `rendered` plus a line break, unless
/// something already sits before it on its line — a break added there would
/// not be reclaimed by removing the element again, so it is appended bare.
fn appended_text(text: &str, offset: usize, rendered: &str, eol: &str) -> String {
    let lead = &text[line_start(text, offset)..offset];
    if lead.is_empty() || lead == "\u{FEFF}" {
        format!("{rendered}{eol}")
    } else {
        rendered.to_string()
    }
}

fn line_indent(text: &str, offset: usize) -> &str {
    let start = line_start(text, offset);
    let bytes = text.as_bytes();
    let mut i = start;
    while i < offset.min(bytes.len()) && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    &text[start..i]
}

fn extend_over_comments(text: &str, doc: &XmlDoc, mut line: usize) -> usize {
    while line > 0 {
        let prev = line_start(text, line - 1);
        if prev == line {
            break;
        }
        let end = line_end(text, prev);
        let content = &text[prev..end];
        let lead = content.len() - content.trim_start().len();
        let is_comment = !content.trim().is_empty()
            && doc
                .node_at(prev + lead)
                .is_some_and(|id| matches!(doc.nodes[id].kind, NodeKind::Comment));
        if is_comment {
            line = prev;
        } else {
            break;
        }
    }
    line
}

/// The formatting `text` already uses, so inserted text matches it.
pub fn infer_style(text: &str, doc: &XmlDoc) -> Style {
    Style {
        unit: infer_unit(text, doc),
        eol: dominant_eol(text),
        self_close: infer_self_close(text, doc),
    }
}

fn infer_self_close(text: &str, doc: &XmlDoc) -> &'static str {
    for node in &doc.nodes {
        let NodeKind::Element(el) = &node.kind else {
            continue;
        };
        if el.self_closing {
            return if el.open.end >= 3 && is_ws(text.as_bytes()[el.open.end - 3]) {
                " />"
            } else {
                "/>"
            };
        }
    }
    "/>"
}

fn infer_unit(text: &str, doc: &XmlDoc) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for id in 0..doc.nodes.len() {
        let Some(el) = doc.element(id) else {
            continue;
        };
        let Some(first) = doc.child_elements_iter(id).next() else {
            continue;
        };
        if !text[el.open.end..doc.nodes[first].span.start].contains('\n') {
            continue;
        }
        let parent = line_indent(text, doc.nodes[id].span.start);
        let child = line_indent(text, doc.nodes[first].span.start);
        let Some(unit) = child.strip_prefix(parent) else {
            continue;
        };
        if unit.is_empty() || !unit.bytes().all(|b| b == b' ' || b == b'\t') {
            continue;
        }
        match counts.iter_mut().find(|(seen, _)| seen == unit) {
            Some((_, count)) => *count += 1,
            None => counts.push((unit.to_string(), 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(unit, _)| unit)
        .unwrap_or_else(|| "    ".to_string())
}

/// Renders `element` as text at `indent`, using `style`'s line break, indent
/// unit and self-closing form.
pub fn render(element: &NewElement, indent: &str, style: &Style) -> Result<String, EditError> {
    check_name(&element.name)?;
    let mut out = String::from(indent);
    out.push('<');
    out.push_str(&element.name);
    for (name, value) in &element.attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        out.push_str(&escape_attr(value, '"'));
        out.push('"');
    }
    if element.text.is_none() && element.children.is_empty() {
        out.push_str(style.self_close);
        return Ok(out);
    }
    out.push('>');
    if element.children.is_empty() {
        out.push_str(&escape_text(element.text.as_deref().unwrap_or("")));
    } else {
        let child_indent = format!("{indent}{}", style.unit);
        for child in &element.children {
            out.push_str(style.eol);
            out.push_str(&render(child, &child_indent, style)?);
        }
        out.push_str(style.eol);
        out.push_str(indent);
    }
    out.push_str("</");
    out.push_str(&element.name);
    out.push('>');
    Ok(out)
}

/// Applies `ops` in order, re-parsing between each.
pub fn plan_all(text: &str, ops: &[XmlOp]) -> Result<String, EditError> {
    let mut current = text.to_string();
    for op in ops {
        let edits = plan(&current, op)?;
        current = apply(&current, &edits);
    }
    Ok(current)
}

/// Applies sorted, non-overlapping edits back to front.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    for edit in edits.iter().rev() {
        out.replace_range(edit.offset..edit.offset + edit.len, &edit.text);
    }
    out
}

fn check_name(name: &str) -> Result<(), EditError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(EditError::InvalidName(format!(
            "`{name}` is not a valid name."
        )))
    }
}

fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_appended_element_round_trips_without_a_final_newline() {
        let text = "\t<type name=\"Alpha\">\n\t\t<nominal>1</nominal>\n\t</type>";
        let insert = XmlOp::InsertElement {
            parent: XmlPath::default(),
            at: Position::Last,
            element: NewElement {
                name: "type".to_string(),
                attrs: vec![("name".to_string(), "__mf_probe".to_string())],
                ..NewElement::default()
            },
        };
        let remove = XmlOp::RemoveElement {
            element: XmlPath(vec![Step {
                name: "type".to_string(),
                pick: Pick::Nth(1),
            }]),
        };
        assert_eq!(plan_all(text, &[insert, remove]), Ok(text.to_string()));
    }
}
