//! A `foreach` straight over a call result throws in the game on the first such loop to run.

use crate::{Diagnostic, Severity, Span, Tree};
use tree_sitter::Node;

/// A `foreach` whose iterated expression is a call.
pub const W_LOOP_OVER_CALL: &str = "MF-W3005";

pub(crate) fn loops_over_calls(tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect(tree.root_node(), text, &mut out);
    out
}

fn collect(node: Node, text: &str, out: &mut Vec<Diagnostic>) {
    if node.kind() == "foreach" {
        out.extend(over_call(node, text));
    }
    let mut child = node.child(0);
    while let Some(c) = child {
        collect(c, text, out);
        child = c.next_sibling();
    }
}

/// Parentheses around the iterated expression are separate children of the
/// `iterated` field, so the expression is the one named child in it.
fn over_call(foreach: Node, text: &str) -> Option<Diagnostic> {
    let mut cursor = foreach.walk();
    let parts: Vec<Node> = foreach
        .children_by_field_name("iterated", &mut cursor)
        .collect();
    let named: Vec<&Node> = parts.iter().filter(|part| part.is_named()).collect();
    let [call] = named.as_slice() else {
        return None;
    };
    if call.kind() != "invokation" {
        return None;
    }
    let (first, last) = (parts.first()?, parts.last()?);
    let span = Span::new(first.start_byte(), last.end_byte());
    let call_text = text.get(span.start..span.end).unwrap_or_default();
    let call_text = call_text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(Diagnostic::at(
        W_LOOP_OVER_CALL,
        Severity::Warning,
        span,
        format!(
            "foreach straight over `{call_text}`: the game throws \"NULL pointer to instance\" at the first such loop to run; hold the result in a local first"
        ),
    ))
}
