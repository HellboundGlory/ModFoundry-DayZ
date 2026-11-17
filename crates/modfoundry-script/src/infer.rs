//! Receiver type inference for a `.` at some byte offset: the textual
//! scan-back a broken parse tree around the caret forces, then declared-type/return-type/generic resolution over the index.

use crate::extract::{each_kind, enclosing, text_of};
use crate::index::{Index, MemberKind, MAX_DEPTH};
use crate::items::TypeRef;
use crate::resolve::{context_at, resolve_local};
use modfoundry_enforce::parse_tree;
use tree_sitter::{Node, Tree};

/// The inferred type of a `.` receiver, and whether it names a class/enum
/// itself (a static access) rather than an instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Receiver {
    pub ty: TypeRef,
    pub is_static: bool,
}

enum Suffix {
    Call,
    Index,
}

struct Segment {
    name: String,
    suffixes: Vec<Suffix>,
}

/// Infer the type of the receiver expression ending right before `dot`
/// (the byte offset of the `.`). `None` covers unresolvable receivers:
/// `auto`-typed variables, casts, grouped expressions, and anything the
/// index has no declared type for.
pub fn infer_receiver(index: &Index, source: &str, dot: usize) -> Option<Receiver> {
    infer_receiver_in(index, &parse_tree(source), source, dot)
}

pub(crate) fn infer_receiver_in(
    index: &Index,
    tree: &Tree,
    source: &str,
    dot: usize,
) -> Option<Receiver> {
    let segments = scan_receiver(source, dot)?;
    if segments.is_empty() || segments.len() > MAX_DEPTH {
        return None;
    }

    let (method, class) = context_at(tree, source, dot);
    let modded = in_modded_class(tree, source, dot);

    let mut current: Option<Receiver> = None;
    for (i, seg) in segments.iter().enumerate() {
        current = Some(if i == 0 {
            resolve_leftmost(index, class.as_deref(), method, modded, source, seg, dot)?
        } else {
            resolve_chained(index, current.take()?, seg)?
        });
    }
    current
}

pub(crate) fn in_modded_class(tree: &Tree, source: &str, offset: usize) -> bool {
    let pos = offset.min(source.len());
    tree.root_node()
        .descendant_for_byte_range(pos, pos)
        .and_then(|node| enclosing(node, "decl_class"))
        .is_some_and(|class| {
            each_kind(class, "class_modifier").any(|m| text_of(m, source) == "modded")
        })
}

/// Source bytes up to a cursor with comment and string text blanked, and
/// whether the cursor sits inside one still open there.
pub(crate) struct Masked {
    pub bytes: Vec<u8>,
    pub inside: bool,
}

/// `source[..end]` with comment and string content replaced by spaces, byte
/// offsets unchanged; `inside` is the open state at `end`.
pub(crate) fn mask_literals(source: &str, end: usize) -> Masked {
    let mut bytes = source.as_bytes()[..end].to_vec();
    let len = bytes.len();
    let mut inside = false;
    let mut i = 0;
    while i < len {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let stop = bytes[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(len, |n| i + n);
            bytes[i..stop].fill(b' ');
            inside = stop == len;
            i = stop;
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let close =
                (i + 2..len.saturating_sub(1)).find(|&j| bytes[j] == b'*' && bytes[j + 1] == b'/');
            let stop = close.map_or(len, |j| j + 2);
            bytes[i..stop].fill(b' ');
            inside = close.is_none();
            i = stop;
        } else if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < len && bytes[j] != b'"' && bytes[j] != b'\n' {
                // A backslash before a newline continues the string, as in the grammar.
                if bytes[j] == b'\\' && j + 1 < len {
                    j += 1;
                }
                j += 1;
            }
            bytes[i + 1..j].fill(b' ');
            inside = j == len;
            let closed = j < len && bytes[j] == b'"';
            i = if closed { j + 1 } else { j };
        } else {
            i += 1;
        }
    }
    Masked { bytes, inside }
}

fn scan_receiver(source: &str, dot: usize) -> Option<Vec<Segment>> {
    if dot > source.len() {
        return None;
    }
    let Masked { bytes, inside } = mask_literals(source, dot);
    if inside {
        return None;
    }
    let bytes = bytes.as_slice();
    let mut pos = dot;
    let mut segments = Vec::new();
    loop {
        let mut p = skip_ws_back(bytes, pos);
        if p == 0 {
            return None;
        }

        let mut suffixes = Vec::new();
        loop {
            let q = skip_ws_back(bytes, p);
            if q == 0 {
                break;
            }
            let c = bytes[q - 1];
            let (open, close, kind) = match c {
                b')' => (b'(', b')', Suffix::Call),
                b']' => (b'[', b']', Suffix::Index),
                _ => break,
            };
            let mut depth = 1i32;
            let mut r = q - 1;
            let mut closed = false;
            while r > 0 {
                r -= 1;
                let ch = bytes[r];
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    depth -= 1;
                    if depth == 0 {
                        closed = true;
                        break;
                    }
                }
            }
            if !closed {
                return None;
            }
            suffixes.push(kind);
            p = r;
        }
        suffixes.reverse();

        let q = skip_ws_back(bytes, p);
        let end = q;
        let mut start = q;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let name = source[start..end].to_owned();
        segments.push(Segment { name, suffixes });

        let r = skip_ws_back(bytes, start);
        if r > 0 && bytes[r - 1] == b'.' {
            pos = r - 1;
        } else {
            break;
        }
    }
    segments.reverse();
    Some(segments)
}

pub(crate) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub(crate) fn skip_ws_back(bytes: &[u8], mut p: usize) -> usize {
    while p > 0 && bytes[p - 1].is_ascii_whitespace() {
        p -= 1;
    }
    p
}

fn make_receiver(index: &Index, ty: TypeRef, is_static: bool) -> Option<Receiver> {
    if ty.name == "auto" {
        return None;
    }
    Some(Receiver {
        ty: index.expand_typedef(&ty).clone(),
        is_static,
    })
}

fn apply_suffixes(index: &Index, receiver: Receiver, suffixes: &[Suffix]) -> Option<Receiver> {
    let mut r = receiver;
    for s in suffixes {
        match s {
            Suffix::Index => {
                let item = r.ty.args.last()?.clone();
                r = make_receiver(index, item, false)?;
            }
            Suffix::Call => return None,
        }
    }
    Some(r)
}

fn resolve_leftmost(
    index: &Index,
    class: Option<&str>,
    method: Option<Node>,
    modded: bool,
    source: &str,
    seg: &Segment,
    offset: usize,
) -> Option<Receiver> {
    if matches!(seg.suffixes.first(), Some(Suffix::Call)) {
        let ret = unqualified_call_return_type(index, class, &seg.name)?;
        let receiver = make_receiver(index, ret, false)?;
        return apply_suffixes(index, receiver, &seg.suffixes[1..]);
    }
    if seg.name == "this" || (seg.name == "super" && modded) {
        let class = class?;
        let receiver = make_receiver(index, TypeRef::named(class), false)?;
        return apply_suffixes(index, receiver, &seg.suffixes);
    }
    if seg.name == "super" {
        let class = class?;
        let receiver = make_receiver(index, base_ty(index, class)?, false)?;
        return apply_suffixes(index, receiver, &seg.suffixes);
    }
    if let Some(method) = method {
        if let Some((_, ty, _)) = resolve_local(method, source, &seg.name, offset) {
            let receiver = make_receiver(index, ty, false)?;
            return apply_suffixes(index, receiver, &seg.suffixes);
        }
    }
    if let Some(class_name) = class {
        if let Some(ty) = member_field(index, class_name, &seg.name).map(|(_, ty)| ty) {
            let receiver = make_receiver(index, ty, false)?;
            return apply_suffixes(index, receiver, &seg.suffixes);
        }
    }
    if let Some((_, f)) = index.global(&seg.name) {
        let receiver = make_receiver(index, f.ty.clone(), false)?;
        return apply_suffixes(index, receiver, &seg.suffixes);
    }
    if index.has_class(&seg.name) || index.enum_def(&seg.name).is_some() {
        let receiver = Receiver {
            ty: TypeRef::named(&seg.name),
            is_static: true,
        };
        return apply_suffixes(index, receiver, &seg.suffixes);
    }
    None
}

fn resolve_chained(index: &Index, prev: Receiver, seg: &Segment) -> Option<Receiver> {
    if matches!(seg.suffixes.first(), Some(Suffix::Call)) {
        let (owner, ret) = member_method(index, &prev.ty.name, &seg.name)?;
        let ret = substitute_generics(index, &prev.ty, &owner, ret);
        let receiver = make_receiver(index, ret, false)?;
        return apply_suffixes(index, receiver, &seg.suffixes[1..]);
    }
    let (owner, ty) = member_field(index, &prev.ty.name, &seg.name)?;
    let ty = substitute_generics(index, &prev.ty, &owner, ty);
    let receiver = make_receiver(index, ty, false)?;
    apply_suffixes(index, receiver, &seg.suffixes)
}

fn unqualified_call_return_type(index: &Index, class: Option<&str>, name: &str) -> Option<TypeRef> {
    if let Some(class) = class {
        if let Some((_, ret)) = member_method(index, class, name) {
            return Some(ret);
        }
    }
    index.function(name).map(|(_, m)| m.ret.clone())
}

fn member_method(index: &Index, class: &str, name: &str) -> Option<(String, TypeRef)> {
    index
        .find_members(class, name)
        .into_iter()
        .find_map(|d| match d.member {
            MemberKind::Method(m) => Some((d.owner.to_owned(), m.ret.clone())),
            MemberKind::Field(_) => None,
        })
}

fn member_field(index: &Index, class: &str, name: &str) -> Option<(String, TypeRef)> {
    index
        .find_members(class, name)
        .into_iter()
        .find_map(|d| match d.member {
            MemberKind::Field(f) => Some((d.owner.to_owned(), f.ty.clone())),
            MemberKind::Method(_) => None,
        })
}

fn substitute_generics(
    index: &Index,
    receiver_ty: &TypeRef,
    owner: &str,
    member_ty: TypeRef,
) -> TypeRef {
    let params = owner_type_params(index, owner);
    if params.is_empty() {
        return member_ty;
    }
    let args = type_at_owner(index, receiver_ty, owner)
        .map_or_else(|| receiver_ty.args.clone(), |ty| ty.args);
    substitute(&params, &args, member_ty)
}

fn type_at_owner(index: &Index, receiver_ty: &TypeRef, owner: &str) -> Option<TypeRef> {
    let mut cur = receiver_ty.clone();
    for _ in 0..MAX_DEPTH {
        if cur.name == owner {
            return Some(cur);
        }
        let base = base_ty(index, &cur.name)?;
        let params = owner_type_params(index, &cur.name);
        cur = substitute(&params, &cur.args, base);
    }
    None
}

fn base_ty(index: &Index, class: &str) -> Option<TypeRef> {
    index.base_target(class).cloned()
}

fn owner_type_params(index: &Index, owner: &str) -> Vec<String> {
    index
        .class_chain(owner)
        .into_iter()
        .find_map(|l| {
            if l.class.type_params.is_empty() {
                None
            } else {
                Some(l.class.type_params.clone())
            }
        })
        .unwrap_or_default()
}

fn substitute(params: &[String], args: &[TypeRef], ty: TypeRef) -> TypeRef {
    if let Some(idx) = params.iter().position(|p| p == &ty.name) {
        if let Some(actual) = args.get(idx) {
            return actual.clone();
        }
    }
    TypeRef {
        name: ty.name,
        args: ty
            .args
            .into_iter()
            .map(|a| substitute(params, args, a))
            .collect(),
        strong: ty.strong,
    }
}
