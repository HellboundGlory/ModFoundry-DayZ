//! A non-void method must end in a `return`, as the script compiler requires.

use crate::{Diagnostic, Severity, Span, Tree};
use tree_sitter::Node;

/// A non-void method whose body does not end in a `return`.
pub const E_MISSING_RETURN: &str = "MF-E1003";

pub(crate) fn missing_returns(tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect(tree.root_node(), text, &mut out);
    out
}

fn collect(node: Node, text: &str, out: &mut Vec<Diagnostic>) {
    if node.kind() == "decl_method" {
        check_method(node, text, out);
        return;
    }
    let mut child = node.child(0);
    while let Some(c) = child {
        collect(c, text, out);
        child = c.next_sibling();
    }
}

fn check_method(method: Node, text: &str, out: &mut Vec<Diagnostic>) {
    let (Some(return_type), Some(body), Some(name)) = (
        method.child_by_field_name("return_type"),
        method.child_by_field_name("body"),
        method.child_by_field_name("name"),
    ) else {
        return;
    };
    // Empty stubs such as `bool IsReady() {}` compile in vanilla scripts.
    if return_type.kind() == "type_void"
        || method.has_error()
        || splits_a_statement(body)
        || ends_with_return(body) != Some(false)
    {
        return;
    }
    let name_text = text.get(name.byte_range()).unwrap_or_default();
    out.push(Diagnostic::at(
        E_MISSING_RETURN,
        Severity::Error,
        Span::new(name.start_byte(), name.end_byte()),
        format!("no return statement at the end of '{name_text}', which returns a value"),
    ));
}

fn ends_in_return(statement: Node) -> bool {
    match statement.kind() {
        "return" => true,
        "block" => ends_with_return(statement) == Some(true),
        "if" => match (
            statement.child_by_field_name("consequence"),
            statement.child_by_field_name("alternative"),
        ) {
            (Some(then), Some(otherwise)) => ends_in_return(then) && ends_in_return(otherwise),
            _ => false,
        },
        _ => false,
    }
}

/// Whether the block's last statement returns, or `None` for an empty block.
/// Each build sees one arm of an `#if` region, and a body the preprocessor
/// empties compiles, so a region counts as returning when every arm that holds
/// statements ends in a `return` and it has an `#else`; one with no `#else`
/// counts only when it does not return (what runs before it decides otherwise).
fn ends_with_return(block: Node) -> Option<bool> {
    let mut last = None;
    let mut regions: Vec<Region> = Vec::new();
    let mut cursor = block.walk();
    for statement in block.named_children(&mut cursor) {
        let ends = match statement.kind() {
            "preproc_ifdef" | "preproc_ifndef" => {
                regions.push(Region::default());
                continue;
            }
            "preproc_else" => {
                if let Some(region) = regions.last_mut() {
                    region.first_arm = Some(region.arm.take());
                }
                continue;
            }
            "preproc_endif" => match regions.pop().and_then(Region::ends) {
                Some(ends) => ends,
                None => continue,
            },
            kind if is_trivia(kind) => continue,
            _ => ends_in_return(statement),
        };
        match regions.last_mut() {
            Some(region) => region.arm = Some(ends),
            None => last = Some(ends),
        }
    }
    // An `#endif` the grammar nested inside a statement leaves its region open.
    while let Some(region) = regions.pop() {
        if let Some(ends) = region.ends() {
            match regions.last_mut() {
                Some(outer) => outer.arm = Some(ends),
                None => last = Some(ends),
            }
        }
    }
    last
}

/// One `#if` region being read: the first arm's ending once `#else` is seen,
/// and the current arm's.
#[derive(Default)]
struct Region {
    first_arm: Option<Option<bool>>,
    arm: Option<bool>,
}

impl Region {
    /// `None` when the region decides nothing about how the block ends.
    fn ends(self) -> Option<bool> {
        match self.first_arm {
            None => self.arm.filter(|returns| !returns),
            Some(first) => match (first, self.arm) {
                (None, None) => None,
                (Some(a), Some(b)) => Some(a && b),
                (Some(a), None) | (None, Some(a)) => Some(a),
            },
        }
    }
}

/// Whether a preprocessor line sits inside a statement (`#ifdef` around an
/// `if` header): the grammar cannot model that, so the tree says nothing reliable.
fn splits_a_statement(node: Node) -> bool {
    let mut cursor = node.walk();
    let split = node.children(&mut cursor).any(|child| {
        (matches!(
            child.kind(),
            "preproc_ifdef" | "preproc_ifndef" | "preproc_else" | "preproc_endif"
        ) && node.kind() != "block")
            || splits_a_statement(child)
    });
    split
}

/// Comments, preprocessor lines and stray semicolons do not count as the last statement.
fn is_trivia(kind: &str) -> bool {
    kind.starts_with("comment_")
        || kind.starts_with("doc_")
        || kind.starts_with("preproc_")
        || kind == "empty_statement"
}
