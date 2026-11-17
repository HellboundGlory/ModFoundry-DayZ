//! Local names declared twice in one function body.
//!
//! DayZ's script compiler refuses a name declared twice in the same function,
//! even in sibling blocks, so the whole body is one scope.

use crate::{Diagnostic, Severity, Span};
use std::collections::HashMap;
use tree_sitter::{Node, Tree};

/// A local name declared twice in one function.
pub const E_DUPLICATE_LOCAL: &str = "MF-E1002";

pub(crate) fn duplicate_locals(tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    each_method(tree.root_node(), text, &mut out);
    out
}

fn each_method(node: Node, text: &str, out: &mut Vec<Diagnostic>) {
    if node.kind() == "decl_method" {
        let mut scope = Scope::default();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            scan(child, text, &mut scope, out);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        each_method(child, text, out);
    }
}

/// Where a declaration sits: the `#if` arms open around it, each as
/// (conditional, arm), outermost first.
type ArmPath = Vec<(usize, usize)>;

/// Names declared so far in one function, and the `#if` arms open now.
#[derive(Default)]
struct Scope<'a> {
    declared: HashMap<&'a str, Vec<ArmPath>>,
    open: ArmPath,
    conditionals: usize,
}

impl<'a> Scope<'a> {
    fn open(&mut self) {
        self.open.push((self.conditionals, 0));
        self.conditionals += 1;
    }

    fn otherwise(&mut self) {
        if let Some((_, arm)) = self.open.last_mut() {
            *arm += 1;
        }
    }

    fn close(&mut self) {
        self.open.pop();
    }

    /// Records `name` here; true when an earlier declaration can be compiled
    /// together with this one. Two arms of one `#if`, or two separate `#if`
    /// blocks, are alternatives the compiler may never see together.
    fn declare(&mut self, name: &'a str) -> bool {
        let here = self.open.clone();
        let paths = self.declared.entry(name).or_default();
        let clash = paths
            .iter()
            .any(|earlier| earlier.starts_with(&here) || here.starts_with(earlier));
        paths.push(here);
        clash
    }
}

fn scan<'a>(node: Node, text: &'a str, scope: &mut Scope<'a>, out: &mut Vec<Diagnostic>) {
    match node.kind() {
        "decl_method" | "decl_class" => return,
        _ if node.is_error() => return,
        "formal_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                declare(name, text, scope, out);
            }
        }
        "decl_variable" if !node.has_error() && !is_misread_else(node, text) => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                declare(name, text, scope, out);
            }
        }
        "iterator" => {
            let mut cursor = node.walk();
            let name = node
                .children(&mut cursor)
                .find(|c| c.kind() == "identifier");
            if let Some(name) = name {
                declare(name, text, scope, out);
            }
        }
        "preproc_ifdef" | "preproc_ifndef" => scope.open(),
        "preproc_else" => scope.otherwise(),
        "preproc_endif" => scope.close(),
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scan(child, text, scope, out);
    }
}

fn declare<'a>(name: Node, text: &'a str, scope: &mut Scope<'a>, out: &mut Vec<Diagnostic>) {
    let name_text = ident(name, text);
    if scope.declare(name_text) {
        out.push(Diagnostic::at(
            E_DUPLICATE_LOCAL,
            Severity::Error,
            Span::new(name.start_byte(), name.end_byte()),
            format!("multiple declaration of variable '{name_text}'"),
        ));
    }
}

/// The grammar reads an unbraced `else` before a call as a declaration of type `else`.
fn is_misread_else(node: Node, text: &str) -> bool {
    node.child_by_field_name("type")
        .is_some_and(|ty| ident(ty, text) == "else")
}

fn ident<'a>(name: Node, text: &'a str) -> &'a str {
    text.get(name.byte_range()).unwrap_or_default()
}
