//! Preprocessor-aware source selection before parsing.

use std::borrow::Cow;
use std::ops::Range;

/// `text` with every arm the preprocessor would drop blanked in place.
pub fn first_arms(text: &str) -> Cow<'_, str> {
    select_arms(text, Arm::First)
}

/// `text` with the arm [`first_arms`] keeps blanked and the other arm kept, in every block.
pub fn other_arms(text: &str) -> Cow<'_, str> {
    select_arms(text, Arm::Other)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    First,
    Other,
}

fn select_arms(text: &str, arm: Arm) -> Cow<'_, str> {
    let mut blanked = Vec::new();
    let mut regions: Vec<Region> = Vec::new();
    for line in lines(text) {
        match line.directive {
            Some(Directive::Open { define, negated }) => {
                regions.push(Region {
                    keeps_then: first_keeps_then(define, negated) == (arm == Arm::First),
                    in_else: false,
                    parent_blanked: is_blanked(&regions),
                });
            }
            Some(Directive::Else) => {
                if let Some(region) = regions.last_mut() {
                    region.in_else = true;
                }
            }
            Some(Directive::Endif) => {
                regions.pop();
            }
            None if is_blanked(&regions) => {
                blanked.push((line.start, line.start + line.body.len()));
            }
            None => {}
        }
    }
    if blanked.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = text.as_bytes().to_vec();
    for (start, end) in blanked {
        for byte in &mut out[start..end] {
            if *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    Cow::Owned(String::from_utf8(out).expect("only ASCII spaces replace bytes"))
}

/// One `#ifdef` / `#ifndef` block, by the 1-based lines of its directives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub define: String,
    pub negated: bool,
    pub open_line: usize,
    pub else_line: Option<usize>,
    pub end_line: usize,
}

impl Guard {
    /// Whether [`first_arms`] keeps this block's `#ifdef` arm rather than its `#else` arm.
    pub fn first_keeps_then(&self) -> bool {
        first_keeps_then(&self.define, self.negated)
    }
}

fn first_keeps_then(define: &str, negated: bool) -> bool {
    negated || define != "DOXYGEN"
}

/// Every preprocessor block in `text`, in the order they open.
pub fn guards(text: &str) -> Vec<Guard> {
    let mut found: Vec<Guard> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut last_line = 0;
    for line in lines(text) {
        last_line = line.number;
        match line.directive {
            Some(Directive::Open { define, negated }) => {
                open.push(found.len());
                found.push(Guard {
                    define: define.to_string(),
                    negated,
                    open_line: line.number,
                    else_line: None,
                    end_line: 0,
                });
            }
            Some(Directive::Else) => {
                if let Some(&index) = open.last() {
                    found[index].else_line.get_or_insert(line.number);
                }
            }
            Some(Directive::Endif) => {
                if let Some(index) = open.pop() {
                    found[index].end_line = line.number;
                }
            }
            None => {}
        }
    }
    for index in open {
        found[index].end_line = last_line + 1;
    }
    found
}

const MAX_NESTED_VIEWS: usize = 16;

/// Views that between them keep the arms neither [`first_arms`] nor [`other_arms`] keeps, at most 16.
pub fn nested_arm_views(text: &str) -> Vec<String> {
    let blocks = guards(text);
    if blocks.is_empty() {
        return Vec::new();
    }
    let spans: Vec<(usize, usize)> = lines(text)
        .map(|line| (line.start, line.body.len()))
        .collect();
    let parent: Vec<Option<usize>> = (0..blocks.len())
        .map(|index| {
            (0..index)
                .rev()
                .find(|&earlier| blocks[earlier].end_line > blocks[index].open_line)
        })
        .collect();
    let first: Vec<bool> = blocks.iter().map(Guard::first_keeps_then).collect();
    let other: Vec<bool> = first.iter().map(|keeps| !keeps).collect();
    let mut extra: Vec<Vec<bool>> = Vec::new();
    'arms: for (index, guard) in blocks.iter().enumerate() {
        for then in [true, false] {
            if !then && guard.else_line.is_none() {
                continue;
            }
            if !arm_has_content(text, &spans, guard, then) {
                continue;
            }
            if extra.len() >= MAX_NESTED_VIEWS {
                break 'arms;
            }
            let known = [&first, &other]
                .into_iter()
                .chain(&extra)
                .any(|keeps| arm_kept(keeps, &parent, &blocks, index, then));
            if known {
                continue;
            }
            let mut keeps = first.clone();
            keeps[index] = then;
            let mut child = index;
            while let Some(ancestor) = parent[child] {
                keeps[ancestor] = parent_arm(&blocks[ancestor], &blocks[child]);
                child = ancestor;
            }
            extra.push(keeps);
        }
    }
    extra
        .iter()
        .map(|keeps| nested_view(text, &spans, &blocks, &parent, keeps))
        .collect()
}

/// Whether `child` sits in `guard`'s then arm rather than its else arm.
fn parent_arm(guard: &Guard, child: &Guard) -> bool {
    guard
        .else_line
        .is_none_or(|else_line| child.open_line < else_line)
}

fn arm_kept(
    keeps: &[bool],
    parent: &[Option<usize>],
    blocks: &[Guard],
    index: usize,
    then: bool,
) -> bool {
    let mut current = index;
    let mut arm = then;
    loop {
        if keeps[current] != arm {
            return false;
        }
        let Some(ancestor) = parent[current] else {
            return true;
        };
        arm = parent_arm(&blocks[ancestor], &blocks[current]);
        current = ancestor;
    }
}

fn arm_has_content(text: &str, spans: &[(usize, usize)], guard: &Guard, then: bool) -> bool {
    arm_bounds(guard, then).any(|line| {
        let Some(&(start, len)) = spans.get(line - 1) else {
            return false;
        };
        let body = text[start..start + len].trim_end_matches('\r');
        body.chars().any(|c| c != ' ' && c != '\t')
    })
}

fn nested_view(
    text: &str,
    spans: &[(usize, usize)],
    blocks: &[Guard],
    parent: &[Option<usize>],
    keeps: &[bool],
) -> String {
    let mut out = text.as_bytes().to_vec();
    for (index, guard) in blocks.iter().enumerate() {
        blank_line(&mut out, spans, guard.open_line);
        if let Some(else_line) = guard.else_line {
            blank_line(&mut out, spans, else_line);
        }
        blank_line(&mut out, spans, guard.end_line);
        for then in [true, false] {
            if arm_kept(keeps, parent, blocks, index, then) {
                continue;
            }
            for line in arm_bounds(guard, then) {
                blank_line(&mut out, spans, line);
            }
        }
    }
    String::from_utf8(out).expect("only ASCII spaces replace bytes")
}

/// The 1-based lines strictly inside an arm.
fn arm_bounds(guard: &Guard, then: bool) -> Range<usize> {
    if then {
        guard.open_line + 1..guard.else_line.unwrap_or(guard.end_line)
    } else {
        let else_line = guard.else_line.unwrap_or(guard.end_line);
        else_line + 1..guard.end_line
    }
}

fn blank_line(out: &mut [u8], spans: &[(usize, usize)], line: usize) {
    let Some(&(start, len)) = line.checked_sub(1).and_then(|index| spans.get(index)) else {
        return;
    };
    for byte in &mut out[start..start + len] {
        if *byte != b'\r' {
            *byte = b' ';
        }
    }
}

struct Line<'a> {
    number: usize,
    start: usize,
    body: &'a str,
    directive: Option<Directive<'a>>,
}

fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut in_comment = false;
    let mut offset = 0;
    text.split_inclusive('\n')
        .enumerate()
        .map(move |(index, line)| {
            let body = line.strip_suffix('\n').unwrap_or(line);
            let starts_in_comment = in_comment;
            in_comment = comments_open(body, in_comment);
            let directive = if starts_in_comment {
                None
            } else {
                parse_directive(body)
            };
            let start = offset;
            offset += line.len();
            Line {
                number: index + 1,
                start,
                body,
                directive,
            }
        })
}

enum Directive<'a> {
    Open { define: &'a str, negated: bool },
    Else,
    Endif,
}

struct Region {
    keeps_then: bool,
    in_else: bool,
    parent_blanked: bool,
}

fn is_blanked(regions: &[Region]) -> bool {
    regions
        .last()
        .is_some_and(|region| region.parent_blanked || region.in_else == region.keeps_then)
}

fn parse_directive(body: &str) -> Option<Directive<'_>> {
    let line = body.trim_start_matches('\u{feff}').trim_start();
    if let Some(rest) = after(line, "#ifdef") {
        return Some(Directive::Open {
            define: define_name(rest),
            negated: false,
        });
    }
    if let Some(rest) = after(line, "#ifndef") {
        return Some(Directive::Open {
            define: define_name(rest),
            negated: true,
        });
    }
    if after(line, "#else").is_some() {
        return Some(Directive::Else);
    }
    after(line, "#endif").map(|_| Directive::Endif)
}

fn define_name(rest: &str) -> &str {
    rest.trim_start()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or_default()
}

fn after<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    let boundary = rest.is_empty()
        || rest.starts_with(char::is_whitespace)
        || rest.starts_with("//")
        || rest.starts_with("/*");
    boundary.then_some(rest)
}

fn comments_open(line: &str, mut in_comment: bool) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if in_comment {
            if bytes[i..].starts_with(b"*/") {
                in_comment = false;
                i += 2;
            } else {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"//") {
            break;
        } else if bytes[i..].starts_with(b"/*") {
            in_comment = true;
            i += 2;
        } else if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    in_comment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_each_view_and_keeps_every_offset() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "#ifdef X\nint a;\n#else\nint b;\n#endif\n",
                "#ifdef X\nint a;\n#else\n      \n#endif\n",
                "#ifdef X\n      \n#else\nint b;\n#endif\n",
            ),
            (
                "#ifndef X\nint a;\n#else\nint b;\n#endif\n",
                "#ifndef X\nint a;\n#else\n      \n#endif\n",
                "#ifndef X\n      \n#else\nint b;\n#endif\n",
            ),
            (
                "#ifdef A\n#ifdef B\nint a;\n#else\nint b;\n#endif\n#else\nint c;\n#endif\n",
                "#ifdef A\n#ifdef B\nint a;\n#else\n      \n#endif\n#else\n      \n#endif\n",
                "#ifdef A\n#ifdef B\n      \n#else\n      \n#endif\n#else\nint c;\n#endif\n",
            ),
            (
                "#ifdef A\nint a;\n#else\n#ifdef B\nint b;\n#else\nint c;\n#endif\nint d;\n#endif\n",
                "#ifdef A\nint a;\n#else\n#ifdef B\n      \n#else\n      \n#endif\n      \n#endif\n",
                "#ifdef A\n      \n#else\n#ifdef B\n      \n#else\nint c;\n#endif\nint d;\n#endif\n",
            ),
            (
                "#ifdef DOXYGEN\nint a;\n#else\nint b;\n#endif\n",
                "#ifdef DOXYGEN\n      \n#else\nint b;\n#endif\n",
                "#ifdef DOXYGEN\nint a;\n#else\n      \n#endif\n",
            ),
            (
                "#ifdef DOXYGEN\nint a;\nint b;\n#endif\n",
                "#ifdef DOXYGEN\n      \n      \n#endif\n",
                "#ifdef DOXYGEN\nint a;\nint b;\n#endif\n",
            ),
            (
                "#ifdef DOXYGEN /* docs */\nproto int Foo;\n#else\nint b;\n#endif\n",
                "#ifdef DOXYGEN /* docs */\n              \n#else\nint b;\n#endif\n",
                "#ifdef DOXYGEN /* docs */\nproto int Foo;\n#else\n      \n#endif\n",
            ),
            (
                "#ifdef X\nint a;\n#else\nint b;\n#endif//end\nint c;\n",
                "#ifdef X\nint a;\n#else\n      \n#endif//end\nint c;\n",
                "#ifdef X\n      \n#else\nint b;\n#endif//end\nint c;\n",
            ),
            (
                "#ifdef X\nint a;\n#else\nint b;\nint c;\n",
                "#ifdef X\nint a;\n#else\n      \n      \n",
                "#ifdef X\n      \n#else\nint b;\nint c;\n",
            ),
            (
                "#ifdef X\r\nint a;\r\n#else\r\nint b;\r\n#endif\r\n",
                "#ifdef X\r\nint a;\r\n#else\r\n      \r\n#endif\r\n",
                "#ifdef X\r\n      \r\n#else\r\nint b;\r\n#endif\r\n",
            ),
            (
                "#ifdef X\nint a;\n#else\n// caf\u{e9}\n#endif\n",
                "#ifdef X\nint a;\n#else\n        \n#endif\n",
                "#ifdef X\n      \n#else\n// caf\u{e9}\n#endif\n",
            ),
            (
                "\u{feff}#ifdef X\nint a;\n#else\nint b;\n#endif\n",
                "\u{feff}#ifdef X\nint a;\n#else\n      \n#endif\n",
                "\u{feff}#ifdef X\n      \n#else\nint b;\n#endif\n",
            ),
            (
                "int a;\n#endif\nint b;\n",
                "int a;\n#endif\nint b;\n",
                "int a;\n#endif\nint b;\n",
            ),
            (
                "#ifdef X\nint a;\n/*\n#else\n*/\nint b;\n#endif\n",
                "#ifdef X\nint a;\n/*\n#else\n*/\nint b;\n#endif\n",
                "#ifdef X\n      \n  \n     \n  \n      \n#endif\n",
            ),
            (
                "string s = \"/*\";\n#ifdef X\nint a;\n#else\nint b;\n#endif\n",
                "string s = \"/*\";\n#ifdef X\nint a;\n#else\n      \n#endif\n",
                "string s = \"/*\";\n#ifdef X\n      \n#else\nint b;\n#endif\n",
            ),
            (
                "class A {}\n",
                "class A {}\n",
                "class A {}\n",
            ),
        ];
        for &(input, first, other) in cases {
            for (out, expected) in [(first_arms(input), first), (other_arms(input), other)] {
                assert_eq!(out.as_ref(), expected, "{input:?}");
                assert_eq!(out.len(), input.len(), "{input:?}");
                assert_eq!(
                    matches!(out, Cow::Borrowed(_)),
                    expected == input,
                    "{input:?}"
                );
            }
        }
    }

    #[test]
    fn nested_arm_views_read_each_arm_neither_basic_view_keeps() {
        let cases: &[(&str, &[&str])] = &[
            ("#ifdef X\nint a;\n#else\nint b;\n#endif\n", &[]),
            (
                "#ifdef X\n#ifdef Y\nint a;\n#else\nint b;\n#endif\n#else\nint c;\n#endif\n",
                &["        \n        \n      \n     \nint b;\n      \n     \n      \n      \n"],
            ),
            ("class A\n{\n}\n", &[]),
        ];
        for &(input, expected) in cases {
            assert_eq!(nested_arm_views(input), expected, "{input:?}");
        }
    }

    fn guard(
        define: &str,
        negated: bool,
        open_line: usize,
        else_line: Option<usize>,
        end_line: usize,
    ) -> Guard {
        Guard {
            define: define.to_string(),
            negated,
            open_line,
            else_line,
            end_line,
        }
    }

    #[test]
    fn guards_lists_each_block_by_its_directive_lines() {
        let cases = [
            (
                "#ifdef A\n#ifdef B\nx\n#endif\n#endif\n",
                vec![guard("A", false, 1, None, 5), guard("B", false, 2, None, 4)],
            ),
            (
                "#ifndef X\na\n#else\nb\n#endif\n",
                vec![guard("X", true, 1, Some(3), 5)],
            ),
            ("#ifdef X\na\n", vec![guard("X", false, 1, None, 3)]),
            ("/*\n#ifdef X\n#endif\n*/\n", vec![]),
            (
                "#endif\n#else\n#ifdef X\n#endif\n",
                vec![guard("X", false, 3, None, 4)],
            ),
            (
                "#ifdef X // note\na\n#endif\n",
                vec![guard("X", false, 1, None, 3)],
            ),
            (
                "#ifdef X\r\na\r\n#else\r\nb\r\n#endif\r\n",
                vec![guard("X", false, 1, Some(3), 5)],
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(guards(input), expected, "{input:?}");
        }
    }
}
