//! Per-file Enforce declaration extraction.

use crate::items::{
    ClassItem, EnumItem, EnumMember, FieldItem, FileItems, MethodItem, ParamItem, TypeRef,
    TypedefItem,
};
use modfoundry_core::Span;
use modfoundry_enforce::{first_arms, other_arms, parse_tree};
use tree_sitter::Node;

/// Extract every declaration in `source`. Broken input yields whatever
/// survived parse recovery.
pub fn extract(source: &str) -> FileItems {
    extract_view(&first_arms(source))
}

/// The declarations of the view that keeps each preprocessor block's other arm.
pub fn extract_other_arms(source: &str) -> FileItems {
    extract_view(&other_arms(source))
}

/// The declarations of each view that keeps an arm neither basic view keeps.
pub fn extract_nested_arms(source: &str) -> Vec<FileItems> {
    modfoundry_enforce::nested_arm_views(source)
        .iter()
        .map(|view| extract_view(view))
        .collect()
}

pub(crate) fn extract_view(selected: &str) -> FileItems {
    let tree = parse_tree(selected);
    let mut out = FileItems::default();
    walk(tree.root_node(), selected, None, &mut out);
    out
}

fn walk(node: Node, src: &str, class: Option<usize>, out: &mut FileItems) {
    match node.kind() {
        "decl_class" => {
            let Some(item) = build_class(node, src) else {
                return;
            };
            let idx = out.classes.len();
            out.classes.push(item);
            each_child(node, |ch| walk(ch, src, Some(idx), out));
        }
        "decl_enum" => {
            if let Some(e) = build_enum(node, src) {
                out.enums.push(e);
            }
        }
        "typedef" => {
            if class.is_none() && enclosing(node, "block").is_none() {
                if let Some(t) = build_typedef(node, src) {
                    out.typedefs.push(t);
                }
            }
        }
        "decl_method" => {
            if let Some(m) = build_method(node, src) {
                match class {
                    Some(i) => out.classes[i].methods.push(m),
                    None => out.functions.push(m),
                }
            }
        }
        "decl_field" => {
            let fields = build_fields(node, src);
            match class {
                Some(i) => out.classes[i].fields.extend(fields),
                None => out.globals.extend(fields),
            }
        }
        "decl_variable" => {
            if class.is_none() && enclosing(node, "block").is_none() {
                out.globals.extend(build_fields(node, src));
            }
        }
        _ => each_child(node, |ch| walk(ch, src, class, out)),
    }
}

/// Every child of `node` with the field it fills, in order. One cursor walks them:
/// indexed child access rescans the siblings on every call.
pub(crate) fn children_with_fields<'a>(
    node: Node<'a>,
) -> impl Iterator<Item = (Option<&'static str>, Node<'a>)> {
    // A leaf skips the cursor: creating one allocates its stack.
    let mut cursor = (node.child_count() > 0).then(|| node.walk());
    let mut more = cursor
        .as_mut()
        .is_some_and(|cursor| cursor.goto_first_child());
    std::iter::from_fn(move || {
        let cursor = cursor.as_mut().filter(|_| more)?;
        let item = (cursor.field_name(), cursor.node());
        more = cursor.goto_next_sibling();
        Some(item)
    })
}

pub(crate) fn children<'a>(node: Node<'a>) -> impl Iterator<Item = Node<'a>> {
    children_with_fields(node).map(|(_, child)| child)
}

pub(crate) fn each_child(node: Node, f: impl FnMut(Node)) {
    children(node).for_each(f);
}

/// First *named* child carrying `field`; `child_by_field_name` also matches the
/// anonymous tokens of a `field(seq('=', expr))` pair.
pub(crate) fn field_named<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    children_with_fields(node)
        .find(|(f, c)| c.is_named() && *f == Some(field))
        .map(|(_, c)| c)
}

pub(crate) fn find_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    children(node).find(|c| c.kind() == kind)
}

pub(crate) fn enclosing<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == kind {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

pub(crate) fn span_of(node: Node) -> Span {
    Span::new(node.start_byte(), node.end_byte())
}

pub(crate) fn text_of<'s>(node: Node, src: &'s str) -> &'s str {
    src.get(node.start_byte()..node.end_byte()).unwrap_or("")
}

fn build_class(node: Node, src: &str) -> Option<ClassItem> {
    let name_node = node.child_by_field_name("typename")?;
    let modded = each_kind(node, "class_modifier").any(|m| text_of(m, src) == "modded");
    let base = node
        .child_by_field_name("superclass")
        .and_then(|s| superclass_ref(s, src));
    let type_params = node
        .child_by_field_name("type_parameters")
        .map(|tp| {
            each_kind(tp, "type_parameter")
                .filter_map(|p| {
                    p.child_by_field_name("name")
                        .map(|n| text_of(n, src).to_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ClassItem {
        name: text_of(name_node, src).to_owned(),
        name_span: span_of(name_node),
        span: span_of(node),
        modded,
        base,
        type_params,
        fields: Vec::new(),
        methods: Vec::new(),
        doc: doc_of(node, src),
        attributes: attributes_of(node, src),
        decl_start: decl_start(node),
    })
}

fn build_enum(node: Node, src: &str) -> Option<EnumItem> {
    let name_node = node.child_by_field_name("typename")?;
    let members = node
        .child_by_field_name("body")
        .map(|body| {
            each_kind(body, "enum_member")
                .filter_map(|m| {
                    let n = m.child_by_field_name("name")?;
                    Some(EnumMember {
                        name: text_of(n, src).to_owned(),
                        name_span: span_of(n),
                        value: m
                            .child_by_field_name("value")
                            .map(|v| text_squeezed(v, src)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(EnumItem {
        name: text_of(name_node, src).to_owned(),
        name_span: span_of(name_node),
        span: span_of(node),
        base: node
            .child_by_field_name("superenum")
            .map(|b| text_of(b, src).to_owned()),
        members,
        doc: doc_of(node, src),
    })
}

fn build_typedef(node: Node, src: &str) -> Option<TypedefItem> {
    let alias = node.child_by_field_name("alias")?;
    Some(TypedefItem {
        name: text_of(alias, src).to_owned(),
        name_span: span_of(alias),
        target: node
            .child_by_field_name("type")
            .map(|t| type_ref(t, src))
            .unwrap_or_default(),
        doc: doc_of(node, src),
    })
}

fn build_method(node: Node, src: &str) -> Option<MethodItem> {
    let name_node = node.child_by_field_name("name")?;
    let params = find_kind(node, "formal_parameters")
        .map(|ps| {
            each_kind(ps, "formal_parameter")
                .filter_map(|p| build_param(p, src))
                .collect()
        })
        .unwrap_or_default();
    Some(MethodItem {
        name: text_of(name_node, src).to_owned(),
        name_span: span_of(name_node),
        span: span_of(node),
        ret: node
            .child_by_field_name("return_type")
            .map(|t| type_ref(t, src))
            .unwrap_or_default(),
        params,
        modifiers: each_kind(node, "method_modifier")
            .map(|m| text_of(m, src).to_owned())
            .collect(),
        doc: doc_of(node, src),
        body: node.child_by_field_name("body").map(span_of),
        attributes: attributes_of(node, src),
        decl_start: decl_start(node),
        destructor: find_kind(node, "~").is_some(),
    })
}

fn build_param(node: Node, src: &str) -> Option<ParamItem> {
    let name_node = node.child_by_field_name("name")?;
    Some(ParamItem {
        name: text_of(name_node, src).to_owned(),
        name_span: span_of(name_node),
        ty: node
            .child_by_field_name("type")
            .map(|t| type_ref(t, src))
            .unwrap_or_default(),
        modifiers: each_kind(node, "formal_parameter_modifier")
            .map(|m| text_of(m, src).to_owned())
            .collect(),
        default: squeezed_text(
            children_with_fields(node)
                .filter(|(field, child)| *field == Some("default") && child.kind() != "=")
                .map(|(_, child)| child),
            src,
        ),
        array: bracket_sizes(children(node), src),
    })
}

fn build_fields(node: Node, src: &str) -> Vec<FieldItem> {
    let modifiers: Vec<String> = each_child_kinds(node, &["field_modifier", "variable_modifier"])
        .map(|m| text_of(m, src).to_owned())
        .collect();
    let ty = node
        .child_by_field_name("type")
        .map(|t| type_ref(t, src))
        .unwrap_or_else(|| TypeRef::named("auto"));
    let doc = doc_of(node, src);
    let attributes = attributes_of(node, src);
    let kids: Vec<(Option<&'static str>, Node)> = children_with_fields(node).collect();
    kids.iter()
        .enumerate()
        .filter(|(_, (field, _))| *field == Some("name"))
        .map(|(at, (_, name))| {
            let rest = declarator_rest(&kids, at);
            FieldItem {
                name: text_of(*name, src).to_owned(),
                name_span: span_of(*name),
                ty: ty.clone(),
                modifiers: modifiers.clone(),
                doc: doc.clone(),
                attributes: attributes.clone(),
                value: squeezed_text(
                    rest.iter()
                        .filter(|(field, _)| *field == Some("init"))
                        .map(|(_, init)| *init),
                    src,
                ),
                array: source_sizes(*name, src),
            }
        })
        .collect()
}

/// The declaration's attribute list; `None` when a previous declaration lost its `;` right there.
fn own_list<'a>(node: Node<'a>, src: &str) -> Option<Node<'a>> {
    let list = find_kind(node, "attribute_list")?;
    let prev =
        std::iter::successors(node.prev_sibling(), |n| n.prev_sibling()).find(|n| !is_comment(*n));
    (!prev.is_some_and(|prev| glued(prev, list, src))).then_some(list)
}

/// Whether `list` follows `prev`, left without its `;`, with only spaces or tabs between.
fn glued(prev: Node, list: Node, src: &str) -> bool {
    let gap = src.get(prev.end_byte()..list.start_byte()).unwrap_or("\n");
    gap.bytes().all(|byte| matches!(byte, b' ' | b'\t')) && ends_without_semicolon(prev)
}

fn ends_without_semicolon(node: Node) -> bool {
    children(node)
        .filter(|child| !is_comment(*child))
        .last()
        .is_some_and(|last| last.is_missing() && last.kind() == ";")
}

/// The nodes of each comma-separated entry of an attribute list.
fn list_entries(list: Node) -> Vec<Vec<Node>> {
    let mut entries = Vec::new();
    let mut group = Vec::new();
    for child in children(list) {
        if matches!(child.kind(), "[" | "," | "]" | ";") {
            entries.push(std::mem::take(&mut group));
        } else {
            group.push(child);
        }
    }
    entries.retain(|entry| !entry.is_empty());
    entries
}

/// The children after the declarator named at `at`, up to the next declarator's name.
pub(crate) fn declarator_rest<'k, 'a>(
    kids: &'k [(Option<&'static str>, Node<'a>)],
    at: usize,
) -> &'k [(Option<&'static str>, Node<'a>)] {
    let rest = &kids[at + 1..];
    let end = rest
        .iter()
        .position(|(field, _)| *field == Some("name"))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The size expression of each `[...]` among `nodes`; empty text for `[]`.
pub(crate) fn bracket_sizes<'a>(nodes: impl Iterator<Item = Node<'a>>, src: &str) -> Vec<String> {
    let mut sizes = Vec::new();
    let mut inside: Option<Vec<Node>> = None;
    for node in nodes {
        match (node.kind(), inside.as_mut()) {
            ("[", _) => inside = Some(Vec::new()),
            ("]", Some(group)) => {
                sizes.push(squeezed_text(group.drain(..), src).unwrap_or_default());
                inside = None;
            }
            (_, Some(group)) => group.push(node),
            _ => {}
        }
    }
    sizes
}

/// The size expression of each `[...]` in the source right after `name`, in order; comments are
/// skipped and strings kept whole.
pub(crate) fn source_sizes(name: Node, src: &str) -> Vec<String> {
    let mut sizes = Vec::new();
    let mut at = name.end_byte();
    loop {
        at = skip_blank(src, at);
        if !src[at..].starts_with('[') {
            return sizes;
        }
        let Some((inner, end)) = bracket_inner(src, at) else {
            return sizes;
        };
        sizes.push(squeeze(&inner));
        at = end;
    }
}

/// The byte after any spaces, tabs and `/* */` comments from `at`; a list on a later line is the
/// next declaration's attributes, so a line break ends the sizes.
fn skip_blank(src: &str, mut at: usize) -> usize {
    loop {
        let rest = &src[at..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        at += rest.len() - trimmed.len();
        let Some(body) = trimmed.strip_prefix("/*") else {
            return at;
        };
        match body.find("*/") {
            Some(end) if !body[..end].contains('\n') => at += end + 4,
            _ => return at,
        }
    }
}

/// The length of the comment `text` starts with; an unclosed one runs to the end.
fn comment_len(text: &str) -> Option<usize> {
    if text.starts_with("//") {
        return Some(text.find('\n').unwrap_or(text.len()));
    }
    text.strip_prefix("/*")
        .map(|body| body.find("*/").map_or(text.len(), |end| end + 4))
}

/// The length of the string literal `text` starts with; an unclosed one runs to the end.
fn string_len(text: &str) -> usize {
    let mut escaped = false;
    for (at, c) in text.char_indices().skip(1) {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return at + 1,
            _ => escaped = false,
        }
    }
    text.len()
}

/// The text inside the brackets that open at `open`, comments dropped, and the byte after them.
fn bracket_inner(src: &str, open: usize) -> Option<(String, usize)> {
    let mut inner = String::new();
    let mut depth = 0usize;
    let mut at = open;
    while let Some(c) = src[at..].chars().next() {
        let rest = &src[at..];
        if let Some(len) = comment_len(rest) {
            inner.push(' ');
            at += len;
            continue;
        }
        if c == '"' {
            let len = string_len(rest);
            inner.push_str(&rest[..len]);
            at += len;
            continue;
        }
        match c {
            '[' => {
                depth += 1;
                if depth > 1 {
                    inner.push(c);
                }
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((inner, at + 1));
                }
                inner.push(c);
            }
            _ => inner.push(c),
        }
        at += c.len_utf8();
    }
    None
}

fn attributes_of(node: Node, src: &str) -> Vec<String> {
    let Some(list) = own_list(node, src) else {
        return Vec::new();
    };
    list_entries(list)
        .iter()
        .filter_map(|entry| squeezed_text(entry.iter().copied(), src))
        .collect()
}

/// The squeezed source from the first to the last of `nodes`, the comments among them left out;
/// `None` when there is no code among them.
pub(crate) fn squeezed_text<'a>(
    nodes: impl IntoIterator<Item = Node<'a>>,
    src: &str,
) -> Option<String> {
    let run: Vec<Node> = nodes.into_iter().filter(|node| !node.is_extra()).collect();
    let (start, end) = (run.first()?.start_byte(), run.last()?.end_byte());
    let mut comments = Vec::new();
    // From the parent, so comments between the run's nodes are found as well as those inside them.
    match run[0].parent() {
        Some(parent) => collect_comments(parent, start, end, &mut comments),
        None => {
            for node in &run {
                collect_comments(*node, start, end, &mut comments);
            }
        }
    }
    comments.sort_by_key(Node::start_byte);
    let mut out = String::new();
    let mut at = start;
    for comment in comments {
        out.push_str(src.get(at..comment.start_byte()).unwrap_or(""));
        at = comment.end_byte();
    }
    out.push_str(src.get(at..end).unwrap_or(""));
    Some(squeeze(&out))
}

fn is_comment(node: Node) -> bool {
    matches!(
        node.kind(),
        "comment_line" | "comment_block" | "doc_line" | "doc_block"
    )
}

fn collect_comments<'a>(node: Node<'a>, start: usize, end: usize, found: &mut Vec<Node<'a>>) {
    for child in children(node) {
        if child.end_byte() <= start || child.start_byte() >= end {
            continue;
        }
        if is_comment(child) {
            found.push(child);
        } else {
            collect_comments(child, start, end, found);
        }
    }
}

/// Where the declaration proper starts: after any attribute list.
fn decl_start(node: Node) -> usize {
    let after = find_kind(node, "attribute_list").and_then(|list| {
        std::iter::successors(list.next_sibling(), |n| n.next_sibling()).find(|n| !n.is_extra())
    });
    after.map_or(node.start_byte(), |next| next.start_byte())
}

/// The node's source with each comment inside left out and each whitespace run collapsed to one space.
pub(crate) fn text_squeezed(node: Node, src: &str) -> String {
    squeezed_text(std::iter::once(node), src).unwrap_or_default()
}

pub(crate) fn squeeze(text: &str) -> String {
    let mut out = String::new();
    let (mut quoted, mut escaped, mut gap) = (false, false, false);
    for c in text.trim().chars() {
        if !quoted && c.is_whitespace() {
            gap = true;
            continue;
        }
        if gap {
            out.push(' ');
            gap = false;
        }
        out.push(c);
        if quoted && escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        }
    }
    out
}

fn superclass_ref(node: Node, src: &str) -> Option<TypeRef> {
    let name = node.child_by_field_name("typename")?;
    let args = node
        .child_by_field_name("types")
        .map(|types| generic_args(types, src))
        .unwrap_or_default();
    Some(TypeRef {
        name: text_of(name, src).to_owned(),
        args,
        strong: false,
    })
}

pub(crate) fn type_ref(node: Node, src: &str) -> TypeRef {
    match node.kind() {
        "type_ref" => {
            let mut inner = node
                .named_child(0)
                .map(|t| type_ref(t, src))
                .unwrap_or_else(|| TypeRef::named(text_of(node, src)));
            inner.strong = true;
            inner
        }
        "type_identifier" => TypeRef {
            name: node
                .named_child(0)
                .map(|n| text_of(n, src).to_owned())
                .unwrap_or_default(),
            args: find_kind(node, "types")
                .map(|types| generic_args(types, src))
                .unwrap_or_default(),
            strong: false,
        },
        _ => TypeRef::named(text_of(node, src).trim()),
    }
}

fn generic_args(types: Node, src: &str) -> Vec<TypeRef> {
    children(types)
        .filter(|c| c.is_named())
        .map(|t| type_ref(t, src))
        .collect()
}

pub(crate) fn each_kind<'a>(node: Node<'a>, kind: &'static str) -> impl Iterator<Item = Node<'a>> {
    children(node).filter(move |c| c.kind() == kind)
}

pub(crate) fn each_child_kinds<'a>(
    node: Node<'a>,
    kinds: &'static [&'static str],
) -> impl Iterator<Item = Node<'a>> {
    children(node).filter(move |c| kinds.contains(&c.kind()))
}

fn doc_of(node: Node, src: &str) -> Option<String> {
    let mut docs = Vec::new();
    let mut anchor = node.start_byte();
    let mut sib = node.prev_sibling();
    while let Some(p) = sib {
        if !matches!(p.kind(), "doc_line" | "doc_block") {
            break;
        }
        let gap = src.get(p.end_byte()..anchor).unwrap_or("\n\n");
        if gap.chars().any(|c| !c.is_whitespace()) || gap.matches('\n').count() > 1 {
            break;
        }
        docs.push(p);
        anchor = p.start_byte();
        sib = p.prev_sibling();
    }
    if docs.is_empty() {
        return None;
    }
    docs.reverse();
    Some(
        docs.iter()
            .map(|d| doc_text(*d, src))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn doc_text(node: Node, src: &str) -> String {
    let raw = src.get(node.start_byte()..node.end_byte()).unwrap_or("");
    if node.kind() == "doc_line" {
        return raw
            .trim_start_matches("//!")
            .trim_start_matches("//?")
            .trim()
            .to_owned();
    }
    let body = raw
        .strip_prefix("/**")
        .or_else(|| raw.strip_prefix("/*!"))
        .unwrap_or(raw);
    let body = body.strip_suffix("*/").unwrap_or(body);
    let mut lines: Vec<String> = body
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim().to_owned())
        .collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}
