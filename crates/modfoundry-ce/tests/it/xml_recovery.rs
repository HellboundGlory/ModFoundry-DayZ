//! Recovery from broken XML, the MF-X001 / MF-X002 / MF-X010 issues, and
//! the escape helpers. One test per recovery-table row, plus the no-panic sweep.

use crate::common::fixtures::slice;
use modfoundry_ce::xml::{decode, escape_attr, escape_text, parse, NodeKind, XmlDoc};
use modfoundry_ce::{Severity, Span};

fn span_of(d: &modfoundry_ce::Diagnostic) -> Span {
    d.span.expect("every issue carries a span")
}

/// Structural guarantees that must hold for every input.
fn check(text: &str) {
    let doc = parse(text);
    assert_eq!(doc.rebuild(text), text, "rebuild mismatch for {text:?}");

    for node in &doc.nodes {
        assert!(node.span.start <= node.span.end, "node order {text:?}");
        assert!(node.span.end <= text.len(), "node bounds {text:?}");
        assert!(
            text.is_char_boundary(node.span.start),
            "node start {text:?}"
        );
        assert!(text.is_char_boundary(node.span.end), "node end {text:?}");

        if let NodeKind::Element(el) = &node.kind {
            let spans = [Some(el.name_span), Some(el.open), el.close]
                .into_iter()
                .flatten()
                .chain(
                    el.attrs
                        .iter()
                        .flat_map(|a| [a.name_span, a.value_span, a.span]),
                );
            for span in spans {
                assert!(span.start <= span.end, "span order {text:?}");
                assert!(span.end <= text.len(), "span bounds {text:?}");
                assert!(
                    text.is_char_boundary(span.start) && text.is_char_boundary(span.end),
                    "span boundary {text:?}"
                );
            }
        }
    }

    for pair in doc.top.windows(2) {
        let a = &doc.nodes[pair[0]].span;
        let b = &doc.nodes[pair[1]].span;
        assert!(a.start <= b.start, "top not sorted {text:?}");
        assert!(a.end <= b.start, "top overlaps {text:?}");
    }
    for node in &doc.nodes {
        if let NodeKind::Element(el) = &node.kind {
            for pair in el.children.windows(2) {
                let a = &doc.nodes[pair[0]].span;
                let b = &doc.nodes[pair[1]].span;
                assert!(a.start <= b.start, "children not sorted {text:?}");
                assert!(a.end <= b.start, "children overlap {text:?}");
            }
        }
    }
    for pair in doc.issues.windows(2) {
        assert!(
            span_of(&pair[0]).start <= span_of(&pair[1]).start,
            "issues not sorted {text:?}"
        );
    }
}

fn only_issue(doc: &XmlDoc) -> &modfoundry_ce::Diagnostic {
    assert_eq!(doc.issues.len(), 1, "issues: {:?}", doc.issues);
    &doc.issues[0]
}

#[test]
fn intervening_element_is_closed_implicitly() {
    let text = "<r><b><c></b></r>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "c");
    assert_eq!(d.message, "<c> is never closed.");

    let r = doc.root().unwrap();
    let b = doc.children_named(r, "b")[0];
    let c = doc.children_named(b, "c")[0];
    assert!(doc.element(c).unwrap().close.is_none());
    assert_eq!(doc.element(b).unwrap().close.unwrap().start, 9);
}

#[test]
fn unmatched_end_tag_becomes_text() {
    let text = "<r></x></r>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "</x>");
    assert_eq!(d.message, "</x> closes nothing.");

    let r = doc.root().unwrap();
    assert!(doc.child_elements(r).is_empty());
    let kids = doc.element(r).unwrap().children.clone();
    assert_eq!(kids.len(), 1);
    assert!(matches!(doc.nodes[kids[0]].kind, NodeKind::Text { .. }));
}

#[test]
fn open_elements_end_at_eof() {
    let text = "<a><b>";
    check(text);
    let doc = parse(text);
    let spans: Vec<&str> = doc.issues.iter().map(|d| slice(text, span_of(d))).collect();
    assert_eq!(spans, ["a", "b"]);
    assert!(doc.issues.iter().all(|d| d.code == "MF-X001"));
    assert!(doc.issues.iter().all(|d| d.severity == Severity::Error));
    assert_eq!(doc.issues[0].message, "<a> is never closed.");
    assert_eq!(doc.issues[1].message, "<b> is never closed.");

    let a = doc.root().unwrap();
    let b = doc.children_named(a, "b")[0];
    assert_eq!(doc.nodes[a].span, Span::new(0, 6));
    assert_eq!(doc.nodes[b].span, Span::new(3, 6));
    assert!(doc.element(a).unwrap().close.is_none());
}

#[test]
fn unterminated_constructs_run_to_eof() {
    for (text, opener, message) in [
        ("<!--", "<!--", "This comment is never closed."),
        (
            "<![CDATA[",
            "<![CDATA[",
            "This CDATA section is never closed.",
        ),
        ("<?pi", "<?", "This processing instruction is never closed."),
        ("<?xml", "<?", "This XML declaration is never closed."),
        ("<!DOCTYPE", "<!DOCTYPE", "This DOCTYPE is never closed."),
    ] {
        check(text);
        let doc = parse(text);
        let d = only_issue(&doc);
        assert_eq!(d.code, "MF-X001", "{text}");
        assert_eq!(d.severity, Severity::Error, "{text}");
        assert_eq!(slice(text, span_of(d)), opener, "{text}");
        assert_eq!(d.message, message, "{text}");
        assert_eq!(
            doc.nodes[doc.top[0]].span,
            Span::new(0, text.len()),
            "{text}"
        );
    }
}

#[test]
fn start_tag_never_closed() {
    for text in ["<a", "<a <b>"] {
        check(text);
        let doc = parse(text);
        assert!(
            doc.issues.iter().any(|d| d.code == "MF-X001"
                && d.severity == Severity::Error
                && d.message == "This tag is never closed."
                && slice(text, span_of(d)) == "<a"),
            "{text}: {:?}",
            doc.issues
        );
    }

    let text = "<a <b>";
    let doc = parse(text);
    let a = doc.top_elements()[0];
    assert!(doc.element(a).unwrap().children.is_empty());
    assert_eq!(doc.top_elements().len(), 2);
}

#[test]
fn attribute_without_value() {
    let text = "<a x y=\"2\"/>";
    check(text);
    let doc = parse(text);
    let a = doc.root().unwrap();
    let x = doc.attr(a, "x").unwrap();
    assert_eq!(x.value, "");
    assert_eq!(x.value_span.start, x.value_span.end);
    assert_eq!(x.value_span.start, x.name_span.end);
    assert_eq!(x.quote, None);

    let d = doc
        .issues
        .iter()
        .find(|d| d.message == "`x` has no value.")
        .unwrap();
    assert_eq!(d.code, "MF-X001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "x");
}

#[test]
fn quoted_value_unterminated() {
    let text = "<a x=\"1";
    check(text);
    let doc = parse(text);
    let a = doc.root().unwrap();
    let x = doc.attr(a, "x").unwrap();
    assert_eq!(x.value, "1");
    assert_eq!(x.quote, Some('"'));
    assert_eq!(slice(text, x.value_span), "1");

    let d = doc
        .issues
        .iter()
        .find(|d| d.message == "This attribute value is never closed.")
        .unwrap();
    assert_eq!(d.code, "MF-X001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "\"");
}

#[test]
fn unquoted_value_is_a_hint() {
    let text = "<e a=1/>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X010");
    assert_eq!(d.severity, Severity::Hint);
    assert_eq!(slice(text, span_of(d)), "1");
    assert_eq!(
        d.message,
        "Quote the value: `a=\"1\"`. The game accepts it; other tools may not."
    );

    let e = doc.root().unwrap();
    assert_eq!(doc.attr(e, "a").unwrap().value, "1");
    assert_eq!(doc.attr(e, "a").unwrap().quote, None);
}

#[test]
fn lt_that_cannot_start_a_tag_is_text() {
    let text = "<r>a < b</r>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "<");
    assert_eq!(
        d.message,
        "`<` starts a tag; write `&lt;` for the character."
    );
}

#[test]
fn duplicate_attribute_is_an_error() {
    let text = "<type name=\"A\" name=\"B\"/>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X002");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(slice(text, span_of(d)), "name");
    assert_eq!(d.message, "`name` appears twice on `<type>`.");

    let t = doc.root().unwrap();
    assert_eq!(doc.element(t).unwrap().attrs.len(), 2);
}

#[test]
fn double_hyphen_in_comment_is_a_hint() {
    let text = "<!-- a -- b -->";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X010");
    assert_eq!(d.severity, Severity::Hint);
    assert_eq!(slice(text, span_of(d)), "--");
    assert_eq!(
        d.message,
        "XML doesn't allow `--` inside a comment. The game accepts it; other tools may not."
    );
}

#[test]
fn declaration_not_first_is_a_hint() {
    let text = "<!--c--><?xml version=\"1.0\"?><a/>";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X010");
    assert_eq!(d.severity, Severity::Hint);
    assert_eq!(slice(text, span_of(d)), "<?xml version=\"1.0\"?>");
    assert_eq!(d.message, "The XML declaration belongs on the first line.");
}

#[test]
fn text_outside_any_element_is_a_hint() {
    let text = "<a/>hello";
    check(text);
    let doc = parse(text);
    let d = only_issue(&doc);
    assert_eq!(d.code, "MF-X010");
    assert_eq!(d.severity, Severity::Hint);
    assert_eq!(slice(text, span_of(d)), "hello");
    assert_eq!(d.message, "Text outside any element.");

    assert!(parse("<a/>  \n\t ").issues.is_empty());
    assert!(parse("\u{FEFF}<a/>").issues.is_empty());
    assert!(parse("\u{FEFF}<?xml version=\"1.0\"?><a/>")
        .issues
        .is_empty());
}

#[test]
fn recovered_tree_keeps_both_records() {
    let text = "<types><type name=\"A\"><nominal>1</type><type name=\"B\"/></types>";
    check(text);
    let doc = parse(text);
    let root = doc.root().unwrap();
    assert_eq!(doc.children_named(root, "type").len(), 2);
    let first = doc.children_named(root, "type")[0];
    assert_eq!(doc.children_named(first, "nominal").len(), 1);
    let naming: Vec<&str> = doc
        .issues
        .iter()
        .filter(|d| d.message.contains("<nominal>"))
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(naming.len(), 1, "{:?}", doc.issues);
}

#[test]
fn clean_file_has_no_issues() {
    let text = "<?xml version=\"1.0\"?>\n<types>\n  <!-- ok -->\n  <type name=\"A\">\n    <nominal>10</nominal>\n  </type>\n</types>\n";
    check(text);
    let doc = parse(text);
    assert!(doc.issues.is_empty(), "{:?}", doc.issues);
    assert!(!doc.blocking());
}

#[test]
fn blocking_follows_error_severity() {
    assert!(parse("<a>").blocking());
    assert!(parse("<a b=\"1\" b=\"2\"/>").blocking());
    assert!(!parse("<e a=1/>").blocking());
    assert!(!parse("<a/>hello").blocking());
    assert!(!parse("<a/>").blocking());
}

#[test]
fn escapes_roundtrip_through_decode() {
    for s in [
        "& < > \" '",
        "a&b<c>d\"e'f",
        "",
        "<",
        ">",
        "&amp;",
        "no specials",
        "é & <ö> \" '",
    ] {
        assert_eq!(decode(&escape_text(s)), s, "escape_text {s:?}");
        assert_eq!(decode(&escape_attr(s, '"')), s, "escape_attr dq {s:?}");
        assert_eq!(decode(&escape_attr(s, '\'')), s, "escape_attr sq {s:?}");
    }
}

#[test]
fn escape_text_targets_and_attr_targets() {
    assert_eq!(escape_text("a&b<c>d"), "a&amp;b&lt;c&gt;d");
    assert_eq!(escape_attr("a&b<c>\"d'e", '"'), "a&amp;b&lt;c>&quot;d'e");
    assert_eq!(escape_attr("a&b<c>\"d'e", '\''), "a&amp;b&lt;c>\"d&apos;e");
}

fn types_fixture() -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<types>\n");
    for i in 0..12 {
        s.push_str(&format!(
            "    <type name=\"Item{i}\">\n        <nominal>{}</nominal>\n        <lifetime>7200</lifetime>\n        <flags count_in_map=\"1\" deloot=\"0\"/>\n    </type>\n",
            10 + i
        ));
    }
    s.push_str("</types>\n");
    s
}

fn spawnable_fixture() -> String {
    let mut s = String::from("<!-- spawnable types -->\n<spawnabletypes>\n");
    for i in 0..10 {
        s.push_str(&format!(
            "    <!-- type {i} -->\n    <type name=\"Item{i}\">\n        <attachments chance=\"0.50\">\n            <item name=\"Attachment{i}\" chance=\"1\"/>\n        </attachments>\n        <cargo chance=\"0.20\">\n            <item name=\"Cargo{i}\" chance=\"1\"/>\n        </cargo>\n    </type>\n"
        ));
    }
    s.push_str("</spawnabletypes>\n");
    s
}

#[test]
fn no_panic_on_every_prefix() {
    for fixture in [types_fixture(), spawnable_fixture()] {
        for i in 0..=fixture.len() {
            if fixture.is_char_boundary(i) {
                check(&fixture[..i]);
            }
        }
    }
}

#[test]
fn no_panic_on_snippets() {
    for text in [
        "<",
        "<<",
        "</",
        "</>",
        "<a",
        "<a b",
        "<a b=",
        "<a b=\"",
        "<a b='x",
        "<!--",
        "<!-- a -- b -->",
        "<![CDATA[",
        "<?",
        "<?xml",
        "<!DOCTYPE",
        "&",
        "&#;",
        "&#xZZ;",
        "a<b",
        "<a></b>",
        "</a>",
        "\u{FEFF}",
    ] {
        check(text);
    }
}
