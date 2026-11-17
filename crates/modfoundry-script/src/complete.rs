//! Completion at a byte offset: receiver-typed member lists after `.`, and
//! locals/members/globals/keywords otherwise.

use crate::extract::text_of;
use crate::index::{Index, MemberDef, MemberKind, Origin};
use crate::infer::{infer_receiver_in, is_ident_byte, mask_literals, skip_ws_back, Receiver};
use crate::items::TypeRef;
use crate::keywords::RESERVED;
use crate::layer::Layer;
use crate::resolve::{class_signature, context_at, declared_type, each_local};
use modfoundry_core::Span;
use modfoundry_enforce::parse_tree;
use std::collections::HashMap;
use tree_sitter::{Node, Tree};

const CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Local,
    Param,
    Field,
    Method,
    Class,
    Enum,
    EnumMember,
    Typedef,
    Function,
    Keyword,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: String,
    pub origin: Option<Origin>,
    pub layer: Option<Layer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub items: Vec<CompletionItem>,
    pub replace: Span,
    pub inferred: bool,
    /// Whether the ranked list was cut at the [`CAP`] candidate limit.
    pub truncated: bool,
}

/// Every candidate at `offset`, filtered and ranked by the identifier prefix
/// touching it.
pub fn complete_at(index: &Index, source: &str, offset: usize) -> Completion {
    let offset = offset.min(source.len());
    let empty = Completion {
        items: Vec::new(),
        replace: Span::new(offset, offset),
        inferred: false,
        truncated: false,
    };
    if !source.is_char_boundary(offset) {
        return empty;
    }
    let tree = parse_tree(source);
    if mask_literals(source, offset).inside {
        return empty;
    }

    let (start, dot) = find_prefix_and_dot(source, offset);
    let replace = Span::new(start, offset);
    let prefix = &source[start..offset];

    let (mut items, inferred) = match dot {
        Some(dot_pos) => match infer_receiver_in(index, &tree, source, dot_pos) {
            Some(receiver) => (member_items(index, &tree, source, offset, &receiver), true),
            None => (all_member_items(index, prefix), false),
        },
        None => (scope_items(index, &tree, source, offset, prefix), false),
    };
    rank(&mut items, prefix);
    let truncated = items.len() > CAP;
    items.truncate(CAP);
    Completion {
        items,
        replace,
        inferred,
        truncated,
    }
}

fn find_prefix_and_dot(source: &str, offset: usize) -> (usize, Option<usize>) {
    let bytes = source.as_bytes();
    let mut start = offset;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let p = skip_ws_back(bytes, start);
    let dot = if p > 0 && bytes[p - 1] == b'.' {
        Some(p - 1)
    } else {
        None
    };
    (start, dot)
}

fn member_items(
    index: &Index,
    tree: &Tree,
    source: &str,
    offset: usize,
    receiver: &Receiver,
) -> Vec<CompletionItem> {
    if receiver.is_static {
        if let Some((key, e)) = index.enum_def(&receiver.ty.name) {
            return e
                .members
                .iter()
                .map(|m| CompletionItem {
                    label: m.name.clone(),
                    kind: CompletionKind::EnumMember,
                    detail: format!("{}.{}", receiver.ty.name, m.name),
                    origin: Some(key.origin),
                    layer: key.layer,
                })
                .collect();
        }
    }

    let mut defs = index.members_of(&receiver.ty.name);
    if receiver.is_static {
        defs.retain(|d| has_modifier(d, "static"));
    }
    let ec = context_at(tree, source, offset).1;
    defs.retain(|d| !has_modifier(d, "private") || ec.as_deref() == Some(d.owner));
    defs.iter().map(member_def_item).collect()
}

fn all_member_items(index: &Index, prefix: &str) -> Vec<CompletionItem> {
    index
        .all_members()
        .iter()
        .filter(|d| {
            let name = match &d.member {
                MemberKind::Field(f) => &f.name,
                MemberKind::Method(m) => &m.name,
            };
            match_score(prefix, name).is_some()
        })
        .map(member_def_item)
        .collect()
}

fn scope_items(
    index: &Index,
    tree: &Tree,
    source: &str,
    offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let (method, class) = context_at(tree, source, offset);
    let locals = method
        .map(|n| locals_in_scope(n, source, offset))
        .unwrap_or_default();
    for (name, ty, is_param) in locals {
        items.push(CompletionItem {
            label: name,
            kind: if is_param {
                CompletionKind::Param
            } else {
                CompletionKind::Local
            },
            detail: ty.display(),
            origin: None,
            layer: None,
        });
    }

    if let Some(class_name) = &class {
        let ec = Some(class_name.as_str());
        let mut defs = index.members_of(class_name);
        defs.retain(|d| !has_modifier(d, "private") || ec == Some(d.owner));
        for d in &defs {
            items.push(member_def_item(d));
        }
    }

    for name in index.class_names() {
        if match_score(prefix, name).is_none() {
            continue;
        }
        items.push(class_item(index, name));
    }
    for name in index.enum_names() {
        if match_score(prefix, name).is_none() {
            continue;
        }
        let def = index.enum_def(name);
        items.push(CompletionItem {
            label: name.to_owned(),
            kind: CompletionKind::Enum,
            detail: format!("enum {name}"),
            origin: def.map(|(k, _)| k.origin),
            layer: def.and_then(|(k, _)| k.layer),
        });
    }
    for name in index.typedef_names() {
        if match_score(prefix, name).is_none() {
            continue;
        }
        let (origin, layer, detail) = match index.typedef_def(name) {
            Some((k, t)) => (
                Some(k.origin),
                k.layer,
                format!("typedef {} {name}", t.target.display()),
            ),
            None => (None, None, format!("typedef {name}")),
        };
        items.push(CompletionItem {
            label: name.to_owned(),
            kind: CompletionKind::Typedef,
            detail,
            origin,
            layer,
        });
    }
    for name in index.function_names() {
        if match_score(prefix, name).is_none() {
            continue;
        }
        let (origin, layer, detail) = match index.function(name) {
            Some((k, m)) => (Some(k.origin), k.layer, m.signature()),
            None => (None, None, String::new()),
        };
        items.push(CompletionItem {
            label: name.to_owned(),
            kind: CompletionKind::Function,
            detail,
            origin,
            layer,
        });
    }
    for kw in RESERVED.iter().chain(&["auto"]) {
        if match_score(prefix, kw).is_none() {
            continue;
        }
        items.push(CompletionItem {
            label: (*kw).to_owned(),
            kind: CompletionKind::Keyword,
            detail: String::new(),
            origin: None,
            layer: None,
        });
    }

    items
}

fn class_item(index: &Index, name: &str) -> CompletionItem {
    let chain = index.class_chain(name);
    let first = chain.first();
    CompletionItem {
        label: name.to_owned(),
        kind: CompletionKind::Class,
        detail: class_signature(index, name),
        origin: first.map(|l| l.key.origin),
        layer: first.and_then(|l| l.key.layer),
    }
}

fn has_modifier(d: &MemberDef, modifier: &str) -> bool {
    let modifiers: &[String] = match &d.member {
        MemberKind::Field(f) => &f.modifiers,
        MemberKind::Method(m) => &m.modifiers,
    };
    modifiers.iter().any(|m| m == modifier)
}

fn member_def_item(d: &MemberDef) -> CompletionItem {
    match &d.member {
        MemberKind::Field(f) => CompletionItem {
            label: f.name.clone(),
            kind: CompletionKind::Field,
            detail: format!("{} {}", f.ty.display(), f.name),
            origin: Some(d.key.origin),
            layer: d.key.layer,
        },
        MemberKind::Method(m) => CompletionItem {
            label: m.name.clone(),
            kind: CompletionKind::Method,
            detail: m.signature(),
            origin: Some(d.key.origin),
            layer: d.key.layer,
        },
    }
}

fn locals_in_scope(method: Node, source: &str, offset: usize) -> Vec<(String, TypeRef, bool)> {
    let mut map: HashMap<String, (usize, TypeRef, bool)> = HashMap::new();
    each_local(method, |n, decl, is_param| {
        update(
            &mut map,
            text_of(n, source).to_owned(),
            n.start_byte(),
            declared_type(decl, source),
            is_param,
            offset,
        );
    });
    map.into_iter()
        .map(|(name, (_, ty, is_param))| (name, ty, is_param))
        .collect()
}

fn update(
    map: &mut HashMap<String, (usize, TypeRef, bool)>,
    name: String,
    start: usize,
    ty: TypeRef,
    is_param: bool,
    offset: usize,
) {
    if start > offset {
        return;
    }
    match map.get(&name) {
        Some((s, _, _)) if *s >= start => {}
        _ => {
            map.insert(name, (start, ty, is_param));
        }
    }
}

const MEMBER_TIER: u8 = 1;

fn tier(kind: CompletionKind) -> u8 {
    match kind {
        CompletionKind::Local | CompletionKind::Param => 0,
        CompletionKind::Field | CompletionKind::Method | CompletionKind::EnumMember => MEMBER_TIER,
        CompletionKind::Class
        | CompletionKind::Enum
        | CompletionKind::Typedef
        | CompletionKind::Function => 2,
        CompletionKind::Keyword => 3,
    }
}

fn rank(items: &mut Vec<CompletionItem>, prefix: &str) {
    let mut scored: Vec<(u8, u8, CompletionItem)> = items
        .drain(..)
        .filter_map(|item| {
            match_score(prefix, &item.label).map(|score| (score, tier(item.kind), item))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| {
            // Members arrive in class-chain order, nearest class first; keep it.
            if a.1 == MEMBER_TIER {
                return std::cmp::Ordering::Equal;
            }
            caseless(&a.2.label)
                .cmp(caseless(&b.2.label))
                .then_with(|| a.2.label.cmp(&b.2.label))
        })
    });
    *items = scored.into_iter().map(|(_, _, i)| i).collect();
}

fn caseless(label: &str) -> impl Iterator<Item = u8> + '_ {
    label.bytes().map(|b| b.to_ascii_lowercase())
}

fn match_score(prefix: &str, label: &str) -> Option<u8> {
    if label.starts_with(prefix) {
        return Some(0);
    }
    let lower_label = label.to_lowercase();
    let lower_prefix = prefix.to_lowercase();
    if lower_label.starts_with(&lower_prefix) {
        return Some(1);
    }
    if !prefix.is_empty() && camel_hump_match(prefix, label) {
        return Some(2);
    }
    if lower_label.contains(&lower_prefix) {
        return Some(3);
    }
    None
}

fn camel_hump_match(prefix: &str, label: &str) -> bool {
    let humps: Vec<char> = label
        .char_indices()
        .filter(|(i, c)| *i == 0 || c.is_uppercase())
        .map(|(_, c)| c)
        .collect();
    let mut hi = 0;
    for pc in prefix.chars() {
        let pc = pc.to_ascii_uppercase();
        let mut found = false;
        while hi < humps.len() {
            let hc = humps[hi].to_ascii_uppercase();
            hi += 1;
            if hc == pc {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}
