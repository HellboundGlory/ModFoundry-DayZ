//! Symbol resolution at a source offset: hover and go-to-definition, over
//! the live (possibly unsaved) text of one file.

use crate::extract::{
    children, children_with_fields, each_kind, enclosing, find_kind, span_of, text_of, type_ref,
};
use crate::index::{class_members, FileKey, Index, Location, MemberDef, MemberKind, Origin};
use crate::infer::{in_modded_class, infer_receiver_in};
use crate::items::TypeRef;
use crate::layer::Layer;
use modfoundry_core::Span;
use modfoundry_enforce::parse_tree;
use tree_sitter::{Node, Tree};

/// What kind of declaration a resolved symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Enum,
    EnumMember,
    Typedef,
    Method,
    Field,
    Local,
    Param,
}

/// Everything shown for a resolved symbol (hover card).
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub name: String,
    pub container: Option<String>,
    pub signature: String,
    pub doc: Option<String>,
    pub origin: Option<Origin>,
    pub layer: Option<Layer>,
}

/// A resolved symbol and every location it can jump to.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub symbol: SymbolInfo,
    pub targets: Vec<Location>,
}

/// Resolve the identifier touching `offset` in the live text `source` of
/// `file`. `source` is parsed fresh here; the index's own copy of this
/// file's items is never trusted for locals.
pub fn resolve_at(
    index: &Index,
    file: &FileKey,
    source: &str,
    offset: usize,
) -> Option<Resolution> {
    let tree = parse_tree(source);
    let node = tree.root_node().descendant_for_byte_range(offset, offset)?;
    if !matches!(node.kind(), "identifier" | "this" | "super") {
        return None;
    }
    let parent = node.parent()?;
    let name = text_of(node, source);

    if is_type_position(node, parent) {
        return resolve_type_name(index, name);
    }

    let (method, class) = context_of(node, source);

    match (parent.kind(), field_of(parent, node)) {
        ("formal_parameter" | "decl_variable", Some("name")) | ("iterator", _) => {
            return decl_self_resolution(file, node, source);
        }
        ("invokation", Some("invoked")) => {
            let instance = parent.child_by_field_name("instance");
            return resolve_call_or_member(
                index,
                file,
                &tree,
                class.as_deref(),
                source,
                instance,
                name,
            );
        }
        ("member_access", Some("member")) => {
            let accessed = parent.child_by_field_name("accessed");
            return resolve_call_or_member(
                index,
                file,
                &tree,
                class.as_deref(),
                source,
                accessed,
                name,
            );
        }
        _ => {}
    }

    resolve_bare_identifier(
        index,
        file,
        class.as_deref(),
        method,
        source,
        name,
        node.start_byte(),
    )
}

/// Parse `source` fresh and find the enclosing method node and class name at
/// `offset` (shared with [`crate::infer::infer_receiver`] and
/// [`crate::complete::complete_at`]).
pub(crate) fn context_at<'a>(
    tree: &'a Tree,
    source: &str,
    offset: usize,
) -> (Option<Node<'a>>, Option<String>) {
    let pos = offset.min(source.len());
    let Some(node) = tree.root_node().descendant_for_byte_range(pos, pos) else {
        return (None, None);
    };
    context_of(node, source)
}

fn context_of<'a>(node: Node<'a>, source: &str) -> (Option<Node<'a>>, Option<String>) {
    let method = enclosing(node, "decl_method");
    let class = enclosing(node, "decl_class")
        .and_then(|c| c.child_by_field_name("typename"))
        .map(|n| text_of(n, source).to_owned());
    (method, class)
}

fn field_of(parent: Node, node: Node) -> Option<&'static str> {
    children_with_fields(parent)
        .find(|(_, ch)| ch.id() == node.id())
        .and_then(|(field, _)| field)
}

fn is_type_position(node: Node, parent: Node) -> bool {
    match parent.kind() {
        "type_identifier" => parent
            .named_child(0)
            .map(|c| c.id() == node.id())
            .unwrap_or(false),
        "superclass" | "decl_class" => field_of(parent, node) == Some("typename"),
        _ => false,
    }
}

fn location(key: &FileKey, span: Span) -> Location {
    Location {
        file: key.id.clone(),
        span,
        origin: key.origin,
        layer: key.layer,
    }
}

fn decl_self_resolution(file: &FileKey, node: Node, source: &str) -> Option<Resolution> {
    let name = text_of(node, source);
    let decl = node.parent()?;
    let is_param = decl.kind() == "formal_parameter";
    let ty = declared_type(decl, source);
    let kind = if is_param {
        SymbolKind::Param
    } else {
        SymbolKind::Local
    };
    Some(Resolution {
        symbol: SymbolInfo {
            kind,
            name: name.to_owned(),
            container: None,
            signature: format!("{} {}", ty.display(), name),
            doc: None,
            origin: Some(file.origin),
            layer: file.layer,
        },
        targets: vec![location(file, span_of(node))],
    })
}

pub(crate) fn declared_type(decl: Node, source: &str) -> TypeRef {
    if let Some(t) = decl.child_by_field_name("type") {
        return type_ref(t, source);
    }
    for kind in [
        "type_identifier",
        "type_primitive",
        "type_ref",
        "type_array",
    ] {
        if let Some(t) = find_kind(decl, kind) {
            return type_ref(t, source);
        }
    }
    TypeRef::named("auto")
}

fn resolve_call_or_member(
    index: &Index,
    file: &FileKey,
    tree: &Tree,
    enclosing_class: Option<&str>,
    source: &str,
    recv: Option<Node>,
    member: &str,
) -> Option<Resolution> {
    match recv {
        None => {
            if let Some(class) = enclosing_class {
                if let Some(res) = resolve_member_use(index, class, member) {
                    return Some(res);
                }
            }
            resolve_top_level(index, member)
        }
        Some(r) if r.kind() == "super" => {
            let modded = in_modded_class(tree, source, r.start_byte());
            resolve_super(index, enclosing_class?, &file.id, modded, member)
        }
        Some(r) => {
            let receiver = infer_receiver_in(index, tree, source, r.end_byte())?;
            resolve_member_on_receiver(index, &receiver, member)
        }
    }
}

fn resolve_member_on_receiver(
    index: &Index,
    receiver: &crate::infer::Receiver,
    member: &str,
) -> Option<Resolution> {
    if receiver.is_static {
        if let Some(res) = resolve_enum_member(index, &receiver.ty.name, member) {
            return Some(res);
        }
    }
    resolve_member_use(index, &receiver.ty.name, member)
}

fn resolve_member_use(index: &Index, class: &str, member: &str) -> Option<Resolution> {
    let defs = index.find_members(class, member);
    build_resolution(&defs)
}

fn resolve_super(
    index: &Index,
    class: &str,
    current_file_id: &str,
    modded: bool,
    member: &str,
) -> Option<Resolution> {
    let chain = index.class_chain(class);
    match chain.iter().position(|l| l.key.id == current_file_id) {
        // A modded link not indexed yet sits on top of the whole chain.
        None if modded => return resolve_member_use(index, class, member),
        Some(cur_idx) if chain[cur_idx].class.modded => {
            let defs: Vec<MemberDef> = chain[..cur_idx]
                .iter()
                .rev()
                .flat_map(|link| class_members(link.key, link.class))
                .filter(|(name, _)| *name == member)
                .map(|(_, def)| def)
                .collect();
            if !defs.is_empty() {
                return build_resolution(&defs);
            }
        }
        _ => {}
    }
    let base = index.base_class(class)?;
    resolve_member_use(index, base, member)
}

fn build_resolution(defs: &[MemberDef]) -> Option<Resolution> {
    let first = defs.first()?;
    let (kind, name, signature) = match first.member {
        MemberKind::Field(f) => (
            SymbolKind::Field,
            f.name.clone(),
            format!("{} {}", f.ty.display(), f.name),
        ),
        MemberKind::Method(m) => (SymbolKind::Method, m.name.clone(), m.signature()),
    };
    let doc = defs.iter().find_map(|d| match d.member {
        MemberKind::Field(f) => f.doc.clone(),
        MemberKind::Method(m) => m.doc.clone(),
    });
    let targets = defs
        .iter()
        .map(|d| {
            let span = match d.member {
                MemberKind::Field(f) => f.name_span,
                MemberKind::Method(m) => m.name_span,
            };
            location(d.key, span)
        })
        .collect();
    Some(Resolution {
        symbol: SymbolInfo {
            kind,
            name,
            container: Some(first.owner.to_owned()),
            signature,
            doc,
            origin: Some(first.key.origin),
            layer: first.key.layer,
        },
        targets,
    })
}

fn resolve_enum_member(index: &Index, enum_name: &str, member: &str) -> Option<Resolution> {
    let (key, e) = index.enum_def(enum_name)?;
    let m = e.members.iter().find(|m| m.name == member)?;
    Some(Resolution {
        symbol: SymbolInfo {
            kind: SymbolKind::EnumMember,
            name: member.to_owned(),
            container: Some(enum_name.to_owned()),
            signature: format!("{enum_name}.{member}"),
            doc: e.doc.clone(),
            origin: Some(key.origin),
            layer: key.layer,
        },
        targets: vec![location(key, m.name_span)],
    })
}

/// `class X extends B` with the base member lookup uses, or `class X`.
pub(crate) fn class_signature(index: &Index, name: &str) -> String {
    match index.base_ref(name) {
        Some(b) => format!("class {name} extends {}", b.name),
        None => format!("class {name}"),
    }
}

fn resolve_type_name(index: &Index, name: &str) -> Option<Resolution> {
    let chain = index.class_chain(name);
    if !chain.is_empty() {
        let modded_count = chain.iter().filter(|l| l.class.modded).count();
        let mut signature = class_signature(index, name);
        if modded_count > 0 {
            let plural = if modded_count == 1 { "" } else { "s" };
            signature.push_str(&format!(" \u{b7} {modded_count} modded link{plural}"));
        }
        let doc = chain.iter().find_map(|l| l.class.doc.clone());
        let first = chain.first()?;
        let targets = chain
            .iter()
            .map(|l| location(l.key, l.class.name_span))
            .collect();
        return Some(Resolution {
            symbol: SymbolInfo {
                kind: SymbolKind::Class,
                name: name.to_owned(),
                container: None,
                signature,
                doc,
                origin: Some(first.key.origin),
                layer: first.key.layer,
            },
            targets,
        });
    }
    if let Some((key, e)) = index.enum_def(name) {
        return Some(Resolution {
            symbol: SymbolInfo {
                kind: SymbolKind::Enum,
                name: name.to_owned(),
                container: None,
                signature: format!("enum {name}"),
                doc: e.doc.clone(),
                origin: Some(key.origin),
                layer: key.layer,
            },
            targets: vec![location(key, e.name_span)],
        });
    }
    if let Some((key, t)) = index.typedef_def(name) {
        return Some(Resolution {
            symbol: SymbolInfo {
                kind: SymbolKind::Typedef,
                name: name.to_owned(),
                container: None,
                signature: format!("typedef {} {name}", t.target.display()),
                doc: t.doc.clone(),
                origin: Some(key.origin),
                layer: key.layer,
            },
            targets: vec![location(key, t.name_span)],
        });
    }
    None
}

fn resolve_top_level(index: &Index, name: &str) -> Option<Resolution> {
    if let Some((key, m)) = index.function(name) {
        return Some(Resolution {
            symbol: SymbolInfo {
                kind: SymbolKind::Method,
                name: name.to_owned(),
                container: None,
                signature: m.signature(),
                doc: m.doc.clone(),
                origin: Some(key.origin),
                layer: key.layer,
            },
            targets: vec![location(key, m.name_span)],
        });
    }
    if let Some((key, f)) = index.global(name) {
        return Some(Resolution {
            symbol: SymbolInfo {
                kind: SymbolKind::Field,
                name: name.to_owned(),
                container: None,
                signature: format!("{} {name}", f.ty.display()),
                doc: f.doc.clone(),
                origin: Some(key.origin),
                layer: key.layer,
            },
            targets: vec![location(key, f.name_span)],
        });
    }
    None
}

fn resolve_bare_identifier(
    index: &Index,
    file: &FileKey,
    enclosing_class: Option<&str>,
    method: Option<Node>,
    source: &str,
    name: &str,
    offset: usize,
) -> Option<Resolution> {
    if let Some(m) = method {
        if let Some((span, ty, is_param)) = resolve_local(m, source, name, offset) {
            let kind = if is_param {
                SymbolKind::Param
            } else {
                SymbolKind::Local
            };
            return Some(Resolution {
                symbol: SymbolInfo {
                    kind,
                    name: name.to_owned(),
                    container: None,
                    signature: format!("{} {name}", ty.display()),
                    doc: None,
                    origin: Some(file.origin),
                    layer: file.layer,
                },
                targets: vec![location(file, span)],
            });
        }
    }
    if let Some(class) = enclosing_class {
        if let Some(res) = resolve_member_use(index, class, name) {
            return Some(res);
        }
    }
    if let Some(resolution) = resolve_type_name(index, name) {
        return Some(resolution);
    }
    resolve_top_level(index, name)
}

type LocalCandidate = (usize, Span, TypeRef, bool);

/// Visit every local declaration of `method` in source order: parameters,
/// then the body in pre-order. `visit` receives `(name_node, decl_node,
/// is_param)`.
pub(crate) fn each_local<'t>(method: Node<'t>, mut visit: impl FnMut(Node<'t>, Node<'t>, bool)) {
    if let Some(params) = find_kind(method, "formal_parameters") {
        for p in each_kind(params, "formal_parameter") {
            if let Some(n) = p.child_by_field_name("name") {
                visit(n, p, true);
            }
        }
    }
    if let Some(body) = method.child_by_field_name("body") {
        walk_locals(body, &mut visit);
    }
}

fn walk_locals<'t>(node: Node<'t>, visit: &mut impl FnMut(Node<'t>, Node<'t>, bool)) {
    match node.kind() {
        "decl_variable" => {
            for (field, ch) in children_with_fields(node) {
                if field == Some("name") {
                    visit(ch, node, false);
                }
            }
        }
        "iterator" => {
            if let Some(n) = find_kind(node, "identifier") {
                visit(n, node, false);
            }
        }
        _ => {}
    }
    for ch in children(node) {
        walk_locals(ch, visit);
    }
}

pub(crate) fn resolve_local(
    method: Node,
    source: &str,
    name: &str,
    offset: usize,
) -> Option<(Span, TypeRef, bool)> {
    let mut best: Option<LocalCandidate> = None;
    each_local(method, |n, decl, is_param| {
        if text_of(n, source) == name {
            consider(
                &mut best,
                n.start_byte(),
                span_of(n),
                declared_type(decl, source),
                is_param,
                offset,
            );
        }
    });
    best.map(|(_, span, ty, is_param)| (span, ty, is_param))
}

fn consider(
    best: &mut Option<LocalCandidate>,
    start: usize,
    span: Span,
    ty: TypeRef,
    is_param: bool,
    offset: usize,
) {
    if start > offset {
        return;
    }
    if best.as_ref().map(|b| start >= b.0).unwrap_or(true) {
        *best = Some((start, span, ty, is_param));
    }
}
