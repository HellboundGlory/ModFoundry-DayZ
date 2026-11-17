//! Line/column helpers for byte offsets into config source text.
//!
//! Pure and CRLF-aware: a `\r\n` pair terminates one line, and `line_end`
//! reports the offset before the terminator while `line_range_with_eol`
//! includes it. Columns count characters, not bytes.

use std::ops::Range;

/// `offset` clamped to `text` and backed up to the nearest char boundary.
pub fn clamp_to_char(text: &str, offset: usize) -> usize {
    let mut at = offset.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// 1-based line and character column of byte `offset`.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let at = clamp_to_char(text, offset);
    let line = text[..at].bytes().filter(|b| *b == b'\n').count() + 1;
    let start = line_start(text, at);
    (line, text[start..at].chars().count() + 1)
}

/// Byte offset of the first character on `offset`'s line; a leading byte-order mark is not part of line 1.
pub fn line_start(text: &str, offset: usize) -> usize {
    let at = clamp_to_char(text, offset);
    match text[..at].rfind('\n') {
        Some(i) => i + 1,
        None if at >= bom_len(text) => bom_len(text),
        None => 0,
    }
}

/// The length of a UTF-8 byte-order mark at the start of `text`, or 0.
pub fn bom_len(text: &str) -> usize {
    if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    }
}

/// Byte offset just past the last character on `offset`'s line, before any
/// `\r`/`\n` terminator.
pub fn line_end(text: &str, offset: usize) -> usize {
    let at = clamp_to_char(text, offset);
    text[at..]
        .find(['\r', '\n'])
        .map(|i| at + i)
        .unwrap_or(text.len())
}

/// The line break `text` uses most, `"\r\n"` or `"\n"`.
pub fn dominant_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// `offset`'s whole line, including its `\r\n`, `\n` or `\r` terminator.
pub fn line_range_with_eol(text: &str, offset: usize) -> Range<usize> {
    let start = line_start(text, offset);
    let end = line_end(text, offset);
    let rest = &text[end..];
    let eol = if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with(['\r', '\n']) {
        1
    } else {
        0
    };
    start..end + eol
}

/// The byte offsets of a text's `\n` characters, for repeated line lookups.
pub struct LineIndex(Vec<usize>);

impl LineIndex {
    /// Builds the newline table for `text`.
    pub fn new(text: &str) -> Self {
        Self(text.match_indices('\n').map(|(at, _)| at).collect())
    }

    /// 1-based line of byte `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        self.0.partition_point(|&newline| newline < offset) + 1
    }
}

/// Levenshtein distance between `a` and `b` over bytes, ignoring ASCII case.
pub fn edit_distance_ignore_ascii_case(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, &left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &right) in b.iter().enumerate() {
            let cost = usize::from(!left.eq_ignore_ascii_case(&right));
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Whether `s` is `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_c_identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_ignores_ascii_case() {
        assert_eq!(edit_distance_ignore_ascii_case("Kitten", "sitting"), 3);
        assert_eq!(edit_distance_ignore_ascii_case("ABC", "abc"), 0);
        assert_eq!(edit_distance_ignore_ascii_case("", "ab"), 2);
    }

    #[test]
    fn a_leading_byte_order_mark_is_not_a_column() {
        let text = "\u{feff}ab\ncd";
        assert_eq!(line_col(text, 3), (1, 1));
        assert_eq!(line_col(text, 4), (1, 2));
        assert_eq!(line_start(text, 4), 3);
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_col(text, 6), (2, 1));
    }

    #[test]
    fn crlf_lines() {
        let text = "a\r\nbb\r\nccc";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 4), (2, 2));
        assert_eq!(line_col(text, 5), (2, 3));
        assert_eq!(line_start(text, 5), 3);
        assert_eq!(line_start(text, 6), 3);
        assert_eq!(line_start(text, 7), 7);
        assert_eq!(line_end(text, 4), 5);
        assert_eq!(line_end(text, 5), 5);
        assert_eq!(line_end(text, 6), 6);
        assert_eq!(line_range_with_eol(text, 0), 0..3);
        assert_eq!(line_range_with_eol(text, 5), 3..7);
        assert_eq!(line_range_with_eol(text, 8), 7..10);
    }

    #[test]
    fn lf_and_last_line_without_terminator() {
        let text = "one\ntwo";
        assert_eq!(line_col(text, 4), (2, 1));
        assert_eq!(line_end(text, 0), 3);
        assert_eq!(line_range_with_eol(text, 0), 0..4);
        assert_eq!(line_range_with_eol(text, 4), 4..7);
        assert_eq!(line_col(text, text.len()), (2, 4));
    }

    #[test]
    fn multi_byte_characters_count_as_columns() {
        let text = "αβ\nγ";
        assert_eq!(line_col(text, 2), (1, 2));
        assert_eq!(line_col(text, 4), (1, 3));
        assert_eq!(line_col(text, 5), (2, 1));
        assert_eq!(line_start(text, 6), 5);
        assert_eq!(line_end(text, 0), 4);
        assert_eq!(line_range_with_eol(text, 0), 0..5);
    }

    #[test]
    fn offsets_clamp_to_the_text() {
        let text = "ab";
        assert_eq!(line_col(text, 99), (1, 3));
        assert_eq!(line_start(text, 99), 0);
        assert_eq!(line_end(text, 99), 2);
        assert_eq!(line_range_with_eol(text, 99), 0..2);
    }

    #[test]
    fn offset_inside_a_multibyte_char_clamps_to_its_start() {
        let text = "αb";
        assert_eq!(line_col(text, 1), (1, 1));
        assert_eq!(line_end(text, 1), 3);
    }

    #[test]
    fn line_index_matches_line_col() {
        let text = "αb\ncd\r\nef";
        let lines = LineIndex::new(text);
        for offset in 0..=text.len() {
            if text.is_char_boundary(offset) {
                assert_eq!(
                    lines.line_of(offset),
                    line_col(text, offset).0,
                    "offset {offset}"
                );
            }
        }
    }
}
