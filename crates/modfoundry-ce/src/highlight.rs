//! Syntax highlighting for the Central Economy Source view.
//!
//! A standalone scanner, not [`crate::xml::parse`]: it runs on every keystroke
//! over text that is usually broken mid-edit, so it never fails and tolerates
//! unterminated constructs.

/// What a highlighted run of source text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlToken {
    /// An element name after `<` or `</`.
    Tag,
    /// An attribute name inside a tag.
    AttrName,
    /// A quoted attribute value, quotes included.
    AttrValue,
    /// A whole number or decimal in element text.
    Number,
    /// A `<!-- ... -->` comment.
    Comment,
    /// A `<? ... ?>` declaration or a `<!DOCTYPE ... >`.
    Decl,
    /// An `&name;`, `&#N;` or `&#xH;` reference in element text.
    Entity,
}

/// A highlighted byte range of source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlSpan {
    /// Start byte offset, on a char boundary.
    pub start: usize,
    /// End byte offset, exclusive, on a char boundary.
    pub end: usize,
    /// What the range is.
    pub kind: XmlToken,
}

/// Highlights `text`, returning sorted, non-overlapping spans.
pub fn highlight(text: &str) -> Vec<XmlSpan> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("<!--") {
            let end = find_from(text, i + 4, "-->").map_or(text.len(), |j| j + 3);
            push(&mut spans, i, end, XmlToken::Comment);
            i = end;
        } else if rest.starts_with("<?") {
            let end = find_from(text, i + 2, "?>").map_or(text.len(), |j| j + 2);
            push(&mut spans, i, end, XmlToken::Decl);
            i = end;
        } else if rest.starts_with("<!DOCTYPE") {
            let end = doctype_end(text, i);
            push(&mut spans, i, end, XmlToken::Decl);
            i = end;
        } else if rest.starts_with('<') {
            i = tag(text, i, &mut spans);
        } else {
            let end = text[i..].find('<').map_or(text.len(), |j| i + j);
            text_run(text, i, end, &mut spans);
            i = end;
        }
    }
    spans
}

fn find_from(text: &str, start: usize, needle: &str) -> Option<usize> {
    text[start..].find(needle).map(|j| start + j)
}

fn push(spans: &mut Vec<XmlSpan>, start: usize, end: usize, kind: XmlToken) {
    if start < end {
        spans.push(XmlSpan { start, end, kind });
    }
}

fn is_name_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '=' | '/' | '>' | '<' | '"' | '\'')
}

fn scan_name(text: &str, mut i: usize) -> usize {
    while let Some(c) = text[i..].chars().next() {
        if is_name_delim(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

fn scan_quoted(text: &str, start: usize, quote: char) -> (usize, bool) {
    let mut i = start + quote.len_utf8();
    while let Some(c) = text[i..].chars().next() {
        if c == quote {
            return (i + c.len_utf8(), true);
        }
        i += c.len_utf8();
    }
    (text.len(), false)
}

fn tag(text: &str, start: usize, spans: &mut Vec<XmlSpan>) -> usize {
    let mut i = start + 1;
    if text[i..].starts_with('/') {
        i += 1;
    }
    while i < text.len() && text[i..].chars().next().unwrap().is_whitespace() {
        i += text[i..].chars().next().unwrap().len_utf8();
    }
    let name_end = scan_name(text, i);
    push(spans, i, name_end, XmlToken::Tag);
    i = name_end;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '>' => return i + 1,
            '"' | '\'' => {
                let (end, terminated) = scan_quoted(text, i, c);
                push(spans, i, end, XmlToken::AttrValue);
                if !terminated {
                    return end;
                }
                i = end;
            }
            '=' | '/' => i += c.len_utf8(),
            '<' => return i,
            _ if c.is_whitespace() => i += c.len_utf8(),
            _ => {
                let end = scan_name(text, i);
                push(spans, i, end, XmlToken::AttrName);
                i = end;
            }
        }
    }
    text.len()
}

fn doctype_end(text: &str, start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start + "<!DOCTYPE".len();
    while let Some(c) = text[i..].chars().next() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '>' if depth == 0 => return i + 1,
            _ => {}
        }
        i += c.len_utf8();
    }
    text.len()
}

fn text_run(text: &str, start: usize, end: usize, spans: &mut Vec<XmlSpan>) {
    let run = &text[start..end];
    let trimmed = run.trim();
    if !trimmed.is_empty() && is_number(trimmed) {
        let lead = run.len() - run.trim_start().len();
        push(
            spans,
            start + lead,
            start + lead + trimmed.len(),
            XmlToken::Number,
        );
        return;
    }
    let mut i = start;
    while i < end {
        if text[i..].starts_with('&') {
            if let Some(e) = entity_end(text, i, end) {
                push(spans, i, e, XmlToken::Entity);
                i = e;
                continue;
            }
        }
        i += text[i..].chars().next().unwrap().len_utf8();
    }
}

fn entity_end(text: &str, start: usize, limit: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    if i < limit && bytes[i] == b'#' {
        i += 1;
        let hex = i < limit && (bytes[i] == b'x' || bytes[i] == b'X');
        if hex {
            i += 1;
        }
        let first = i;
        while i < limit {
            let ok = if hex {
                bytes[i].is_ascii_hexdigit()
            } else {
                bytes[i].is_ascii_digit()
            };
            if !ok {
                break;
            }
            i += 1;
        }
        return (i > first && i < limit && bytes[i] == b';').then_some(i + 1);
    }
    let first = i;
    while i < limit
        && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b':' | b'.' | b'-'))
    {
        i += 1;
    }
    (i > first && i < limit && bytes[i] == b';').then_some(i + 1)
}

fn is_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));
    let first = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == first {
        return false;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let first = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == first {
            return false;
        }
    }
    i == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn of<'a>(text: &'a str, spans: &[XmlSpan], kind: XmlToken) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| &text[s.start..s.end])
            .collect()
    }

    fn check(text: &str, spans: &[XmlSpan]) {
        let mut prev = 0;
        for s in spans {
            assert!(s.start < s.end, "empty span {s:?} in {text:?}");
            assert!(s.start >= prev, "unsorted/overlapping {s:?} in {text:?}");
            assert!(s.end <= text.len(), "out of bounds {s:?} in {text:?}");
            assert!(
                text.is_char_boundary(s.start) && text.is_char_boundary(s.end),
                "misaligned {s:?} in {text:?}"
            );
            prev = s.end;
        }
    }

    #[test]
    fn each_kind_on_a_types_file() {
        let t = "<?xml version=\"1.0\"?><!-- note --><types>\r\n  <type name=\"A\">\r\n    <nominal>12</nominal>\r\n    <name>abc</name>\r\n    <flags count_in_map=\"1\" deloot=\"0\"/>\r\n    <user text=\"café &amp; tea\">x &#65; y</user>\r\n  </type>\r\n</types>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(of(t, &spans, XmlToken::Decl), ["<?xml version=\"1.0\"?>"]);
        assert_eq!(of(t, &spans, XmlToken::Comment), ["<!-- note -->"]);
        assert_eq!(
            of(t, &spans, XmlToken::Tag),
            [
                "types", "type", "nominal", "nominal", "name", "name", "flags", "user", "user",
                "type", "types"
            ]
        );
        assert_eq!(
            of(t, &spans, XmlToken::AttrName),
            ["name", "count_in_map", "deloot", "text"]
        );
        assert_eq!(
            of(t, &spans, XmlToken::AttrValue),
            ["\"A\"", "\"1\"", "\"0\"", "\"café &amp; tea\""]
        );
        assert_eq!(of(t, &spans, XmlToken::Number), ["12"]);
        assert_eq!(of(t, &spans, XmlToken::Entity), ["&#65;"]);
    }

    #[test]
    fn number_only_over_numeric_text() {
        let t = "<nominal>12</nominal><name>abc</name>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(of(t, &spans, XmlToken::Number), ["12"]);
    }

    #[test]
    fn decimal_number_is_trimmed() {
        let t = "<a>\r\n  -12.5  \r\n</a>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(of(t, &spans, XmlToken::Number), ["-12.5"]);
    }

    #[test]
    fn entities_are_recognised_in_all_three_forms() {
        let t = "<a>&amp; &#65; &#x41; &bogus &#xZZ; & ;</a>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(
            of(t, &spans, XmlToken::Entity),
            ["&amp;", "&#65;", "&#x41;"]
        );
    }

    #[test]
    fn comment_between_every_token() {
        let t = "<?xml v=\"1\"?><!--c--><a><!--c--><b x=\"2\"><!--c-->3<!--c--></b><!--c--></a>";
        let spans = highlight(t);
        check(t, &spans);
        let kinds: Vec<XmlToken> = spans.iter().map(|s| s.kind).collect();
        for k in [
            XmlToken::Decl,
            XmlToken::Comment,
            XmlToken::Tag,
            XmlToken::AttrName,
            XmlToken::AttrValue,
            XmlToken::Number,
        ] {
            assert!(kinds.contains(&k), "missing {k:?} in {kinds:?}");
        }
    }

    #[test]
    fn crlf_and_multibyte_values() {
        let t = "<a>\r\n  <b x=\"café\">12\r\n  </b>\r\n</a>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(of(t, &spans, XmlToken::AttrValue), ["\"café\""]);
        assert_eq!(of(t, &spans, XmlToken::Number), ["12"]);
    }

    #[test]
    fn bom_before_declaration() {
        let t = "\u{FEFF}<?xml version=\"1.0\"?><types/>";
        let spans = highlight(t);
        check(t, &spans);
        assert_eq!(spans[0].kind, XmlToken::Decl);
        assert_eq!(&t[spans[0].start..spans[0].end], "<?xml version=\"1.0\"?>");
        assert_eq!(of(t, &spans, XmlToken::Tag), ["types"]);
    }

    #[test]
    fn doctype_and_pi_declarations() {
        for (t, decl) in [
            ("<!DOCTYPE x SYSTEM \"y\"><a/>", "<!DOCTYPE x SYSTEM \"y\">"),
            ("<?pi data?><a/>", "<?pi data?>"),
        ] {
            let spans = highlight(t);
            check(t, &spans);
            assert_eq!(spans[0].kind, XmlToken::Decl, "{t:?}");
            assert_eq!(&t[spans[0].start..spans[0].end], decl);
            assert_eq!(of(t, &spans, XmlToken::Tag), ["a"], "{t:?}");
        }
    }

    #[test]
    fn unterminated_runs_reach_the_end() {
        for t in ["<!-- x", "<?xml", "<a b=\"x", "<a", "<!DOCTYPE x"] {
            let spans = highlight(t);
            check(t, &spans);
            assert_eq!(spans.last().unwrap().end, t.len(), "{t:?}");
        }
    }

    #[test]
    fn a_lt_inside_an_open_tag_starts_a_new_tag() {
        for (t, tags) in [
            ("<<", &[][..]),
            ("<a <b>", &["a", "b"][..]),
            (
                "<type name=\"x\"\n    <nominal>1</nominal>",
                &["type", "nominal", "nominal"][..],
            ),
        ] {
            // A loop that never ends here would hang the suite instead of failing it.
            let (tx, rx) = std::sync::mpsc::channel();
            let text = t.to_string();
            std::thread::spawn(move || {
                let _ = tx.send(highlight(&text));
            });
            let spans = rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("highlight never returned for {t:?}"));
            check(t, &spans);
            assert_eq!(of(t, &spans, XmlToken::Tag), tags, "{t:?}");
        }
    }

    fn big_fixture() -> String {
        let unit = "<?xml version=\"1.0\"?>\r\n<!-- c --><types>\r\n  <type name=\"Ä\"><nominal>12.5</nominal><name>abc</name><user text=\"café &amp; tea\">x &#x2014; y</user></type>\r\n</types>\r\n";
        unit.repeat(13)
    }

    #[test]
    fn every_prefix_is_well_formed() {
        let text = big_fixture();
        assert!(text.len() >= 2048, "fixture too small: {}", text.len());
        for i in 0..=text.len() {
            if text.is_char_boundary(i) {
                check(&text[..i], &highlight(&text[..i]));
            }
        }
    }
}
