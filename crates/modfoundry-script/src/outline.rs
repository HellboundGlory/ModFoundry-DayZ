//! The declarations of one Enforce file in both preprocessor views, ready for JSON.

use crate::extract::{
    children, children_with_fields, each_kind, extract_view, field_named, find_kind, source_sizes,
    squeezed_text, text_of, text_squeezed, type_ref,
};
use crate::items::{
    ClassItem, EnumItem, FieldItem, FileItems, MethodItem, ParamItem, TypeRef, TypedefItem,
};
use crate::layer::Layer;
use modfoundry_core::text::{line_col, LineIndex};
use modfoundry_core::Span;
use modfoundry_enforce::{first_arms, guards, other_arms, parse_tree, syntax_diagnostics, Guard};
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use tree_sitter::Node;

/// The declarations of one Enforce file in both preprocessor views, ready for JSON.
pub fn outline(path: &str, source: &str) -> FileOutline {
    let blocks = Blocks {
        lines: LineIndex::new(source),
        guards: guards(source),
    };
    let first_src = first_arms(source);
    let other_src = other_arms(source);
    let first_sites = Sites::new(&first_src, &blocks);
    let other_sites = Sites::new(&other_src, &blocks);
    let first = extract_view(&first_src);
    let other = extract_view(&other_src);
    let bodies = merged_bodies(&first, &first_sites, &other, &other_sites, &blocks);
    let mut classes: Vec<ClassOutline> = first
        .classes
        .iter()
        .map(|c| class_outline(c, &blocks, &bodies))
        .collect();
    for class in &other.classes {
        match first
            .classes
            .iter()
            .position(|c| c.name_span.start == class.name_span.start)
        {
            Some(i) => {
                let fields = class
                    .fields
                    .iter()
                    .filter(|f| blocks.outside_first(f.name_span))
                    .map(|f| field_outline(f, &blocks));
                classes[i].fields.extend(fields);
                let methods = class
                    .methods
                    .iter()
                    .filter(|m| blocks.outside_first(m.name_span))
                    .map(|m| method_outline(m, &blocks, &bodies));
                classes[i].methods.extend(methods);
            }
            None if blocks.outside_first(class.name_span) => {
                let mut outline = class_outline(class, &blocks, &bodies);
                outline
                    .fields
                    .retain(|f| !blocks.in_view(f.line, View::First));
                outline
                    .methods
                    .retain(|m| !blocks.in_view(m.lines[0], View::First));
                classes.push(outline);
            }
            None => {}
        }
    }
    for class in &mut classes {
        class.fields.sort_by_key(|f| f.line);
        class.methods.sort_by_key(|m| m.lines[0]);
    }
    classes.sort_by_key(|c| c.lines[0]);
    FileOutline {
        path: path.to_owned(),
        layer: Layer::from_path(path).map(Layer::name),
        inactive: blocks.inactive(source),
        errors: file_errors(&first_sites, &other_sites, source, &blocks),
        classes,
        enums: merged(&first.enums, &other.enums, |e| e.name_span, &blocks)
            .into_iter()
            .map(|e| enum_outline(e, &other.enums, &blocks))
            .collect(),
        typedefs: merged(&first.typedefs, &other.typedefs, |t| t.name_span, &blocks)
            .into_iter()
            .map(|t| typedef_outline(t, &blocks))
            .collect(),
        functions: merged(&first.functions, &other.functions, |m| m.name_span, &blocks)
            .into_iter()
            .map(|m| method_outline(m, &blocks, &bodies))
            .collect(),
        globals: merged(&first.globals, &other.globals, |f| f.name_span, &blocks)
            .into_iter()
            .map(|f| field_outline(f, &blocks))
            .collect(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    First,
    Other,
}

struct Blocks {
    lines: LineIndex,
    guards: Vec<Guard>,
}

impl Blocks {
    fn line(&self, offset: usize) -> usize {
        self.lines.line_of(offset)
    }

    fn outside_first(&self, span: Span) -> bool {
        self.outside_first_line(self.line(span.start))
    }

    fn outside_first_line(&self, line: usize) -> bool {
        !self.in_view(line, View::First)
    }

    fn enclosing(&self, line: usize) -> impl Iterator<Item = (&Guard, Arm)> + '_ {
        self.guards.iter().filter_map(move |g| {
            let inside = g.open_line < line && line < g.end_line && g.else_line != Some(line);
            inside.then(|| {
                let arm = if g.else_line.is_some_and(|e| line > e) {
                    Arm::Else
                } else {
                    Arm::Then
                };
                (g, arm)
            })
        })
    }

    fn in_view(&self, line: usize, view: View) -> bool {
        self.enclosing(line).all(|(g, arm)| {
            let keeps_then = g.first_keeps_then() == (view == View::First);
            (arm == Arm::Then) == keeps_then
        })
    }

    /// Whether `line` sits in a `DOXYGEN` arm, which no build compiles.
    fn in_docs_only(&self, line: usize) -> bool {
        self.enclosing(line)
            .any(|(g, arm)| g.define == "DOXYGEN" && (arm == Arm::Then) != g.first_keeps_then())
    }

    fn inactive(&self, source: &str) -> Vec<[usize; 2]> {
        let mut runs = Vec::new();
        let mut run: Option<[usize; 2]> = None;
        for (index, text) in source.lines().enumerate() {
            let line = index + 1;
            if self.in_view(line, View::First) || self.in_view(line, View::Other) {
                runs.extend(run.take());
            } else if !text.trim().is_empty() && !self.is_directive(line) {
                run = Some(match run {
                    Some([first, _]) => [first, line],
                    None => [line, line],
                });
            }
        }
        runs.extend(run);
        runs
    }

    fn is_directive(&self, line: usize) -> bool {
        self.guards
            .iter()
            .any(|g| g.open_line == line || g.end_line == line || g.else_line == Some(line))
    }

    fn guard_outlines(&self, line: usize) -> Vec<GuardOutline> {
        self.enclosing(line)
            .map(|(g, arm)| GuardOutline {
                define: g.define.clone(),
                negated: g.negated,
                arm,
            })
            .collect()
    }
}

fn merged<'a, T>(
    first: &'a [T],
    other: &'a [T],
    name_span: impl Fn(&T) -> Span,
    blocks: &Blocks,
) -> Vec<&'a T> {
    let mut items: Vec<&T> = first
        .iter()
        .chain(
            other
                .iter()
                .filter(|item| blocks.outside_first(name_span(item))),
        )
        .collect();
    items.sort_by_key(|item| name_span(item).start);
    items
}

fn enum_outline(e: &EnumItem, other: &[EnumItem], blocks: &Blocks) -> EnumOutline {
    let line = blocks.line(e.name_span.start);
    let twin = other
        .iter()
        .find(|o| o.name_span.start == e.name_span.start);
    let extra = twin
        .into_iter()
        .flat_map(|o| &o.members)
        .filter(|m| blocks.outside_first(m.name_span));
    let mut members: Vec<_> = e.members.iter().chain(extra).collect();
    members.sort_by_key(|m| m.name_span.start);
    members.dedup_by_key(|m| m.name_span.start);
    EnumOutline {
        name: e.name.clone(),
        base: e.base.clone(),
        members: members.iter().map(|m| m.name.clone()).collect(),
        values: members.iter().map(|m| m.value.clone()).collect(),
        guards: blocks.guard_outlines(line),
        line,
    }
}

fn typedef_outline(t: &TypedefItem, blocks: &Blocks) -> TypedefOutline {
    let line = blocks.line(t.name_span.start);
    TypedefOutline {
        name: t.name.clone(),
        target: t.target.display(),
        guards: blocks.guard_outlines(line),
        line,
    }
}

fn class_outline(c: &ClassItem, blocks: &Blocks, bodies: &MethodBodies) -> ClassOutline {
    ClassOutline {
        name: c.name.clone(),
        modded: c.modded,
        base: c.base.as_ref().map(TypeRef::display),
        type_params: c.type_params.clone(),
        guards: blocks.guard_outlines(blocks.line(c.name_span.start)),
        lines: span_lines(c.span, blocks),
        attributes: c.attributes.clone(),
        decl_line: blocks.line(c.decl_start),
        fields: c.fields.iter().map(|f| field_outline(f, blocks)).collect(),
        methods: c
            .methods
            .iter()
            .map(|m| method_outline(m, blocks, bodies))
            .collect(),
    }
}

fn field_outline(f: &FieldItem, blocks: &Blocks) -> FieldOutline {
    let line = blocks.line(f.name_span.start);
    FieldOutline {
        name: f.name.clone(),
        ty: f.ty.display(),
        modifiers: f.modifiers.clone(),
        guards: blocks.guard_outlines(line),
        line,
        attributes: f.attributes.clone(),
        value: f.value.clone(),
        array: f.array.clone(),
    }
}

fn method_outline(m: &MethodItem, blocks: &Blocks, bodies: &MethodBodies) -> MethodOutline {
    let body = bodies.get(&m.name_span.start).cloned().unwrap_or_default();
    MethodOutline {
        name: m.name.clone(),
        ret: m.ret.display(),
        modifiers: m.modifiers.clone(),
        params: m.params.iter().map(param_outline).collect(),
        guards: blocks.guard_outlines(blocks.line(m.name_span.start)),
        lines: span_lines(m.span, blocks),
        calls: body.calls,
        strings: body.strings,
        returns: body.returns,
        locals: body.locals,
        types: sorted_names(body.types),
        attributes: m.attributes.clone(),
        decl_line: blocks.line(m.decl_start),
        destructor: m.destructor,
        return_notnull: m.modifiers.iter().any(|modifier| modifier == "notnull"),
    }
}

fn sorted_names(mentions: Vec<TypeSite>) -> Vec<String> {
    let mut names: Vec<String> = mentions.into_iter().map(|t| t.name).collect();
    names.sort();
    names.dedup();
    names
}

fn param_outline(p: &ParamItem) -> ParamOutline {
    ParamOutline {
        name: p.name.clone(),
        ty: p.ty.display(),
        modifiers: p.modifiers.clone(),
        default: p.default.clone(),
        array: p.array.clone(),
    }
}

fn span_lines(span: Span, blocks: &Blocks) -> [usize; 2] {
    [blocks.line(span.start), blocks.line(span.end)]
}

/// Each method's body, keyed by the method's name start, merged across both views.
type MethodBodies = HashMap<usize, Body>;

fn merged_bodies(
    first: &FileItems,
    first_sites: &Sites,
    other: &FileItems,
    other_sites: &Sites,
    blocks: &Blocks,
) -> MethodBodies {
    let mut bodies: MethodBodies = method_bodies(first, first_sites).collect();
    for (name, body) in method_bodies(other, other_sites) {
        match bodies.entry(name) {
            Entry::Occupied(mut twin) => twin.get_mut().add_other_view(body, blocks),
            Entry::Vacant(slot) => {
                slot.insert(body);
            }
        }
    }
    bodies
}

fn method_bodies<'a>(
    items: &'a FileItems,
    sites: &'a Sites,
) -> impl Iterator<Item = (usize, Body)> + 'a {
    items
        .functions
        .iter()
        .chain(items.classes.iter().flat_map(|c| &c.methods))
        .map(|m| (m.name_span.start, sites.body(m.body)))
}

/// One view's calls, strings, returns, locals, type names and syntax errors, keyed by start byte.
struct Sites {
    calls: Vec<(usize, CallOutline)>,
    strings: Vec<(usize, StringOutline)>,
    returns: Vec<(usize, ReturnOutline)>,
    locals: Vec<(usize, LocalOutline)>,
    types: Vec<(usize, TypeSite)>,
    errors: Vec<(Span, String)>,
}

impl Sites {
    fn new(src: &str, blocks: &Blocks) -> Self {
        let tree = parse_tree(src);
        let mut sites = Sites {
            calls: Vec::new(),
            strings: Vec::new(),
            returns: Vec::new(),
            locals: Vec::new(),
            types: Vec::new(),
            errors: syntax_diagnostics(&tree, src)
                .into_iter()
                .filter_map(|d| Some((d.span?, d.message)))
                .collect(),
        };
        collect(tree.root_node(), src, blocks, &mut sites);
        // Keyed by the name: a chain's outer call starts before the calls it wraps.
        sites.calls.sort_by_key(|(at, _)| *at);
        sites.returns.sort_by_key(|(at, _)| *at);
        sites.locals.sort_by_key(|(at, _)| *at);
        sites.types.sort_by_key(|(at, _)| *at);
        sites
    }

    fn body(&self, span: Option<Span>) -> Body {
        let Some(span) = span else {
            return Body::default();
        };
        Body {
            calls: within(&self.calls, span).cloned().collect(),
            strings: within(&self.strings, span).cloned().collect(),
            returns: within(&self.returns, span).cloned().collect(),
            locals: within(&self.locals, span).cloned().collect(),
            types: within(&self.types, span).cloned().collect(),
        }
    }
}

fn file_errors(first: &Sites, other: &Sites, source: &str, blocks: &Blocks) -> Vec<ErrorOutline> {
    let mut errors: Vec<ErrorOutline> = first
        .errors
        .iter()
        .map(|(span, message)| error_outline(message, span.start, source))
        .collect();
    let mut arm_ends: Option<Vec<(usize, usize)>> = None;
    for (span, message) in &other.errors {
        if let Some(at) = other_error_at(span, first, blocks, &mut arm_ends, source) {
            errors.push(error_outline(message, at, source));
        }
    }
    errors.sort_by_key(|e| (e.line, e.column));
    errors.dedup_by_key(|e| (e.line, e.column));
    errors
}

/// Where an error only the other view has goes, or `None` when it stays unlisted. A missing token
/// sits right after the previous token, which the other view often blanks, so it moves to the end
/// of the nearest code line the first view blanks instead of being lost. `arm_ends` caches those
/// line ends for the rare error that needs them.
fn other_error_at(
    span: &Span,
    first: &Sites,
    blocks: &Blocks,
    arm_ends: &mut Option<Vec<(usize, usize)>>,
    source: &str,
) -> Option<usize> {
    let line = blocks.line(span.start);
    if blocks.in_docs_only(line) {
        return None;
    }
    if blocks.outside_first(*span) {
        return Some(span.start);
    }
    let missing = span.start == span.end;
    let shared = first
        .errors
        .iter()
        .any(|(other, _)| other.start == span.start);
    if !missing || shared {
        return None;
    }
    let ends = arm_ends.get_or_insert_with(|| other_arm_ends(source, blocks));
    let at = arm_end_before(ends, line).unwrap_or(span.start);
    (!blocks.in_docs_only(blocks.line(at))).then_some(at)
}

fn error_outline(message: &str, at: usize, source: &str) -> ErrorOutline {
    let (line, column) = line_col(source, at);
    ErrorOutline {
        line,
        column,
        message: message.to_owned(),
    }
}

/// The byte just after the last non-whitespace character of each code line the first view blanks,
/// as (line, byte) in source order.
fn other_arm_ends(source: &str, blocks: &Blocks) -> Vec<(usize, usize)> {
    let mut ends = Vec::new();
    let mut offset = 0;
    for (index, text) in source.split_inclusive('\n').enumerate() {
        let line = index + 1;
        let body = text.trim_end_matches(['\r', '\n']);
        let code = body.trim_end();
        if !code.is_empty() && blocks.outside_first_line(line) && !blocks.is_directive(line) {
            ends.push((line, offset + code.len()));
        }
        offset += text.len();
    }
    ends
}

fn arm_end_before(ends: &[(usize, usize)], line: usize) -> Option<usize> {
    let at = ends.partition_point(|(number, _)| *number <= line);
    ends.get(at.checked_sub(1)?).map(|(_, byte)| *byte)
}

fn within<T>(sites: &[(usize, T)], span: Span) -> impl Iterator<Item = &T> {
    let from = sites.partition_point(|(at, _)| *at < span.start);
    sites[from..]
        .iter()
        .take_while(move |(at, _)| *at < span.end)
        .map(|(_, site)| site)
}

fn collect(node: Node, src: &str, blocks: &Blocks, sites: &mut Sites) {
    match node.kind() {
        "invokation" => sites.calls.extend(call_site(node, src, blocks)),
        // The keyword token has the same kind as the statement.
        "return" if node.is_named() => sites.returns.push(return_site(node, src, blocks)),
        "decl_variable" => declared_site(node, src, blocks, sites),
        "iterator" => iterator_site(node, src, blocks, sites),
        "cast" | "new" => {
            if let Some(ty) = field_named(node, "type") {
                type_sites(ty, src, blocks, sites);
            }
        }
        "literal_string" => {
            let raw = text_of(node, src);
            let text = raw
                .strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'))
                .unwrap_or(raw);
            sites.strings.push((
                node.start_byte(),
                StringOutline {
                    text: text.to_owned(),
                    line: blocks.line(node.start_byte()),
                },
            ));
        }
        _ => {}
    }
    for child in children(node) {
        collect(child, src, blocks, sites);
    }
}

fn call_site(node: Node, src: &str, blocks: &Blocks) -> Option<(usize, CallOutline)> {
    let invoked = field_named(node, "invoked")?;
    let arg_texts: Vec<String> = find_kind(node, "actual_parameters")
        .map(|ps| {
            each_kind(ps, "actual_parameter")
                .map(|p| text_squeezed(p, src))
                .collect()
        })
        .unwrap_or_default();
    let call = CallOutline {
        receiver: field_named(node, "instance").map(|i| text_squeezed(i, src)),
        name: text_of(invoked, src).to_owned(),
        args: arg_texts.len(),
        arg_texts,
        line: blocks.line(invoked.start_byte()),
    };
    Some((invoked.start_byte(), call))
}

fn return_site(node: Node, src: &str, blocks: &Blocks) -> (usize, ReturnOutline) {
    // Spans the parentheses too: the grammar hides them in a parenthesised return.
    let parts = children(node)
        .skip(1)
        .filter(|c| !c.is_extra() && c.kind() != ";");
    let text = squeezed_text(parts, src);
    let line = blocks.line(node.start_byte());
    (node.start_byte(), ReturnOutline { text, line })
}

/// One local per declared name; `auto` has no type node.
fn declared_site(node: Node, src: &str, blocks: &Blocks, sites: &mut Sites) {
    let ty = field_named(node, "type");
    for (field, name) in children_with_fields(node) {
        if field == Some("name") {
            push_local(node, name, ty, source_sizes(name, src), src, blocks, sites);
        }
    }
    if let Some(ty) = ty {
        type_sites(ty, src, blocks, sites);
    }
}

/// A foreach variable: an optional type node, then its name.
fn iterator_site(node: Node, src: &str, blocks: &Blocks, sites: &mut Sites) {
    let named: Vec<Node> = children(node)
        .filter(|c| c.is_named() && !c.is_extra())
        .collect();
    let (ty, name) = match named[..] {
        [ty, name] => (Some(ty), name),
        [name] => (None, name),
        _ => return,
    };
    push_local(node, name, ty, Vec::new(), src, blocks, sites);
    if let Some(ty) = ty {
        type_sites(ty, src, blocks, sites);
    }
}

fn push_local(
    decl: Node,
    name: Node,
    ty: Option<Node>,
    array: Vec<String>,
    src: &str,
    blocks: &Blocks,
    sites: &mut Sites,
) {
    let local = LocalOutline {
        name: text_of(name, src).to_owned(),
        ty: ty.map_or_else(|| "auto".to_owned(), |t| type_ref(t, src).display()),
        array,
        line: blocks.line(name.start_byte()),
    };
    sites.locals.push((decl.start_byte(), local));
}

/// Every type name in `node`, generic arguments included; primitives have no `type_identifier`.
fn type_sites(node: Node, src: &str, blocks: &Blocks, sites: &mut Sites) {
    if node.kind() == "type_identifier" {
        if let Some(name) = node.named_child(0) {
            let site = TypeSite {
                name: text_of(name, src).to_owned(),
                line: blocks.line(node.start_byte()),
            };
            sites.types.push((node.start_byte(), site));
        }
    }
    for child in children(node).filter(Node::is_named) {
        type_sites(child, src, blocks, sites);
    }
}

/// A method body's calls, strings, returns, locals and type names, merged across both views.
#[derive(Default, Clone)]
struct Body {
    calls: Vec<CallOutline>,
    strings: Vec<StringOutline>,
    returns: Vec<ReturnOutline>,
    locals: Vec<LocalOutline>,
    types: Vec<TypeSite>,
}

impl Body {
    fn add_other_view(&mut self, other: Body, blocks: &Blocks) {
        extend_outside_first(&mut self.calls, other.calls, |c| c.line, blocks);
        extend_outside_first(&mut self.strings, other.strings, |s| s.line, blocks);
        extend_outside_first(&mut self.returns, other.returns, |r| r.line, blocks);
        extend_outside_first(&mut self.locals, other.locals, |l| l.line, blocks);
        extend_outside_first(&mut self.types, other.types, |t| t.line, blocks);
    }
}

fn extend_outside_first<T>(
    into: &mut Vec<T>,
    from: Vec<T>,
    line: impl Fn(&T) -> usize,
    blocks: &Blocks,
) {
    into.extend(
        from.into_iter()
            .filter(|item| !blocks.in_view(line(item), View::First)),
    );
    into.sort_by_key(|item| line(item));
}

#[derive(Clone)]
struct TypeSite {
    name: String,
    line: usize,
}

/// One Enforce file's declarations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileOutline {
    pub path: String,
    pub layer: Option<&'static str>,
    pub inactive: Vec<[usize; 2]>,
    pub errors: Vec<ErrorOutline>,
    pub classes: Vec<ClassOutline>,
    pub enums: Vec<EnumOutline>,
    pub typedefs: Vec<TypedefOutline>,
    pub functions: Vec<MethodOutline>,
    pub globals: Vec<FieldOutline>,
}

/// A syntax error in one file, at the start of the faulty text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorOutline {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// A class declaration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassOutline {
    pub name: String,
    pub modded: bool,
    pub base: Option<String>,
    pub type_params: Vec<String>,
    pub guards: Vec<GuardOutline>,
    pub lines: [usize; 2],
    pub attributes: Vec<String>,
    pub decl_line: usize,
    pub fields: Vec<FieldOutline>,
    pub methods: Vec<MethodOutline>,
}

/// A class field or a global.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldOutline {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub modifiers: Vec<String>,
    pub guards: Vec<GuardOutline>,
    pub line: usize,
    pub attributes: Vec<String>,
    pub value: Option<String>,
    pub array: Vec<String>,
}

/// A method or a free function.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MethodOutline {
    pub name: String,
    #[serde(rename = "return")]
    pub ret: String,
    pub modifiers: Vec<String>,
    pub params: Vec<ParamOutline>,
    pub guards: Vec<GuardOutline>,
    pub lines: [usize; 2],
    pub calls: Vec<CallOutline>,
    pub strings: Vec<StringOutline>,
    pub returns: Vec<ReturnOutline>,
    pub locals: Vec<LocalOutline>,
    pub types: Vec<String>,
    pub attributes: Vec<String>,
    pub decl_line: usize,
    pub destructor: bool,
    pub return_notnull: bool,
}

/// A formal parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamOutline {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub modifiers: Vec<String>,
    pub default: Option<String>,
    pub array: Vec<String>,
}

/// A call inside a method body, nested calls included.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallOutline {
    pub receiver: Option<String>,
    pub name: String,
    pub args: usize,
    pub arg_texts: Vec<String>,
    pub line: usize,
}

/// A string literal inside a method body, escapes as written.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StringOutline {
    pub text: String,
    pub line: usize,
}

/// A `return` inside a method body; `text` is null for a bare `return;`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReturnOutline {
    pub text: Option<String>,
    pub line: usize,
}

/// A local declared in a method body, one per declared name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalOutline {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub array: Vec<String>,
    pub line: usize,
}

/// An enum declaration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnumOutline {
    pub name: String,
    pub base: Option<String>,
    pub members: Vec<String>,
    pub values: Vec<Option<String>>,
    pub guards: Vec<GuardOutline>,
    pub line: usize,
}

/// A typedef declaration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypedefOutline {
    pub name: String,
    pub target: String,
    pub guards: Vec<GuardOutline>,
    pub line: usize,
}

/// One preprocessor block enclosing a declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuardOutline {
    pub define: String,
    pub negated: bool,
    pub arm: Arm,
}

/// Which arm of a preprocessor block a declaration sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    Then,
    Else,
}
