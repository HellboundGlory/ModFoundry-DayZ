//! Removes Enforce `//` and `/* */` comments from script source bytes.

/// A block comment with no closing `*/`, and the 1-based line it opens on.
#[derive(Debug, PartialEq, Eq)]
pub struct UnterminatedComment {
    pub line: usize,
}

/// Removes Enforce `//` and `/* */` comments from `source`, keeping every line break.
pub fn strip_comments(source: &[u8]) -> Result<Vec<u8>, UnterminatedComment> {
    let mut out = Vec::with_capacity(source.len());
    let mut i = 0;
    let mut line = 1;
    while i < source.len() {
        match source[i] {
            b'\n' => {
                out.push(b'\n');
                line += 1;
                i += 1;
            }
            b'"' => i = copy_string(source, i, &mut out, &mut line),
            b'\'' => i = copy_char(source, i, &mut out),
            b'/' if source.get(i + 1) == Some(&b'/') => {
                trim_line_end(&mut out);
                i = skip_line_comment(source, i + 2, &mut out);
            }
            b'/' if source.get(i + 1) == Some(&b'*') => {
                i = copy_block_comment(source, i, &mut out, &mut line)?;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn trim_line_end(out: &mut Vec<u8>) {
    while matches!(out.last(), Some(b' ' | b'\t')) {
        out.pop();
    }
}

/// Copies a string from its opening quote to the closing one, or to its line break.
fn copy_string(source: &[u8], start: usize, out: &mut Vec<u8>, line: &mut usize) -> usize {
    out.push(source[start]);
    let mut i = start + 1;
    while i < source.len() {
        match source[i] {
            b'\\' => {
                out.push(b'\\');
                i += 1;
                match source.get(i) {
                    Some(&b'\n') => {
                        out.push(b'\n');
                        *line += 1;
                        i += 1;
                    }
                    Some(&b'\r') if source.get(i + 1) == Some(&b'\n') => {
                        out.extend_from_slice(b"\r\n");
                        *line += 1;
                        i += 2;
                    }
                    Some(&escaped) => {
                        out.push(escaped);
                        i += 1;
                    }
                    None => {}
                }
            }
            b'"' => {
                out.push(b'"');
                return i + 1;
            }
            b'\n' => {
                out.push(b'\n');
                *line += 1;
                return i + 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    i
}

/// Copies a quoted character, or just the apostrophe when no partner follows on its line.
fn copy_char(source: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    let mut i = start + 1;
    while i < source.len() {
        match source[i] {
            b'\n' => break,
            b'\\' if !matches!(source.get(i + 1), Some(b'\n') | None) => i += 1,
            b'\'' => {
                out.extend_from_slice(&source[start..=i]);
                return i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(b'\'');
    start + 1
}

/// Skips a line comment, keeping any carriage return in it, and stops at its newline.
fn skip_line_comment(source: &[u8], mut i: usize, out: &mut Vec<u8>) -> usize {
    while let Some(&byte) = source.get(i) {
        match byte {
            b'\n' => break,
            b'\r' => out.push(byte),
            _ => {}
        }
        i += 1;
    }
    i
}

fn copy_block_comment(
    source: &[u8],
    start: usize,
    out: &mut Vec<u8>,
    line: &mut usize,
) -> Result<usize, UnterminatedComment> {
    let Some(end) = find_comment_end(source, start + 2) else {
        return Err(UnterminatedComment { line: *line });
    };
    if start > 0 && !matches!(source[start - 1], b' ' | b'\t' | b'\n' | b'\r') {
        out.push(b' ');
    }
    keep_line_breaks(&source[start + 2..end - 2], out, line);
    Ok(trim_space_before_line_break(source, end, out))
}

/// Drops the spaces and tabs a removed comment left at the end of its line.
fn trim_space_before_line_break(source: &[u8], end: usize, out: &mut Vec<u8>) -> usize {
    let rest = &source[end..];
    let spaces = rest
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    if rest
        .get(spaces)
        .is_none_or(|byte| matches!(byte, b'\n' | b'\r'))
    {
        trim_line_end(out);
        return end + spaces;
    }
    end
}

fn find_comment_end(source: &[u8], from: usize) -> Option<usize> {
    (from..source.len().saturating_sub(1))
        .find(|&i| source[i] == b'*' && source[i + 1] == b'/')
        .map(|i| i + 2)
}

/// Keeps a removed block comment's line breaks and drops everything else.
fn keep_line_breaks(body: &[u8], out: &mut Vec<u8>, line: &mut usize) {
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'\r' if body.get(i + 1) == Some(&b'\n') => {
                trim_line_end(out);
                out.push(b'\r');
                out.push(b'\n');
                *line += 1;
                i += 1;
            }
            b'\n' => {
                trim_line_end(out);
                out.push(b'\n');
                *line += 1;
            }
            _ => {}
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_each_comment_shape() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"int a; // x\n", b"int a;\n"),
            (b"// x\r\nint a;\r\n", b"\r\nint a;\r\n"),
            (b"int a; // x", b"int a;"),
            (b"int/*c*/b;", b"int b;"),
            (b"a /* c */ b", b"a  b"),
            (b"/* c */", b""),
            (b"x /* one\ntwo */ y\n", b"x\n y\n"),
            (b"/* one\r\ntwo */\r\n", b"\r\n\r\n"),
            (b"\"http://x\"; // c\n", b"\"http://x\";\n"),
            (b"\"a\\\"//b\"", b"\"a\\\"//b\""),
            (b"\"a\\\\\"; // c", b"\"a\\\\\";"),
            (b"\"open // x\nint b; // c\n", b"\"open // x\nint b;\n"),
            (b"\"a\\\nb // c\"; // d\n", b"\"a\\\nb // c\";\n"),
            (b"\"a\\\r\nb // c\"; // d\r\n", b"\"a\\\r\nb // c\";\r\n"),
            (b"'a//b'", b"'a//b'"),
            (b"it's // c\n", b"it's\n"),
            (b"// a \\\nint b;\n", b"\nint b;\n"),
            (b"// /* c\nint a;\n", b"\nint a;\n"),
            (b"/* // */ int a;", b" int a;"),
            (b"a / b; c * d; */", b"a / b; c * d; */"),
            (b"/**/x /***/y", b"x y"),
            (b"// \xE9\n\"\xE9\"\n", b"\n\"\xE9\"\n"),
            (b"\xEF\xBB\xBFint a;\n", b"\xEF\xBB\xBFint a;\n"),
            (b"int a;   \nint b; // c\n", b"int a;   \nint b;\n"),
            (b"x/*c*/  \ny\n", b"x\ny\n"),
            (b"x  /* c\r\n */ y\r\n", b"x\r\n y\r\n"),
            (
                b"int a;  /* one\r\ntwo */  \r\nint b;",
                b"int a;\r\n\r\nint b;",
            ),
            (b"a /* c */ \r\nb", b"a\r\nb"),
            (b"x /*c*/\ny", b"x\ny"),
            (b"x/*c*/  ", b"x"),
            (b"#define X 1 // c\n", b"#define X 1\n"),
        ];
        for (input, expected) in cases {
            let text = String::from_utf8_lossy(input);
            let stripped =
                strip_comments(input).unwrap_or_else(|e| panic!("{text:?}: line {}", e.line));
            assert_eq!(stripped.as_slice(), *expected, "{text:?}");
            let again = strip_comments(&stripped).expect("the output has no comment");
            assert_eq!(again, stripped, "{text:?} is not stripped twice");
        }
    }

    #[test]
    fn an_unclosed_block_comment_names_its_line() {
        assert_eq!(
            strip_comments(b"int a;\n/* open"),
            Err(UnterminatedComment { line: 2 })
        );
        assert_eq!(
            strip_comments(b"/* open"),
            Err(UnterminatedComment { line: 1 })
        );
    }
}
