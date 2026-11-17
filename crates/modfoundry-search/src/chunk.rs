//! Source files cut into searchable pieces at class, method, config class, widget and heading edges.

use std::ops::Range;

use modfoundry_config::ast::{classes, ConfigClass, Member};
use modfoundry_config::parser::parse;
use modfoundry_core::text::{line_end, line_start, LineIndex};
use modfoundry_core::Span;
use modfoundry_layout::parse::{parse as parse_layout, Widget};
use modfoundry_script::items::{ClassItem, EnumItem, FileItems, MethodItem};
use serde::Serialize;

/// The kind of source a chunk was cut from.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Script,
    Config,
    Layout,
    Docs,
}

/// One searchable piece of a file, with 1-based inclusive lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chunk {
    pub kind: Kind,
    pub lines: [usize; 2],
    pub class: Option<String>,
    pub method: Option<String>,
    pub text: String,
}

/// Script source cut into method, function, class member and enum chunks.
pub fn script_chunks(source: &str) -> Vec<Chunk> {
    let file = Source::new(source);
    let mut views = vec![
        modfoundry_script::extract(source),
        modfoundry_script::extract::extract_other_arms(source),
    ];
    views.extend(modfoundry_script::extract::extract_nested_arms(source));
    let items = merge_views(views);
    let mut chunks = Vec::new();
    let mut covered = Vec::new();
    for class in &items.classes {
        push_class(&file, class, &mut chunks);
        covered.push(commented(source, class.span.start)..class.span.end);
    }
    for function in &items.functions {
        push_function(&file, function, &mut chunks);
        covered.push(commented(source, function.span.start)..function.span.end);
    }
    for item in &items.enums {
        let range = enum_range(source, item);
        chunks.push(file.chunk(Kind::Script, range.clone(), None, None));
        covered.push(range);
    }
    push_globals(&file, &items, covered, &mut chunks);
    chunks.sort_by_key(|chunk| chunk.lines[0]);
    chunks
}

/// Folds each later view into the first, matching a declaration by the span it covers.
fn merge_views(views: Vec<FileItems>) -> FileItems {
    let mut merged = FileItems::default();
    for view in views {
        for class in view.classes {
            match merged
                .classes
                .iter()
                .position(|held| overlaps(held.span, class.span))
            {
                Some(index) => {
                    let taken: Vec<Span> = merged
                        .classes
                        .iter()
                        .flat_map(|held| held.methods.iter().map(|method| method.span))
                        .collect();
                    let fresh: Vec<MethodItem> = class
                        .methods
                        .into_iter()
                        .filter(|method| !taken.iter().any(|span| overlaps(*span, method.span)))
                        .collect();
                    let held = &mut merged.classes[index];
                    held.methods.extend(fresh);
                    // An arm can close the class later than the first view does.
                    held.span.end = held.span.end.max(class.span.end);
                }
                None => merged.classes.push(class),
            }
        }
        add_unless_overlapping(&mut merged.functions, view.functions, |item| item.span);
        add_unless_overlapping(&mut merged.enums, view.enums, |item| item.span);
        merged.globals.extend(view.globals);
        merged.typedefs.extend(view.typedefs);
    }
    for class in &mut merged.classes {
        class.methods.sort_by_key(|method| method.span.start);
    }
    merged
}

fn add_unless_overlapping<T>(held: &mut Vec<T>, items: Vec<T>, span: impl Fn(&T) -> Span) {
    for item in items {
        if !held.iter().any(|kept| overlaps(span(kept), span(&item))) {
            held.push(item);
        }
    }
}

fn overlaps(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end
}

fn push_function(file: &Source, function: &MethodItem, out: &mut Vec<Chunk>) {
    let range = commented(file.text, function.span.start)..function.span.end;
    out.push(file.chunk(Kind::Script, range, None, Some(&function.name)));
}

/// One piece per stretch of top-level source that declares a global or a typedef.
fn push_globals(
    file: &Source,
    items: &FileItems,
    mut covered: Vec<Range<usize>>,
    out: &mut Vec<Chunk>,
) {
    let anchors: Vec<usize> = items
        .globals
        .iter()
        .map(|field| field.name_span.start)
        .chain(items.typedefs.iter().map(|item| item.name_span.start))
        .collect();
    covered.sort_by_key(|range| range.start);
    let mut cursor = 0;
    for range in covered {
        if range.start > cursor {
            push_stretch(file, cursor..range.start, &anchors, out);
        }
        cursor = cursor.max(range.end);
    }
    push_stretch(file, cursor..file.text.len(), &anchors, out);
}

fn push_stretch(file: &Source, range: Range<usize>, anchors: &[usize], out: &mut Vec<Chunk>) {
    for piece in pieces(&file.text[range.clone()]) {
        let part = range.start + piece.start..range.start + piece.end;
        if anchors.iter().any(|anchor| part.contains(anchor)) {
            out.push(file.chunk(Kind::Script, part, None, None));
        }
    }
}

/// One chunk per method, plus the class head and every later stretch of members between methods.
fn push_class(file: &Source, class: &ClassItem, out: &mut Vec<Chunk>) {
    let name = Some(class.name.as_str());
    let mut cursor = commented(file.text, class.span.start);
    for (index, method) in class.methods.iter().enumerate() {
        let start = commented(file.text, method.span.start).max(cursor);
        if index == 0 || has_members(&file.text[cursor..start]) {
            out.push(file.chunk(Kind::Script, cursor..start, name, None));
        }
        out.push(file.chunk(
            Kind::Script,
            start..method.span.end,
            name,
            Some(&method.name),
        ));
        cursor = method.span.end;
    }
    let end = class.span.end.max(cursor);
    if class.methods.is_empty() || has_members(&file.text[cursor..end]) {
        out.push(file.chunk(Kind::Script, cursor..end, name, None));
    }
}

fn has_members(text: &str) -> bool {
    text.chars()
        .any(|c| !c.is_whitespace() && !matches!(c, '{' | '}' | ';'))
}

/// Moves `at` back over the comment lines directly above it, when it starts its line.
fn commented(source: &str, at: usize) -> usize {
    let mut start = line_start(source, at);
    if !source[start..at].trim().is_empty() {
        return at;
    }
    while start > 0 {
        let above = line_start(source, start - 1);
        if !is_comment_line(source[above..start].trim()) {
            break;
        }
        start = above;
    }
    start
}

fn is_comment_line(line: &str) -> bool {
    if line.starts_with("//") || line.starts_with('*') {
        return true;
    }
    if !line.contains("/*") {
        return line.ends_with("*/");
    }
    let mut code = String::new();
    let mut rest = line;
    loop {
        let Some(open) = rest.find("/*") else {
            code.push_str(rest);
            break;
        };
        code.push_str(&rest[..open]);
        let Some(close) = rest[open + 2..].find("*/") else {
            break;
        };
        rest = &rest[open + 2 + close + 2..];
    }
    if let Some(at) = code.find("//") {
        code.truncate(at);
    }
    code.trim().is_empty()
}

/// Config source cut into one chunk per class with a body; small nested classes fold into their parent.
pub fn config_chunks(source: &str) -> Vec<Chunk> {
    let file = Source::new(source);
    let parsed = parse(source);
    let mut chunks = Vec::new();
    for class in classes(&parsed.file.members).filter(|class| !class.forward) {
        let cut = cut_config_class(&file, class, &class.name);
        chunks.push(config_piece(
            &file,
            class,
            class.name.clone(),
            cut.text,
            cut.end_line,
        ));
        chunks.extend(cut.pieces);
    }
    chunks
}

const FOLD_CHARS: usize = 200;

/// A class's piece text and the pieces of its nested classes that did not fold, in source order.
struct ConfigCut {
    text: String,
    pieces: Vec<Chunk>,
    end_line: usize,
}

fn cut_config_class(file: &Source, class: &ConfigClass, path: &str) -> ConfigCut {
    let mut text = config_class_text(file.text, class);
    // A top-level class such as CfgVehicles holds the items themselves; only their sub-blocks fold.
    let folds = path.contains('/');
    let mut end_line = config_class_end_line(file, class);
    let mut pieces = Vec::new();
    for child in classes(&class.members).filter(|child| !child.forward) {
        let child_path = format!("{path}/{}", child.name);
        let cut = cut_config_class(file, child, &child_path);
        if folds && cut.text.chars().count() < FOLD_CHARS {
            end_line = end_line.max(cut.end_line);
            text.push('\n');
            text.push_str(&folded(&cut.text));
        } else {
            pieces.push(config_piece(
                file,
                child,
                child_path,
                cut.text,
                cut.end_line,
            ));
        }
        pieces.extend(cut.pieces);
    }
    ConfigCut {
        text,
        pieces,
        end_line,
    }
}

/// The last line a class's own properties reach, or its header line when it has none.
fn config_class_end_line(file: &Source, class: &ConfigClass) -> usize {
    let header = file.lines.line_of(class.spans.statement.start);
    class
        .members
        .iter()
        .filter_map(|member| match member {
            Member::Property(property) => Some(property),
            _ => None,
        })
        .map(|property| {
            file.lines
                .line_of(property.spans.statement.end.saturating_sub(1))
        })
        .fold(header, usize::max)
}

fn config_piece(
    file: &Source,
    class: &ConfigClass,
    path: String,
    text: String,
    end_line: usize,
) -> Chunk {
    let first = file.lines.line_of(class.spans.statement.start);
    Chunk {
        kind: Kind::Config,
        lines: [first, end_line.max(first)],
        class: Some(path),
        method: None,
        text,
    }
}

/// A piece text written as a class body inside its parent: `header {`, the body lines, then `};`.
fn folded(text: &str) -> String {
    match text.split_once('\n') {
        Some((header, body)) => format!("{header} {{\n{body}\n}};"),
        None => format!("{text} {{\n}};"),
    }
}

fn config_class_text(source: &str, class: &ConfigClass) -> String {
    let properties = class.members.iter().filter_map(|member| match member {
        Member::Property(property) => Some(&source[span_range(property.spans.statement)]),
        _ => None,
    });
    std::iter::once(config_header(source, class))
        .chain(properties)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The class line up to its opening brace, whichever line the brace is on.
fn config_header<'a>(source: &'a str, class: &ConfigClass) -> &'a str {
    let start = class.spans.statement.start;
    let line = line_end(source, start);
    let end = class
        .spans
        .open_brace
        .map_or(line, |brace| brace.start.min(line));
    let header = &source[start..end];
    header
        .split_once("//")
        .map_or(header, |(code, _)| code)
        .trim_end()
}

/// Layout source cut into one chunk per widget, named by the slash path of its ancestors.
pub fn layout_chunks(source: &str) -> Result<Vec<Chunk>, String> {
    let widgets = parse_layout(source).map_err(|err| err.to_string())?;
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();
    for widget in &widgets {
        push_widget(&lines, widget, None, &mut chunks);
    }
    Ok(chunks)
}

fn push_widget(lines: &[&str], widget: &Widget, parent: Option<&str>, out: &mut Vec<Chunk>) {
    let name = widget
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&widget.class);
    let path = match parent {
        Some(parent) => format!("{parent}/{name}"),
        None => name.to_string(),
    };
    let first = widget.pos.line as usize;
    let lead = match widget.children.first() {
        Some(child) => widget
            .properties
            .iter()
            .take_while(|property| property.pos.line < child.pos.line)
            .count(),
        None => widget.properties.len(),
    };
    let last = widget.properties[..lead]
        .last()
        .map_or(first, |property| property.pos.line as usize);
    let mut text: Vec<&str> = lines[first - 1..last].to_vec();
    text.extend(
        widget.properties[lead..]
            .iter()
            .map(|property| lines[property.pos.line as usize - 1]),
    );
    out.push(Chunk {
        kind: Kind::Layout,
        lines: [first, last],
        class: Some(path.clone()),
        method: None,
        text: text.join("\n"),
    });
    for child in &widget.children {
        push_widget(lines, child, Some(&path), out);
    }
}

/// Markdown or plain text cut at headings, blank lines and line breaks.
pub fn docs_chunks(source: &str) -> Vec<Chunk> {
    let file = Source::new(source);
    let mut chunks = Vec::new();
    for (heading, section) in sections(source) {
        for piece in pieces(&source[section.clone()]) {
            let range = section.start + piece.start..section.start + piece.end;
            if !source[range.clone()].trim().is_empty() {
                chunks.push(file.chunk(Kind::Docs, range, heading, None));
            }
        }
    }
    chunks
}

const PIECE_CHARS: usize = 1500;

fn pieces(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut group: Option<Range<usize>> = None;
    for paragraph in paragraphs(text) {
        // Blank lines never change what fits, and measuring them made long blank runs quadratic.
        if text[paragraph.clone()].trim().is_empty() {
            if let Some(group) = &mut group {
                group.end = paragraph.end;
            }
            continue;
        }
        let joined = match &group {
            Some(group) => group.start..paragraph.end,
            None => paragraph.clone(),
        };
        if fits(&text[joined.clone()]) {
            group = Some(joined);
            continue;
        }
        out.extend(group.take());
        if fits(&text[paragraph.clone()]) {
            group = Some(paragraph);
        } else {
            split_at_line_breaks(text, paragraph, &mut out);
        }
    }
    out.extend(group);
    out
}

fn paragraphs(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let (mut start, mut end) = (0, 0);
    for line in text.split_inclusive('\n') {
        end += line.len();
        if line.trim().is_empty() {
            out.push(start..end);
            start = end;
        }
    }
    if start < text.len() {
        out.push(start..text.len());
    }
    out
}

fn split_at_line_breaks(text: &str, mut rest: Range<usize>, out: &mut Vec<Range<usize>>) {
    loop {
        let body = &text[rest.clone()];
        if fits(body) {
            out.push(rest);
            return;
        }
        let limit = char_offset(body, PIECE_CHARS);
        let cut = body[..limit]
            .rfind('\n')
            .map_or(limit, |line_break| line_break + 1);
        out.push(rest.start..rest.start + cut);
        rest.start += cut;
    }
}

fn fits(text: &str) -> bool {
    let body = text.trim_end();
    char_offset(body, PIECE_CHARS) == body.len()
}

fn char_offset(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(at, _)| at)
}

fn sections(source: &str) -> Vec<(Option<&str>, Range<usize>)> {
    let mut sections = Vec::new();
    let mut heading = None;
    let mut begin = 0;
    let mut offset = 0;
    let mut fence: Option<&str> = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(open)) => fence = Some(open),
            (Some(open), Some(close)) if open == close => fence = None,
            (None, None) => {
                if let Some(title) = heading_text(line) {
                    sections.push((heading, begin..offset));
                    heading = Some(title);
                    begin = offset;
                }
            }
            _ => {}
        }
        offset += line.len();
    }
    sections.push((heading, begin..source.len()));
    sections
}

/// A Markdown ATX heading: up to three spaces, one to six `#`, then a space or the line end.
fn heading_text(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    let after = &rest[hashes..];
    let ends_marker = after.is_empty() || after.starts_with(char::is_whitespace);
    if indent > 3 || !(1..=6).contains(&hashes) || !ends_marker {
        return None;
    }
    let title = after.trim();
    let open = title.trim_end_matches('#');
    let closed = open.is_empty() || open.ends_with(char::is_whitespace);
    Some(if closed { open.trim_end() } else { title })
}

struct Source<'a> {
    text: &'a str,
    lines: LineIndex,
}

impl<'a> Source<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: LineIndex::new(text),
        }
    }

    fn chunk(
        &self,
        kind: Kind,
        range: Range<usize>,
        class: Option<&str>,
        method: Option<&str>,
    ) -> Chunk {
        let range = self.trimmed(range);
        Chunk {
            kind,
            lines: self.lines_of(&range),
            class: class.map(str::to_owned),
            method: method.map(str::to_owned),
            text: self.text[range].to_owned(),
        }
    }

    fn trimmed(&self, range: Range<usize>) -> Range<usize> {
        let raw = &self.text[range.clone()];
        let start = range.start + (raw.len() - raw.trim_start().len());
        start..start + raw.trim().len()
    }

    fn lines_of(&self, range: &Range<usize>) -> [usize; 2] {
        let last = range.end.saturating_sub(1).max(range.start);
        [self.lines.line_of(range.start), self.lines.line_of(last)]
    }
}

fn span_range(span: Span) -> Range<usize> {
    span.start..span.end
}

fn enum_range(source: &str, item: &EnumItem) -> Range<usize> {
    let start = commented(source, line_start(source, item.name_span.start));
    start..item.span.end
}
