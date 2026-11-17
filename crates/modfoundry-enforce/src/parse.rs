//! Pure Enforce syntax analysis over a source string.
//!
//! Parses with the tree-sitter Enforce grammar and maps any `ERROR`/`MISSING`
//! tree nodes to [`Diagnostic`]s. Pure (no IO) so it runs in tests/CI/WASM;
//! module classification is [`detect`], IO lives in [`lint`].

use crate::{Diagnostic, Severity, Span};
use tree_sitter::{Parser, Tree};

/// Enforce syntax error code (`MF-E1001…`).
pub const E_SYNTAX: &str = "MF-E1001";

/// Parse `text` with the tree-sitter Enforce grammar; broken input yields a tree with error nodes.
pub fn parse_tree(text: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_enforce::LANGUAGE.into())
        .expect("tree-sitter-enforce grammar loads");
    // A parse cannot fail once a language is set.
    parser.parse(text, None).expect("parse with a language set")
}

/// Syntax diagnostics for the parsed `text`. Returns `MF-E1001` per top-level
/// syntax fault; empty when the source parses cleanly.
pub fn syntax_diagnostics(tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect_errors(tree.root_node(), text, &mut out);
    out.dedup_by(|a, b| a.span == b.span && a.message == b.message);
    out
}

/// ``missing `}` `` for a token, `missing identifier` for a named node.
fn missing_message(node: tree_sitter::Node) -> String {
    if node.is_named() {
        format!("missing {}", node.kind().replace('_', " "))
    } else {
        format!("missing `{}`", node.kind())
    }
}

/// Depth-first: report each `ERROR` node at its topmost extent (its children
/// are subsumed) and each `MISSING` node (expected-but-absent token).
fn collect_errors(node: tree_sitter::Node, text: &str, out: &mut Vec<Diagnostic>) {
    if node.is_error() {
        out.push(Diagnostic::at(
            E_SYNTAX,
            Severity::Error,
            span_of(node),
            format!("syntax error near `{}`", snippet(node, text)),
        ));
        return;
    }
    // Enforce accepts a statement with no `;` before `}` or a line end (vanilla does it).
    if node.is_missing() && node.kind() != ";" {
        out.push(Diagnostic::at(
            E_SYNTAX,
            Severity::Error,
            Span::new(node.start_byte(), node.start_byte()),
            missing_message(node),
        ));
        return;
    }
    // A cursor, because `next_sibling` skips a zero-width MISSING node.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, text, out);
    }
}

fn span_of(node: tree_sitter::Node) -> Span {
    Span::new(node.start_byte(), node.end_byte())
}

/// The offending source slice, trimmed to a readable length.
fn snippet(node: tree_sitter::Node, text: &str) -> String {
    const MAX: usize = 40;
    let (s, e) = (node.start_byte(), node.end_byte());
    let raw = text.get(s..e).unwrap_or_default().trim();
    let snip: String = raw.chars().take(MAX).collect();
    if raw.chars().count() > MAX {
        format!("{snip}…")
    } else {
        snip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnose(text: &str) -> Vec<Diagnostic> {
        syntax_diagnostics(&parse_tree(text), text)
    }

    #[test]
    fn class_missing_its_closing_brace_is_a_syntax_error() {
        let ds = diagnose("class A\n{\n    void F() {}\n");
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!(ds[0].code, E_SYNTAX);
    }

    #[test]
    fn an_unclosed_method_in_an_unclosed_class_is_one_named_diagnostic() {
        let src = "class A\n{\n    void F()\n    {\n        Print(1);\n";
        let ds = diagnose(src);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!(ds[0].span, Some(Span::new(46, 46)));
        assert_eq!(ds[0].message, "missing `}`");

        let ds = diagnose("class A\n{\n    void F() {}\n");
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!(ds[0].message, "missing `}`");
    }

    #[test]
    fn clean_source_has_no_diagnostics() {
        for src in [
            "class Foo {\n    void hello() {\n        Print(\"hi\");\n    }\n}\n",
            "modded class Foo {\n    void hello() {}\n}\n",
            "[CF_Export()]\nenum MyEnum {\n    A,\n    B\n}\n",
        ] {
            assert!(diagnose(src).is_empty(), "should parse clean: {src}");
        }
    }

    #[test]
    fn lenient_constructs_parse_clean() {
        for src in [
            "class A { private ref static W s; ref protected W t; }\n",
            "class A { autoptr array<autoptr X> m = new array<autoptr X>(); }\n",
            "void f(local array<string> a, local bool b = false) {}\n",
            "class A { protected bool b;; }\n",
            "enum E { A = 0\n B = 1,\n C, = 2,\n D }\n",
            "void f(int g) { switch (g) { int p = 0; case 1: break; default: break; } }\n",
            "void f(int g) { switch (g) { if (x) y = 1; else y = 0; } }\n",
            "void f(int g) { switch (g) { case 1: return 1; default: return 2; case 2: return 3; } }\n",
            "class A { [Obsolete(\"x\")]; void f(); }\n",
            "class A {\n    protected int a\n    protected bool b;\n}\n",
            "void f() {\n    while (a) {\n        continue\n    }\n}\n",
            "void f() {\n    while (a) {\n        continue\n        Print(a);\n    }\n}\n",
            "void f() {\n    while (a) {\n        break\n    }\n}\n",
            "void f() {\n    while (a) {\n        break\n        Print(a);\n    }\n}\n",
            "void f() {\n    return x\n}\n",
            "void f() {\n    return x\n    Print(x);\n}\n",
            "void f() {\n    int a = 1\n    Print(a);\n}\n",
            "void f() {\n    bool r = a // note\n        && b;\n}\n",
            "int a = b\n + c;\n",
            "void f() { g(a,\n        b); }\n",
            "void f() { x\n        .M(); }\n",
            "class A { void R() { m = 0 } }\n",
            "class A { void R(){m = 0}; }\n",
            "void f() { if (a) {m = B.C} }\n",
            "proto native int F(int a)\nproto native int G(int b);\n",
            "class B { int g[2][3]; }\n",
            "class C\n{\n    int h;\n    int g[2][3];\n}\n",
            "int G[2][3];\nint K[2][3] = 1;\n",
            "class A { int a[2][3], b; }\n",
            "class A { int g[2] [3]; }\n",
            "void f() {\n    int c[2][3];\n    int d;\n}\n",
        ] {
            assert!(diagnose(src).is_empty(), "should parse clean: {src}");
        }
    }

    #[test]
    fn dangling_brace_is_a_syntax_error() {
        let ds = diagnose("}\n");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].code, E_SYNTAX);
        assert_eq!(ds[0].severity, Severity::Error);
        let span = ds[0].span.expect("spans the fault");
        assert_eq!(span.start, 0);
        assert!(span.end <= "}\n".len());
        assert!(!ds[0].message.is_empty());
    }

    #[test]
    fn class_without_name_is_a_syntax_error() {
        let ds = diagnose("class { }\n");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].code, E_SYNTAX);
        assert_eq!(ds[0].severity, Severity::Error);
    }

    #[test]
    fn each_top_level_fault_is_reported() {
        let ds = diagnose("class A { @ % }\nclass B { @ % }\n");
        assert!(ds.len() >= 2, "expected separate faults, got {}", ds.len());
        assert!(ds.iter().all(|d| d.code == E_SYNTAX));
    }

    #[test]
    fn an_automatic_semicolon_is_zero_width() {
        let src = "void f() {\n    return x\n}\n";
        let tree = parse_tree(src);
        let method = tree.root_node().child(0).expect("method");
        let body = method.child_by_field_name("body").expect("body");
        let value = body
            .named_child(0)
            .expect("return")
            .named_child(0)
            .expect("returned");
        assert_eq!(&src[value.start_byte()..value.end_byte()], "x");
        assert_eq!(body.end_byte(), src.len() - 1);
    }

    #[test]
    fn spans_stay_within_source_bounds() {
        let src = "class Foo { @ % }\n";
        for d in diagnose(src) {
            let span = d.span.unwrap();
            assert!(span.start <= span.end && span.end <= src.len());
        }
    }
}
