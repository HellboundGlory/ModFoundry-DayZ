//! Lossless parser for Central Economy XML.
//!
//! Every byte of the input belongs to exactly one node, so [`XmlDoc::rebuild`]
//! returns the source verbatim. Well-formed input parses clean; broken input is
//! recovered from and reported as `MF-X001`, `MF-X002` or `MF-X010`, so a file
//! still yields its rows and every problem at once.

use modfoundry_core::text::line_end;
use modfoundry_core::{Diagnostic, Severity, Span};
use serde::{Deserialize, Serialize};

/// Index of a [`Node`] in [`XmlDoc::nodes`].
pub type NodeId = usize;

/// A parsed document: the node arena, the top-level order, and any issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XmlDoc {
    /// Arena of every node; a [`NodeId`] indexes it.
    pub nodes: Vec<Node>,
    /// Top-level nodes in source order.
    pub top: Vec<NodeId>,
    /// Parse issues; empty for a clean parse.
    pub issues: Vec<Diagnostic>,
    /// Whether the text started with U+FEFF; spans still count its bytes.
    pub bom: bool,
}

/// One node: its parent, raw byte span, and kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub span: Span,
    pub kind: NodeKind,
}

/// What a node is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Element(Element),
    /// `trimmed` is the raw extent without XML whitespace at either end; entities stay whole.
    Text {
        text: String,
        trimmed: Span,
    },
    Comment,
    Decl,
    Pi,
    Cdata {
        text: String,
    },
    Doctype,
}

/// An element and its attributes and children.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    pub name: String,
    pub name_span: Span,
    pub attrs: Vec<Attr>,
    /// `<` through the `>` that ends the start tag (the whole tag when self-closing).
    pub open: Span,
    /// `</name>`; `None` when self-closing or recovered without a close tag.
    pub close: Option<Span>,
    pub self_closing: bool,
    pub children: Vec<NodeId>,
}

/// An attribute; `value` is entity-decoded, spans stay raw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attr {
    pub name: String,
    pub name_span: Span,
    pub value: String,
    pub value_span: Span,
    pub quote: Option<char>,
    pub span: Span,
}

/// Parses `text` into a lossless document, recovering from anything broken.
pub fn parse(text: &str) -> XmlDoc {
    let mut parser = Parser {
        text,
        nodes: Vec::new(),
        top: Vec::new(),
        issues: Vec::new(),
    };
    let mut stack: Vec<NodeId> = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        if text.as_bytes()[pos] == b'<' {
            pos = parser.parse_markup(pos, &mut stack);
        } else {
            let end = parser.next_lt(pos);
            parser.push_text(&stack, pos, end);
            if stack.is_empty()
                && !parser.is_leading_bom(pos, end)
                && text[pos..end].chars().any(|c| !c.is_whitespace())
            {
                parser.issue(
                    "MF-X010",
                    Severity::Hint,
                    Span::new(pos, end),
                    "Text outside any element.",
                );
            }
            pos = end;
        }
    }
    for &id in stack.iter().rev() {
        parser.implicit_close(id, text.len());
    }
    parser.issues.sort_by_key(|d| d.span.map_or(0, |s| s.start));
    XmlDoc {
        nodes: parser.nodes,
        top: parser.top,
        issues: parser.issues,
        bom: text.starts_with('\u{FEFF}'),
    }
}

/// Decodes the five named XML entities plus decimal and hex character
/// references; an unknown `&name;` is kept as written.
pub fn decode(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.find(';') {
            Some(j) => {
                let entity = &tail[..=j];
                match decode_entity(entity) {
                    Some(decoded) => out.push_str(&decoded),
                    None => out.push_str(entity),
                }
                rest = &tail[j + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Escapes `&`, `<` and `>` for element text.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes `&`, `<` and `quote` for a quoted attribute value; `quote` becomes
/// `&quot;` or `&apos;`, any other quote character is left alone.
pub fn escape_attr(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if quote == '"' => out.push_str("&quot;"),
            '\'' if quote == '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "&lt;" => Some("<".to_string()),
        "&gt;" => Some(">".to_string()),
        "&amp;" => Some("&".to_string()),
        "&quot;" => Some("\"".to_string()),
        "&apos;" => Some("'".to_string()),
        _ => {
            let body = entity.strip_prefix("&#")?.strip_suffix(';')?;
            let code = match body.strip_prefix('x').or_else(|| body.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => body.parse::<u32>().ok()?,
            };
            char::from_u32(code).map(String::from)
        }
    }
}

pub(crate) fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || c as u32 >= 0x80
}

pub(crate) fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '.' || c == '-'
}

pub(crate) fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

struct Parser<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    top: Vec<NodeId>,
    issues: Vec<Diagnostic>,
}

impl Parser<'_> {
    fn push_child(&mut self, stack: &[NodeId], span: Span, kind: NodeKind) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: stack.last().copied(),
            span,
            kind,
        });
        match stack.last() {
            Some(&parent) => self.element_mut(parent).children.push(id),
            None => self.top.push(id),
        }
        id
    }

    fn push_text(&mut self, stack: &[NodeId], start: usize, end: usize) {
        let raw = &self.text.as_bytes()[start..end];
        let leading = raw.iter().take_while(|&&b| is_ws(b)).count();
        let trailing = raw[leading..]
            .iter()
            .rev()
            .take_while(|&&b| is_ws(b))
            .count();
        let trimmed = Span::new(start + leading, end - trailing);
        let text = decode(&self.text[start..end]);
        self.push_child(
            stack,
            Span::new(start, end),
            NodeKind::Text { text, trimmed },
        );
    }

    fn issue(&mut self, code: &str, severity: Severity, span: Span, message: impl Into<String>) {
        self.issues
            .push(Diagnostic::at(code, severity, span, message));
    }

    fn element_mut(&mut self, id: NodeId) -> &mut Element {
        match &mut self.nodes[id].kind {
            NodeKind::Element(element) => element,
            _ => unreachable!("only element nodes are used as parents"),
        }
    }

    fn element_name(&self, id: NodeId) -> &str {
        match &self.nodes[id].kind {
            NodeKind::Element(element) => &element.name,
            _ => "",
        }
    }

    fn is_leading_bom(&self, start: usize, end: usize) -> bool {
        start == 0 && self.text.starts_with('\u{FEFF}') && end == '\u{FEFF}'.len_utf8()
    }

    fn at_file_start(&self, pos: usize) -> bool {
        pos == if self.text.starts_with('\u{FEFF}') {
            3
        } else {
            0
        }
    }

    fn skip_ws(&self, mut pos: usize) -> usize {
        let bytes = self.text.as_bytes();
        while matches!(bytes.get(pos), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            pos += 1;
        }
        pos
    }

    fn next_lt(&self, pos: usize) -> usize {
        match self.text[pos..].find('<') {
            Some(i) => pos + i,
            None => self.text.len(),
        }
    }

    fn next_lt_after(&self, pos: usize) -> usize {
        match self.text[pos + 1..].find('<') {
            Some(i) => pos + 1 + i,
            None => self.text.len(),
        }
    }

    fn parse_name(&self, pos: usize) -> Option<(String, usize, usize)> {
        let mut chars = self.text[pos..].chars();
        let first = chars.next()?;
        if !is_name_start(first) {
            return None;
        }
        let mut end = pos + first.len_utf8();
        for c in chars {
            if !is_name_char(c) {
                break;
            }
            end += c.len_utf8();
        }
        Some((self.text[pos..end].to_string(), pos, end))
    }

    fn parse_markup(&mut self, pos: usize, stack: &mut Vec<NodeId>) -> usize {
        if self.text[pos..].starts_with("<!--") {
            return self.parse_comment(pos, stack);
        }
        if self.text[pos..].starts_with("<![CDATA[") {
            return self.parse_cdata(pos, stack);
        }
        if self.text[pos..].starts_with("<!DOCTYPE") {
            return self.parse_doctype(pos, stack);
        }
        if self.text[pos..].starts_with("<?") {
            return self.parse_pi(pos, stack);
        }
        if self.text[pos..].starts_with("</") {
            return self.parse_end_tag(pos, stack);
        }
        let named = self.text[pos + 1..]
            .chars()
            .next()
            .is_some_and(is_name_start);
        if named {
            return self.parse_start_tag(pos, stack);
        }
        self.literal_text(pos, stack)
    }

    fn literal_text(&mut self, pos: usize, stack: &[NodeId]) -> usize {
        let end = self.next_lt_after(pos);
        self.push_text(stack, pos, end);
        self.issue(
            "MF-X001",
            Severity::Error,
            Span::new(pos, pos + 1),
            "`<` starts a tag; write `&lt;` for the character.",
        );
        end
    }

    fn parse_comment(&mut self, pos: usize, stack: &[NodeId]) -> usize {
        let body = pos + 4;
        match self.text[body..].find("-->") {
            Some(i) => {
                let end = body + i + 3;
                self.push_child(stack, Span::new(pos, end), NodeKind::Comment);
                self.report_double_hyphen(body, end - 3);
                end
            }
            None => {
                let end = self.text.len();
                self.push_child(stack, Span::new(pos, end), NodeKind::Comment);
                self.issue(
                    "MF-X001",
                    Severity::Error,
                    Span::new(pos, pos + 4),
                    "This comment is never closed.",
                );
                end
            }
        }
    }

    fn report_double_hyphen(&mut self, start: usize, end: usize) {
        let mut from = start;
        while let Some(i) = self.text[from..end].find("--") {
            let at = from + i;
            self.issue(
                "MF-X010",
                Severity::Hint,
                Span::new(at, at + 2),
                "XML doesn't allow `--` inside a comment. The game accepts it; other tools may not.",
            );
            from = at + 2;
        }
    }

    fn parse_cdata(&mut self, pos: usize, stack: &[NodeId]) -> usize {
        let body = pos + 9;
        match self.text[body..].find("]]>") {
            Some(i) => {
                let end = body + i + 3;
                let text = self.text[body..end - 3].to_string();
                self.push_child(stack, Span::new(pos, end), NodeKind::Cdata { text });
                end
            }
            None => {
                let end = self.text.len();
                let text = self.text[body..end].to_string();
                self.push_child(stack, Span::new(pos, end), NodeKind::Cdata { text });
                self.issue(
                    "MF-X001",
                    Severity::Error,
                    Span::new(pos, pos + 9),
                    "This CDATA section is never closed.",
                );
                end
            }
        }
    }

    fn parse_doctype(&mut self, pos: usize, stack: &[NodeId]) -> usize {
        let bytes = self.text.as_bytes();
        let mut depth = 0i32;
        let mut at = pos + 9;
        while let Some(&b) = bytes.get(at) {
            match b {
                b'[' => depth += 1,
                b']' => depth -= 1,
                b'>' if depth == 0 => {
                    let end = at + 1;
                    self.push_child(stack, Span::new(pos, end), NodeKind::Doctype);
                    return end;
                }
                _ => {}
            }
            at += 1;
        }
        let end = self.text.len();
        self.push_child(stack, Span::new(pos, end), NodeKind::Doctype);
        self.issue(
            "MF-X001",
            Severity::Error,
            Span::new(pos, pos + 9),
            "This DOCTYPE is never closed.",
        );
        end
    }

    fn is_declaration(&self, pos: usize) -> bool {
        let after = &self.text[pos + 2..];
        after.starts_with("xml") && {
            let rest = &after[3..];
            rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('?')
        }
    }

    fn parse_pi(&mut self, pos: usize, stack: &[NodeId]) -> usize {
        let decl = self.is_declaration(pos);
        let kind = if decl { NodeKind::Decl } else { NodeKind::Pi };
        match self.text[pos + 2..].find("?>") {
            Some(i) => {
                let end = pos + 2 + i + 2;
                self.push_child(stack, Span::new(pos, end), kind);
                if decl && !self.at_file_start(pos) {
                    self.issue(
                        "MF-X010",
                        Severity::Hint,
                        Span::new(pos, end),
                        "The XML declaration belongs on the first line.",
                    );
                }
                end
            }
            None => {
                let end = self.text.len();
                self.push_child(stack, Span::new(pos, end), kind);
                let message = if decl {
                    "This XML declaration is never closed."
                } else {
                    "This processing instruction is never closed."
                };
                self.issue("MF-X001", Severity::Error, Span::new(pos, pos + 2), message);
                end
            }
        }
    }

    fn parse_start_tag(&mut self, start: usize, stack: &mut Vec<NodeId>) -> usize {
        let (name, name_start, after_name) = match self.parse_name(start + 1) {
            Some(named) => named,
            None => return self.literal_text(start, stack),
        };
        let name_span = Span::new(name_start, after_name);
        let mut attrs = Vec::new();
        let mut pos = after_name;
        let (open, self_closing) = loop {
            pos = self.skip_ws(pos);
            match self.text.as_bytes().get(pos) {
                Some(b'>') => break (Span::new(start, pos + 1), false),
                Some(b'/') if self.text.as_bytes().get(pos + 1) == Some(&b'>') => {
                    break (Span::new(start, pos + 2), true);
                }
                Some(_) if self.text[pos..].chars().next().is_some_and(is_name_start) => {
                    let (attr, next) = self.parse_attr(pos);
                    attrs.push(attr);
                    pos = next;
                }
                _ => return self.never_closed_tag(start, name, name_span, attrs, pos, stack),
            }
        };
        let id = self.push_start_element(name, name_span, attrs, open, self_closing, stack);
        if !self_closing {
            stack.push(id);
        }
        open.end
    }

    fn push_start_element(
        &mut self,
        name: String,
        name_span: Span,
        attrs: Vec<Attr>,
        open: Span,
        self_closing: bool,
        stack: &[NodeId],
    ) -> NodeId {
        let element = Element {
            name,
            name_span,
            attrs,
            open,
            close: None,
            self_closing,
            children: Vec::new(),
        };
        let id = self.push_child(stack, open, NodeKind::Element(element));
        self.report_duplicate_attrs(id);
        id
    }

    fn never_closed_tag(
        &mut self,
        start: usize,
        name: String,
        name_span: Span,
        attrs: Vec<Attr>,
        end: usize,
        stack: &[NodeId],
    ) -> usize {
        self.push_start_element(name, name_span, attrs, Span::new(start, end), false, stack);
        self.issue(
            "MF-X001",
            Severity::Error,
            Span::new(start, name_span.end),
            "This tag is never closed.",
        );
        end
    }

    fn report_duplicate_attrs(&mut self, id: NodeId) {
        let (element_name, duplicates) = {
            let NodeKind::Element(element) = &self.nodes[id].kind else {
                return;
            };
            let mut duplicates = Vec::new();
            for (index, attr) in element.attrs.iter().enumerate() {
                if element.attrs[..index]
                    .iter()
                    .any(|prev| prev.name == attr.name)
                {
                    duplicates.push((attr.name.clone(), attr.name_span));
                }
            }
            (element.name.clone(), duplicates)
        };
        for (name, span) in duplicates {
            self.issue(
                "MF-X002",
                Severity::Error,
                span,
                format!("`{name}` appears twice on `<{element_name}>`."),
            );
        }
    }

    fn parse_attr(&mut self, pos: usize) -> (Attr, usize) {
        let (name, name_start, after_name) = self
            .parse_name(pos)
            .expect("callers only reach here on a name start");
        let name_span = Span::new(name_start, after_name);
        let empty = |name: String, name_span: Span| Attr {
            name,
            name_span,
            value: String::new(),
            value_span: Span::new(name_span.end, name_span.end),
            quote: None,
            span: name_span,
        };
        let mut at = self.skip_ws(after_name);
        if self.text.as_bytes().get(at) != Some(&b'=') {
            self.issue(
                "MF-X001",
                Severity::Error,
                name_span,
                format!("`{name}` has no value."),
            );
            return (empty(name, name_span), after_name);
        }
        at = self.skip_ws(at + 1);
        match self.text.as_bytes().get(at).copied() {
            Some(quote @ (b'"' | b'\'')) => {
                let quote = quote as char;
                let value_start = at + 1;
                let line_end = line_end(self.text, value_start);
                match self.text[value_start..line_end].find(quote) {
                    Some(i) => {
                        let value_end = value_start + i;
                        let attr = Attr {
                            name,
                            name_span,
                            value: decode(&self.text[value_start..value_end]),
                            value_span: Span::new(value_start, value_end),
                            quote: Some(quote),
                            span: Span::new(name_start, value_end + 1),
                        };
                        (attr, value_end + 1)
                    }
                    None => {
                        self.issue(
                            "MF-X001",
                            Severity::Error,
                            Span::new(at, at + 1),
                            "This attribute value is never closed.",
                        );
                        let attr = Attr {
                            name,
                            name_span,
                            value: decode(&self.text[value_start..line_end]),
                            value_span: Span::new(value_start, line_end),
                            quote: Some(quote),
                            span: Span::new(name_start, line_end),
                        };
                        (attr, line_end)
                    }
                }
            }
            Some(b) if !is_ws(b) && b != b'/' && b != b'>' => {
                let bytes = self.text.as_bytes();
                let mut end = at;
                while let Some(&b) = bytes.get(end) {
                    if is_ws(b) || b == b'/' || b == b'>' {
                        break;
                    }
                    end += 1;
                }
                let shown = self.text[at..end].to_string();
                self.issue(
                    "MF-X010",
                    Severity::Hint,
                    Span::new(at, end),
                    format!("Quote the value: `{name}=\"{shown}\"`. The game accepts it; other tools may not."),
                );
                let attr = Attr {
                    name,
                    name_span,
                    value: decode(&self.text[at..end]),
                    value_span: Span::new(at, end),
                    quote: None,
                    span: Span::new(name_start, end),
                };
                (attr, end)
            }
            _ => {
                self.issue(
                    "MF-X001",
                    Severity::Error,
                    name_span,
                    format!("`{name}` has no value."),
                );
                (empty(name, name_span), at)
            }
        }
    }

    fn parse_end_tag(&mut self, pos: usize, stack: &mut Vec<NodeId>) -> usize {
        let Some((name, _, after_name)) = self.parse_name(pos + 2) else {
            return self.literal_text(pos, stack);
        };
        let after = self.skip_ws(after_name);
        if self.text.as_bytes().get(after) != Some(&b'>') {
            let end = self.next_lt_after(pos);
            self.push_text(stack, pos, end);
            self.issue(
                "MF-X001",
                Severity::Error,
                Span::new(pos, after_name),
                "This tag is never closed.",
            );
            return end;
        }
        let close = Span::new(pos, after + 1);
        match stack.iter().rposition(|&id| self.element_name(id) == name) {
            Some(index) => {
                for i in (index + 1..stack.len()).rev() {
                    let id = stack[i];
                    self.implicit_close(id, pos);
                }
                let id = stack[index];
                stack.truncate(index);
                self.close_element(id, close);
                after + 1
            }
            None => {
                self.push_text(stack, pos, after + 1);
                self.issue(
                    "MF-X001",
                    Severity::Error,
                    close,
                    format!("</{name}> closes nothing."),
                );
                after + 1
            }
        }
    }

    fn close_element(&mut self, id: NodeId, close: Span) {
        let start = match &mut self.nodes[id].kind {
            NodeKind::Element(element) => {
                element.close = Some(close);
                element.open.start
            }
            _ => return,
        };
        self.nodes[id].span = Span::new(start, close.end);
    }

    fn implicit_close(&mut self, id: NodeId, end: usize) {
        let (name, name_span, start) = match &mut self.nodes[id].kind {
            NodeKind::Element(element) => {
                element.close = None;
                (element.name.clone(), element.name_span, element.open.start)
            }
            _ => return,
        };
        self.nodes[id].span = Span::new(start, end);
        self.issue(
            "MF-X001",
            Severity::Error,
            name_span,
            format!("<{name}> is never closed."),
        );
    }
}

impl XmlDoc {
    /// The element for `id`, or `None` when the node is not an element.
    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.nodes.get(id)?.kind {
            NodeKind::Element(element) => Some(element),
            _ => None,
        }
    }

    /// Whether the document carries an `MF-X001` or `MF-X002` issue.
    pub fn blocking(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.code == "MF-X001" || issue.code == "MF-X002")
    }

    /// The only top-level element, or `None` when there are zero or several.
    pub fn root(&self) -> Option<NodeId> {
        let mut elements = self.top_elements();
        if elements.len() == 1 {
            elements.pop()
        } else {
            None
        }
    }

    /// Top-level element nodes, in source order.
    pub fn top_elements(&self) -> Vec<NodeId> {
        self.top
            .iter()
            .copied()
            .filter(|id| matches!(self.nodes[*id].kind, NodeKind::Element(_)))
            .collect()
    }

    /// Direct child elements of `id`.
    pub fn child_elements(&self, id: NodeId) -> Vec<NodeId> {
        self.child_elements_iter(id).collect()
    }

    /// Direct child elements of `id`, without collecting them.
    pub fn child_elements_iter(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.element(id)
            .into_iter()
            .flat_map(|element| element.children.iter().copied())
            .filter(|child| matches!(self.nodes[*child].kind, NodeKind::Element(_)))
    }

    /// Direct child elements of `id` whose name equals `name` exactly.
    pub fn children_named(&self, id: NodeId, name: &str) -> Vec<NodeId> {
        self.child_elements_iter(id)
            .filter(|child| self.element(*child).is_some_and(|e| e.name == name))
            .collect()
    }

    /// The first direct child element of `id` whose name equals `name` exactly.
    pub fn first_child_named(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.child_elements_iter(id)
            .find(|child| self.element(*child).is_some_and(|e| e.name == name))
    }

    /// The attribute of `id` named `name`, matched exactly.
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&Attr> {
        self.element(id)?
            .attrs
            .iter()
            .find(|attr| attr.name == name)
    }

    /// Direct `Text` and `Cdata` children, decoded, joined and trimmed.
    pub fn text_of(&self, id: NodeId) -> String {
        let Some(element) = self.element(id) else {
            return String::new();
        };
        let mut out = String::new();
        for &child in &element.children {
            match &self.nodes[child].kind {
                NodeKind::Text { text, .. } | NodeKind::Cdata { text } => out.push_str(text),
                _ => {}
            }
        }
        out.trim().to_string()
    }

    /// The trimmed extent of the element's one non-blank `Text` child, or of its only, blank, one.
    pub fn text_span(&self, id: NodeId) -> Option<Span> {
        let element = self.element(id)?;
        let mut filled: Option<Span> = None;
        for &child in &element.children {
            match &self.nodes[child].kind {
                NodeKind::Text { trimmed, .. } if trimmed.start < trimmed.end => {
                    if filled.replace(*trimmed).is_some() {
                        return None;
                    }
                }
                NodeKind::Cdata { text } if !text.trim().is_empty() => return None,
                _ => {}
            }
        }
        match (filled, element.children.as_slice()) {
            (Some(span), _) => Some(span),
            (None, [only]) => match &self.nodes[*only].kind {
                NodeKind::Text { trimmed, .. } => Some(*trimmed),
                _ => None,
            },
            _ => None,
        }
    }

    /// The innermost node whose span holds `offset`.
    pub fn node_at(&self, offset: usize) -> Option<NodeId> {
        self.top.iter().find_map(|id| self.deepest_at(*id, offset))
    }

    fn deepest_at(&self, id: NodeId, offset: usize) -> Option<NodeId> {
        let node = self.nodes.get(id)?;
        if offset < node.span.start || offset >= node.span.end {
            return None;
        }
        if let NodeKind::Element(element) = &node.kind {
            for &child in &element.children {
                if let Some(inner) = self.deepest_at(child, offset) {
                    return Some(inner);
                }
            }
        }
        Some(id)
    }

    /// The source slices the top-level nodes cover, in order.
    pub fn rebuild(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for &id in &self.top {
            let span = self.nodes[id].span;
            out.push_str(&text[span.start..span.end]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles(text: &str) -> XmlDoc {
        let doc = parse(text);
        assert_eq!(doc.rebuild(text), text, "rebuild mismatch for {text:?}");
        doc
    }

    fn slice(text: &str, span: Span) -> &str {
        &text[span.start..span.end]
    }

    const TYPES: &str = "<?xml version=\"1.0\"?><!--c--><types><type name=\"A\"><nominal>10</nominal><flags count_in_map=\"1\" deloot=\"0\"/></type></types>";

    #[test]
    fn types_sample_tree() {
        let doc = tiles(TYPES);
        let kinds: Vec<&NodeKind> = doc.top.iter().map(|id| &doc.nodes[*id].kind).collect();
        assert!(matches!(kinds[0], NodeKind::Decl));
        assert!(matches!(kinds[1], NodeKind::Comment));
        assert!(matches!(kinds[2], NodeKind::Element(_)));
        assert_eq!(kinds.len(), 3);

        let root = doc.root().expect("one top-level element");
        assert_eq!(doc.element(root).unwrap().name, "types");

        let types = doc.child_elements(root);
        assert_eq!(types.len(), 1);
        let ty = doc.element(types[0]).unwrap();
        assert_eq!(ty.name, "type");

        assert_eq!(doc.children_named(root, "type"), vec![types[0]]);
        assert!(doc.children_named(root, "TYPE").is_empty());

        assert_eq!(doc.attr(types[0], "name").unwrap().value, "A");

        let nominal = doc.children_named(types[0], "nominal")[0];
        assert_eq!(doc.text_of(nominal), "10");
        assert_eq!(slice(TYPES, doc.text_span(nominal).unwrap()), "10");
    }

    #[test]
    fn the_text_span_keeps_edge_entities_whole() {
        let text = "<types><type name=\"A\"><nominal>&#32;10&#10;</nominal></type></types>";
        let doc = tiles(text);
        let ty = doc.children_named(doc.root().unwrap(), "type")[0];
        let nominal = doc.children_named(ty, "nominal")[0];
        assert_eq!(slice(text, doc.text_span(nominal).unwrap()), "&#32;10&#10;");
    }

    #[test]
    fn spans_slice_to_what_they_claim() {
        let doc = tiles(TYPES);
        let root = doc.root().unwrap();
        let types = doc.element(root).unwrap();
        assert_eq!(slice(TYPES, types.name_span), "types");
        assert!(slice(TYPES, types.open).starts_with("<types"));
        assert_eq!(slice(TYPES, types.open), "<types>");
        assert_eq!(slice(TYPES, types.close.unwrap()), "</types>");

        let ty = doc.children_named(root, "type")[0];
        let flags = doc.children_named(ty, "flags")[0];
        let attr = doc.attr(flags, "count_in_map").unwrap();
        assert_eq!(slice(TYPES, attr.span), "count_in_map=\"1\"");
        assert_eq!(slice(TYPES, attr.value_span), "1");
        assert_eq!(slice(TYPES, attr.name_span), "count_in_map");
        assert_eq!(attr.quote, Some('"'));
        assert!(doc.element(flags).unwrap().self_closing);
        assert!(doc.element(flags).unwrap().close.is_none());
    }

    #[test]
    fn tiles_whitespace_and_syntax_variants() {
        for text in [
            "<a>\n  <b>x</b>\n</a>",
            "<a>\r\n\t<b>x</b>\r\n</a>",
            "<a/>",
            "<a />",
            "<a b='1'/>",
            "<a b = \"1\" />",
            "\u{FEFF}<a/>",
            "<!--1--><a><!--2--><b/><!--3--></a><!--4-->",
            "<?pi data?><a/>",
            "<!DOCTYPE x SYSTEM \"y\"><a/>",
            "<!DOCTYPE x [ <!ELEMENT a > ]><a/>",
            "<a><![CDATA[x<y&z]]></a>",
            "",
            "   \n\t ",
        ] {
            tiles(text);
        }
    }

    #[test]
    fn bom_flag() {
        assert!(parse("\u{FEFF}<a/>").bom);
        assert!(!parse("<a/>").bom);
        let doc = tiles("\u{FEFF}<a/>");
        assert_eq!(doc.top.len(), 2);
    }

    #[test]
    fn entities_in_text_and_attributes() {
        let text = "<a b=\"&lt;x&gt;&amp;&quot;&apos;\">&#65;&#x41;&nbsp;</a>";
        let doc = tiles(text);
        let a = doc.root().unwrap();
        assert_eq!(doc.text_of(a), "AA&nbsp;");
        assert_eq!(doc.attr(a, "b").unwrap().value, "<x>&\"'");
    }

    #[test]
    fn decode_named_decimal_hex_and_unknown() {
        assert_eq!(decode("&lt;&gt;&amp;&quot;&apos;"), "<>&\"'");
        assert_eq!(decode("&#65;&#x41;"), "AA");
        assert_eq!(decode("&nbsp;"), "&nbsp;");
        assert_eq!(decode("plain"), "plain");
        assert_eq!(decode("a & b"), "a & b");
    }

    #[test]
    fn escape_helpers_invert_decode() {
        assert_eq!(escape_text("a&b<c>d"), "a&amp;b&lt;c&gt;d");
        assert_eq!(escape_text("\" '"), "\" '");
        assert_eq!(escape_attr("a&b<c>\"d", '"'), "a&amp;b&lt;c>&quot;d");
        assert_eq!(escape_attr("a&b<c>'d", '\''), "a&amp;b&lt;c>&apos;d");
        for s in ["& < > \" '", "", "<"] {
            assert_eq!(decode(&escape_text(s)), s);
            assert_eq!(decode(&escape_attr(s, '"')), s);
            assert_eq!(decode(&escape_attr(s, '\'')), s);
        }
    }

    #[test]
    fn blocking_reports_error_issues() {
        assert!(!parse(TYPES).blocking());
        assert!(parse("<a>").blocking());
        assert!(parse("<a b=\"1\" b=\"2\"/>").blocking());
        assert!(!parse("<e a=1/>").blocking());
    }

    #[test]
    fn multibyte_text_and_attribute_spans_stay_on_char_boundaries() {
        let text = "<a n=\"wörld\">héllo</a>";
        let doc = tiles(text);
        let a = doc.root().unwrap();
        assert_eq!(doc.text_of(a), "héllo");
        assert_eq!(doc.attr(a, "n").unwrap().value, "wörld");
        let el = doc.element(a).unwrap();
        let mut spans = vec![doc.nodes[a].span, el.name_span, doc.text_span(a).unwrap()];
        spans.push(doc.attr(a, "n").unwrap().value_span);
        for span in spans {
            assert!(text.is_char_boundary(span.start));
            assert!(text.is_char_boundary(span.end));
        }
    }

    #[test]
    fn fragment_of_two_top_level_elements() {
        let doc = tiles("<a/><b/>");
        assert_eq!(doc.root(), None);
        assert_eq!(doc.top_elements().len(), 2);
        assert!(doc.issues.is_empty());
    }

    #[test]
    fn node_at_finds_the_innermost_node() {
        let text = "<a>hi</a> <b/>";
        let doc = tiles(text);

        let tag = doc.node_at(1).unwrap();
        assert_eq!(doc.element(tag).unwrap().name, "a");

        let inner = doc.node_at(4).unwrap();
        assert!(matches!(doc.nodes[inner].kind, NodeKind::Text { .. }));

        let gap = doc.node_at(9).unwrap();
        assert!(matches!(doc.nodes[gap].kind, NodeKind::Text { .. }));

        let last = doc.node_at(text.len() - 1).unwrap();
        assert_eq!(doc.element(last).unwrap().name, "b");
    }
}
