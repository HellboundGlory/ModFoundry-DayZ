//! Pure syntax highlighting for the editor: the grammar's own
//! `queries/highlights.scm` through `tree-sitter-highlight`.

use std::sync::LazyLock;

use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

/// Coarse syntax category of a highlighted region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Constant,
    Preprocessor,
    Operator,
    Punctuation,
    Variable,
}

/// A half-open byte range `[start, end)` of the source and its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

/// Capture name → category, in `configure` precedence order: ties go to the
/// earlier name, so `keyword` precedes `type`/`operator`. A node resolves to its
/// last capture, and the query captures `@spell` after comments and
/// `@_classname` after class names, so both map to the node's real category.
const CAPTURES: &[(&str, HighlightKind)] = &[
    ("keyword.directive", HighlightKind::Preprocessor),
    ("constant.macro", HighlightKind::Preprocessor),
    ("keyword", HighlightKind::Keyword),
    ("boolean", HighlightKind::Keyword),
    ("constant.builtin", HighlightKind::Keyword),
    ("type", HighlightKind::Type),
    ("constructor", HighlightKind::Type),
    ("_classname", HighlightKind::Type),
    ("function", HighlightKind::Function),
    ("string", HighlightKind::String),
    ("number", HighlightKind::Number),
    ("comment", HighlightKind::Comment),
    ("spell", HighlightKind::Comment),
    ("constant", HighlightKind::Constant),
    ("operator", HighlightKind::Operator),
    ("punctuation", HighlightKind::Punctuation),
    ("variable", HighlightKind::Variable),
];

static CONFIG: LazyLock<Option<HighlightConfiguration>> = LazyLock::new(|| {
    let mut config = HighlightConfiguration::new(
        tree_sitter_enforce::LANGUAGE.into(),
        "enforce",
        tree_sitter_enforce::HIGHLIGHTS_QUERY,
        "",
        "",
    )
    .ok()?;
    let names: Vec<&str> = CAPTURES.iter().map(|(name, _)| *name).collect();
    config.configure(&names);
    Some(config)
});

/// Byte spans for `source`, ascending and non-overlapping. Unknown captures are
/// skipped, and any highlighter failure yields no spans.
pub fn highlight(source: &str) -> Vec<HighlightSpan> {
    let Some(config) = CONFIG.as_ref() else {
        return Vec::new();
    };
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(config, source.as_bytes(), None, |_| None) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    let mut stack: Vec<Highlight> = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(highlight)) => stack.push(highlight),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(kind) = stack.last().copied().and_then(kind_of) {
                    push_span(&mut spans, start, end, kind);
                }
            }
            Err(_) => return Vec::new(),
        }
    }
    spans
}

fn kind_of(highlight: Highlight) -> Option<HighlightKind> {
    CAPTURES.get(highlight.0).map(|(_, kind)| *kind)
}

fn push_span(spans: &mut Vec<HighlightSpan>, start: usize, end: usize, kind: HighlightKind) {
    if start >= end {
        return;
    }
    match spans.last_mut() {
        Some(last) if last.end == start && last.kind == kind => last.end = end,
        _ => spans.push(HighlightSpan { start, end, kind }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "class Foo extends Bar\n{\n\tint x = 5; // hi\n\tstring s = \"a\";\n};";

    fn kind_at(source: &str, text: &str) -> Option<HighlightKind> {
        highlight(source)
            .into_iter()
            .find(|s| &source[s.start..s.end] == text)
            .map(|s| s.kind)
    }

    #[test]
    fn grammar_highlights_query_is_accepted() {
        assert!(CONFIG.is_some());
    }

    #[test]
    fn sample_source_is_classified() {
        assert_eq!(kind_at(SAMPLE, "class"), Some(HighlightKind::Keyword));
        assert_eq!(kind_at(SAMPLE, "5"), Some(HighlightKind::Number));
        assert_eq!(kind_at(SAMPLE, "// hi"), Some(HighlightKind::Comment));
        assert_eq!(kind_at(SAMPLE, "\"a\""), Some(HighlightKind::String));
        assert!(
            kind_at(SAMPLE, "Foo") == Some(HighlightKind::Type)
                || kind_at(SAMPLE, "Bar") == Some(HighlightKind::Type),
            "class name or superclass should be a type"
        );
    }

    #[test]
    fn autoptr_in_a_type_argument_is_a_keyword() {
        let source = "class A { array<autoptr X> m; }";
        assert_eq!(kind_at(source, "autoptr"), Some(HighlightKind::Keyword));
    }

    #[test]
    fn class_name_stays_a_type_when_the_class_has_a_constructor() {
        let source = "class Foo\n{\n\tvoid Foo() {}\n};";
        let spans = highlight(source);
        let first_foo = source.find("Foo").unwrap();
        let span = spans
            .iter()
            .find(|s| s.start == first_foo)
            .expect("class name span");
        assert_eq!(span.kind, HighlightKind::Type);
    }

    #[test]
    fn spans_are_sorted_disjoint_and_in_bounds_for_any_input() {
        for source in [
            SAMPLE,
            "",
            "class A { void f() { return; } }",
            "}}}{{{ class",
            "@@@ ///",
            "class {",
            "\"unterminated",
        ] {
            let spans = highlight(source);
            let mut prev_end = 0;
            for s in &spans {
                assert!(s.start <= s.end, "inverted span {s:?} in {source:?}");
                assert!(s.end <= source.len(), "span past EOF {s:?} in {source:?}");
                assert!(
                    s.start >= prev_end,
                    "overlap or disorder in {source:?}: {spans:?}"
                );
                prev_end = s.end;
            }
        }
    }
}
