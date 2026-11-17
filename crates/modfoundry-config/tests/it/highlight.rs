//! The config.cpp highlighter.

use modfoundry_config::highlight::{highlight, ConfigSpan, ConfigToken};

fn pairs(text: &str) -> Vec<(&str, ConfigToken)> {
    highlight(text)
        .into_iter()
        .map(|s| (&text[s.start..s.end], s.kind))
        .collect()
}

fn has(text: &str, snippet: &str, kind: ConfigToken) {
    let found = pairs(text);
    assert!(
        found.contains(&(snippet, kind)),
        "expected {snippet:?} as {kind:?} in {found:?}"
    );
}

#[test]
fn a_class_with_a_base_a_property_and_an_array() {
    let text = "class A: B { x = 1; y[] += { \"s\" }; };";
    has(text, "class", ConfigToken::Keyword);
    has(text, "A", ConfigToken::ClassName);
    has(text, "B", ConfigToken::ClassName);
    has(text, "x", ConfigToken::Property);
    has(text, "y", ConfigToken::Property);
    has(text, "1", ConfigToken::Number);
    has(text, "\"s\"", ConfigToken::Str);
    for punct in ["{", "}", ";", "=", ":", "[", "]"] {
        has(text, punct, ConfigToken::Punct);
    }
}

#[test]
fn a_class_name_is_not_a_property_and_a_value_word_is_plain() {
    let text = "class A { type = mod; };";
    has(text, "type", ConfigToken::Property);
    has(text, "mod", ConfigToken::Plain);
    assert!(!pairs(text).contains(&("A", ConfigToken::Property)));
}

#[test]
fn delete_and_external_are_keywords_and_external_names_a_class() {
    let text = "class C { delete Foo; };\nexternal class Bar;\n";
    has(text, "delete", ConfigToken::Keyword);
    has(text, "external", ConfigToken::Keyword);
    has(text, "Bar", ConfigToken::ClassName);
}

#[test]
fn a_preprocessor_line_is_one_span() {
    let text = "#include \"a.hpp\"\nclass A {};\n";
    let found = pairs(text);
    let preproc: Vec<_> = found
        .iter()
        .filter(|(_, kind)| *kind == ConfigToken::Preproc)
        .collect();
    assert_eq!(preproc.len(), 1, "{found:?}");
    assert!(preproc[0].0.starts_with("#include"), "{found:?}");
}

#[test]
fn comments_between_tokens_are_comment_spans() {
    let text = "// hi\nclass /* c */ A {}; /* x\n y */";
    has(text, "// hi", ConfigToken::Comment);
    has(text, "/* c */", ConfigToken::Comment);
    has(text, "/* x\n y */", ConfigToken::Comment);
    has(text, "class", ConfigToken::Keyword);
    has(text, "A", ConfigToken::ClassName);
}

#[test]
fn an_unterminated_string_highlights_what_came_before_it() {
    let text = "class A { s = \"abc";
    has(text, "class", ConfigToken::Keyword);
    has(text, "A", ConfigToken::ClassName);
    let quote = text.find('"').unwrap();
    assert!(
        highlight(text)
            .iter()
            .all(|s| s.start < quote || s.kind == ConfigToken::Plain),
        "{:?}",
        pairs(text)
    );
}

#[test]
fn crlf_line_endings_stay_out_of_every_span() {
    let text = "class A\r\n{\r\n    x = 1;\r\n};\r\n";
    has(text, "A", ConfigToken::ClassName);
    has(text, "x", ConfigToken::Property);
    has(text, "1", ConfigToken::Number);
    assert!(pairs(text).iter().all(|(s, _)| !s.contains(['\r', '\n'])));
}

#[test]
fn multibyte_text_in_a_string_keeps_spans_on_char_boundaries() {
    let text = "class A { s = \"héllo wörld ☃\"; };";
    has(text, "\"héllo wörld ☃\"", ConfigToken::Str);
    for span in highlight(text) {
        assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
    }
}

fn fixture() -> String {
    let block = "\
#include \"defs.hpp\"
// leading comment
class CfgPatches
{
    class Fixture_Data { requiredAddons[] = { \"DZ_Data\", \"JM_CF\" }; units[] = {}; };
};
/* block
   comment */
class CfgVehicles
{
    class Base;
    class Canteen: Base { scope = 2; displayName = \"Feld – Flasche ☃\"; weight = 0.5; list[] += { 1, 2 }; };
    delete Gone;
};
";
    block.repeat(2048 / block.len() + 1)
}

fn assert_well_formed(text: &str, spans: &[ConfigSpan]) {
    let mut previous_end = 0;
    for span in spans {
        assert!(span.start < span.end, "{span:?}");
        assert!(
            span.start >= previous_end,
            "overlap or disorder at {span:?}"
        );
        assert!(span.end <= text.len(), "{span:?} out of bounds");
        assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
        let snippet = &text[span.start..span.end];
        assert_eq!(
            snippet,
            snippet.trim(),
            "whitespace at the edge of {span:?}"
        );
        previous_end = span.end;
    }
}

#[test]
fn every_prefix_of_a_fixture_highlights_without_panicking() {
    let text = fixture();
    for end in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
        let prefix = &text[..end];
        assert_well_formed(prefix, &highlight(prefix));
    }
}
