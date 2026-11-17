//! Semantic lints over the live parse of one workspace file: module-layer
//! violations (MF-E3001), unknown type references (MF-W3002), variables named
//! like a type the module can see (MF-E3003) and bare calls to a method named
//! like a class (MF-E3004).

use crate::extract::{
    children, children_with_fields, each_child, each_kind, enclosing, find_kind, span_of, text_of,
};
use crate::index::{FileKey, Index, MemberKind, Origin};
use crate::keywords::RESERVED;
use crate::layer::Layer;
use modfoundry_core::{Diagnostic, Severity, Span};
use tree_sitter::{Node, Tree};

/// Semantic diagnostics for the parsed `source` (the live text of `file`): an
/// `MF-E3001` error when a referenced type exists only in a module the file
/// cannot see, an `MF-W3002` warning for a type name that resolves nowhere, an
/// `MF-E3003` error for a variable named like a visible type and an `MF-E3004`
/// error for a bare call to a method named like a visible class. Empty for any
/// file that is not a workspace file.
pub fn semantic_diagnostics(
    index: &Index,
    file: &FileKey,
    tree: &Tree,
    source: &str,
) -> Vec<Diagnostic> {
    if file.origin != Origin::Workspace {
        return Vec::new();
    }
    let mut refs = Vec::new();
    let mut params = Vec::new();
    let mut cond = 0usize;
    walk_refs(
        tree.root_node(),
        source,
        index,
        &mut params,
        &mut refs,
        false,
        &mut cond,
    );
    let mut out: Vec<Diagnostic> = refs
        .into_iter()
        .filter_map(|(name, span)| classify(index, file, &name, span))
        .collect();
    if let Some(layer) = file.layer {
        let mut cond = 0usize;
        walk_names(
            tree.root_node(),
            source,
            index,
            layer,
            false,
            &mut cond,
            &mut out,
        );
    }
    out
}

fn type_layers(index: &Index, name: &str) -> Vec<Option<Layer>> {
    let mut layers: Vec<Option<Layer>> = index
        .class_chain(name)
        .iter()
        .map(|link| link.key.layer)
        .collect();
    layers.extend(index.enum_def(name).map(|(key, _)| key.layer));
    layers.extend(index.typedef_def(name).map(|(key, _)| key.layer));
    layers
}

fn classify(index: &Index, file: &FileKey, name: &str, span: Span) -> Option<Diagnostic> {
    let layers = type_layers(index, name);
    let unresolved = layers.is_empty();

    if let (Some(layer), false) = (file.layer, unresolved) {
        if layers.iter().all(|l| matches!(l, Some(l) if *l > layer)) {
            let kind = if index.has_class(name) {
                "class"
            } else if index.enum_def(name).is_some() {
                "enum"
            } else {
                "typedef"
            };
            let defined = layers
                .into_iter()
                .flatten()
                .next()
                .expect("every candidate is above the file");
            return Some(Diagnostic::at(
                "MF-E3001",
                Severity::Error,
                span,
                format!(
                    "{kind} `{name}` is defined in {}; {} code cannot see it",
                    defined.name(),
                    layer.name()
                ),
            ));
        }
    }

    if unresolved && !index.type_exists(name) {
        return Some(Diagnostic::at(
            "MF-W3002",
            Severity::Warning,
            span,
            format!("unknown type `{name}`"),
        ));
    }
    None
}

fn is_type_visible(index: &Index, name: &str, layer: Layer) -> bool {
    type_layers(index, name)
        .iter()
        .any(|l| matches!(l, Some(l) if *l <= layer))
}

fn track_conditional(kind: &str, cond: &mut usize) -> bool {
    match kind {
        "preproc_ifdef" | "preproc_ifndef" => *cond += 1,
        "preproc_endif" => *cond = cond.saturating_sub(1),
        _ => return false,
    }
    true
}

fn walk_refs(
    node: Node,
    src: &str,
    index: &Index,
    params: &mut Vec<String>,
    out: &mut Vec<(String, Span)>,
    skip: bool,
    cond: &mut usize,
) {
    if track_conditional(node.kind(), cond) {
        return;
    }
    if node.kind() == "ERROR" {
        // Nothing in broken code is reported, but its directives still count.
        each_child(node, |c| walk_refs(c, src, index, params, out, true, cond));
        return;
    }
    let skip = skip || *cond > 0;
    match node.kind() {
        "decl_class" => {
            let mark = push_params(node, src, params);
            if !skip {
                if let Some(sup) = node.child_by_field_name("superclass") {
                    if !sup.has_error() {
                        superclass_refs(sup, src, params, out);
                    }
                }
            }
            each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond));
            params.truncate(mark);
        }
        "decl_method" => {
            let mark = push_params(node, src, params);
            let native = skip || is_native(node, src);
            if !native {
                if let Some(ret) = node.child_by_field_name("return_type") {
                    if !node.has_error() {
                        type_refs(ret, src, params, out);
                    }
                }
            }
            each_child(node, |c| {
                walk_refs(c, src, index, params, out, native, cond)
            });
            params.truncate(mark);
        }
        "decl_enum" => {
            if !skip {
                if let Some(sup) = node.child_by_field_name("superenum") {
                    push_ref(text_of(sup, src), span_of(sup), params, out);
                }
            }
            each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond));
        }
        "decl_field" | "decl_variable" | "formal_parameter" | "cast" | "new" => {
            if !skip && !node.has_error() {
                if let Some(ty) = node.child_by_field_name("type") {
                    type_refs(ty, src, params, out);
                }
            }
            each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond));
        }
        "invokation" => {
            if !skip && !node.has_error() {
                if let Some(recv) = node.child_by_field_name("instance") {
                    static_receiver(recv, src, index, params, out);
                }
            }
            each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond));
        }
        "member_access" => {
            if !skip && !node.has_error() {
                if let Some(recv) = node.child_by_field_name("accessed") {
                    static_receiver(recv, src, index, params, out);
                }
            }
            each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond));
        }
        _ => each_child(node, |c| walk_refs(c, src, index, params, out, skip, cond)),
    }
}

fn is_native(node: Node, src: &str) -> bool {
    each_kind(node, "method_modifier").any(|m| matches!(text_of(m, src), "proto" | "native"))
}

fn push_params(node: Node, src: &str, params: &mut Vec<String>) -> usize {
    let mark = params.len();
    if let Some(tp) = node.child_by_field_name("type_parameters") {
        for p in each_kind(tp, "type_parameter") {
            if let Some(name) = p.child_by_field_name("name") {
                params.push(text_of(name, src).to_owned());
            }
        }
    }
    mark
}

fn push_ref(name: &str, span: Span, params: &[String], out: &mut Vec<(String, Span)>) {
    if name.is_empty() || RESERVED.contains(&name) || params.iter().any(|p| p == name) {
        return;
    }
    out.push((name.to_owned(), span));
}

fn superclass_refs(sup: Node, src: &str, params: &[String], out: &mut Vec<(String, Span)>) {
    if let Some(name) = sup.child_by_field_name("typename") {
        push_ref(text_of(name, src), span_of(name), params, out);
    }
    if let Some(types) = sup.child_by_field_name("types") {
        types_refs(types, src, params, out);
    }
}

fn types_refs(types: Node, src: &str, params: &[String], out: &mut Vec<(String, Span)>) {
    for child in children(types).filter(|c| c.is_named()) {
        type_refs(child, src, params, out);
    }
}

fn type_refs(node: Node, src: &str, params: &[String], out: &mut Vec<(String, Span)>) {
    match node.kind() {
        "type_ref" | "type_array" => {
            if let Some(inner) = node.named_child(0) {
                type_refs(inner, src, params, out);
            }
        }
        "type_identifier" => {
            if let Some(id) = node.named_child(0) {
                push_ref(text_of(id, src), span_of(id), params, out);
            }
            for child in each_kind(node, "types") {
                types_refs(child, src, params, out);
            }
        }
        "type_primitive" => push_ref(text_of(node, src), span_of(node), params, out),
        _ => {}
    }
}

fn static_receiver(
    node: Node,
    src: &str,
    index: &Index,
    params: &[String],
    out: &mut Vec<(String, Span)>,
) {
    if !matches!(node.kind(), "identifier" | "type_identifier") {
        return;
    }
    let name_node = if node.kind() == "type_identifier" {
        match node.named_child(0) {
            Some(id) => id,
            None => return,
        }
    } else {
        node
    };
    let name = text_of(name_node, src);
    if !index.declares_type(name) {
        return;
    }
    push_ref(name, span_of(name_node), params, out);
}

fn walk_names(
    node: Node,
    src: &str,
    index: &Index,
    layer: Layer,
    skip: bool,
    cond: &mut usize,
    out: &mut Vec<Diagnostic>,
) {
    if track_conditional(node.kind(), cond) {
        return;
    }
    if node.kind() == "ERROR" {
        each_child(node, |c| walk_names(c, src, index, layer, true, cond, out));
        return;
    }
    let skip = skip || *cond > 0;
    if !skip && !node.has_error() {
        match node.kind() {
            "decl_field" | "decl_variable" if !reserved_type(node, src) => {
                for (_, name) in children_with_fields(node).filter(|(f, _)| *f == Some("name")) {
                    report_name(index, layer, name, src, out);
                }
            }
            "formal_parameter" => {
                if let Some(name) = node.child_by_field_name("name") {
                    report_name(index, layer, name, src, out);
                }
            }
            "iterator" => {
                if let Some(name) = find_kind(node, "identifier") {
                    report_name(index, layer, name, src, out);
                }
            }
            "invokation" => report_call(node, src, index, layer, out),
            _ => {}
        }
    }
    each_child(node, |c| walk_names(c, src, index, layer, skip, cond, out));
}

fn report_name(index: &Index, layer: Layer, name: Node, src: &str, out: &mut Vec<Diagnostic>) {
    let text = text_of(name, src);
    if is_type_visible(index, text, layer) {
        out.push(Diagnostic::at(
            "MF-E3003",
            Severity::Error,
            span_of(name),
            format!("variable name '{text}' already used as type name"),
        ));
    }
}

/// The grammar reads an unbraced `else` before a call as a declaration of type
/// `else`.
fn reserved_type(node: Node, src: &str) -> bool {
    node.child_by_field_name("type")
        .is_some_and(|ty| RESERVED.contains(&text_of(ty, src)))
}

/// A call with no receiver whose name is a visible type runs that type's
/// constructor, even where the enclosing class declares a method of that name.
fn report_call(node: Node, src: &str, index: &Index, layer: Layer, out: &mut Vec<Diagnostic>) {
    let Some(name_node) = node.child_by_field_name("invoked") else {
        return;
    };
    if node.child_by_field_name("instance").is_some() {
        return;
    }
    let Some(class) = enclosing(node, "decl_class") else {
        return;
    };
    let Some(owner) = class
        .child_by_field_name("typename")
        .map(|n| text_of(n, src))
    else {
        return;
    };
    let name = text_of(name_node, src);
    if name == owner
        || !is_type_visible(index, name, layer)
        || is_generic(index, name)
        || is_base_name(index, class, src, owner, name)
    {
        return;
    }
    if declares_method(index, class, src, owner, name) {
        out.push(Diagnostic::at(
            "MF-E3004",
            Severity::Error,
            span_of(name_node),
            format!(
                "call '{name}()' runs the class {name}'s constructor, not the method {name}(); rename the method"
            ),
        ));
    }
}

// A bare `set(..)` or `array(..)` without `<T>` is not known to run a constructor.
fn is_generic(index: &Index, name: &str) -> bool {
    index
        .class_chain(name)
        .iter()
        .any(|link| !link.class.type_params.is_empty())
}

fn live_base<'a>(class: Node, src: &'a str) -> Option<&'a str> {
    class
        .child_by_field_name("superclass")
        .and_then(|s| s.child_by_field_name("typename"))
        .map(|n| text_of(n, src))
}

fn is_base_name(index: &Index, class: Node, src: &str, owner: &str, name: &str) -> bool {
    [live_base(class, src), index.base_class(owner)]
        .into_iter()
        .flatten()
        .any(|base| base_chain_holds(index, base, name))
}

fn base_chain_holds(index: &Index, base: &str, name: &str) -> bool {
    let mut base = Some(base);
    // `base_class` has no cycle guard, so a cyclic `extends` pair needs a bound.
    for _ in 0..64 {
        let Some(current) = base else {
            return false;
        };
        if current == name {
            return true;
        }
        base = index.base_class(current);
    }
    false
}

fn declares_method(index: &Index, class: Node, src: &str, owner: &str, name: &str) -> bool {
    let local = class
        .child_by_field_name("body")
        .into_iter()
        .flat_map(|body| each_kind(body, "decl_method"))
        .filter_map(|m| m.child_by_field_name("name"))
        .any(|n| text_of(n, src) == name);
    local
        || [Some(owner), live_base(class, src)]
            .into_iter()
            .flatten()
            .flat_map(|class| index.find_members(class, name))
            .any(|d| matches!(d.member, MemberKind::Method(_)))
}
