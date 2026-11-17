//! Tokens and the lexer for the config.cpp C-like superset.
//!
//! The lexer is hand-rolled and deliberately tolerant: config.cpp is close
//! enough to C that a generic scanner covers real files (vanilla/workshop/CF),
//! but strict enough that structural errors (unbalanced `{}`, an unterminated
//! string) are surfaced with byte spans for `MF-C1xx` diagnostics.

use modfoundry_core::Span;

/// A lexical token with its byte span into the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// `class`, `delete`, `external` are parsed by context; all bare words land
    /// here (we do not keep a keyword table — config.cpp namespaces are data).
    Ident(String),
    /// Double-quoted string with escapes decoded.
    Str(String),
    /// Integer or float literal, kept verbatim.
    Number(String),
    /// A preprocessor line: starts with `#` and runs to end of line. The lexer
    /// records the *content* (without `#`) so `mf validate` can reason about
    /// `#include`/`#define` presence without evaluating them (v1 does not
    /// evaluate conditionals — both branches are parsed conservatively).
    Preproc(String),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Equals,
    Semicolon,
    Colon,
    Comma,
    /// Any other single character not otherwise classified (kept so the parser
    /// can recover rather than abort on an unfamiliar byte).
    Other(char),
    Eof,
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(s) => format!("identifier `{s}`"),
            TokenKind::Str(_) => "string".to_string(),
            TokenKind::Number(n) => format!("number `{n}`"),
            TokenKind::Preproc(s) => format!("preprocessor `#{s}`"),
            TokenKind::LBrace => "`{`".to_string(),
            TokenKind::RBrace => "`}`".to_string(),
            TokenKind::LBracket => "`[`".to_string(),
            TokenKind::RBracket => "`]`".to_string(),
            TokenKind::Equals => "`=`".to_string(),
            TokenKind::Semicolon => "`;`".to_string(),
            TokenKind::Colon => "`:`".to_string(),
            TokenKind::Comma => "`,`".to_string(),
            TokenKind::Other(c) => format!("character `{c}`"),
            TokenKind::Eof => "end of file".to_string(),
        }
    }
}

/// Errors produced by the lexer. All are syntax-level with a span.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LexError {
    #[error("unterminated string starting at byte {start}")]
    UnterminatedString { start: usize },
    #[error("unterminated block comment starting at byte {start}")]
    UnterminatedComment { start: usize },
}

/// Whether a digit-leading token is an actual number literal. CfgConvert writes
/// floats as `1e-05`-style text; anything else that starts with a digit is a
/// name, not a number.
fn is_number_lexeme(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() >= 3 && b[0] == b'0' && (b[1] | 0x20) == b'x' {
        return b[2..].iter().all(|c| c.is_ascii_hexdigit() || *c == b'_');
    }
    let mut i = 0;
    if b.first() == Some(&b'.') {
        i = 1;
    } else if !b.first().is_some_and(u8::is_ascii_digit) {
        return false;
    }
    let integer_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i == integer_start && b.first() != Some(&b'.') {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    if i < b.len() && matches!(b[i] | 0x20, b'e' | b'd') {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let exponent_start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
        if i == exponent_start {
            return false;
        }
    }
    i == b.len()
}

/// Decode the string literal at `*i`, advancing past its closing quote.
/// `None` when `*i` is not a `"` or the string is unterminated.
pub(crate) fn read_string(src: &str, i: &mut usize) -> Option<String> {
    let b = src.as_bytes();
    if b.get(*i) != Some(&b'"') {
        return None;
    }
    *i += 1;
    let mut out = String::new();
    while *i < b.len() {
        let c = b[*i];
        if c == b'"' {
            if b.get(*i + 1) == Some(&b'"') {
                out.push('"');
                *i += 2;
                continue;
            }
            *i += 1;
            return Some(out);
        }
        if c.is_ascii() {
            out.push(c as char);
            *i += 1;
        } else {
            let ch = src[*i..].chars().next().expect("index at a char boundary");
            out.push(ch);
            *i += ch.len_utf8();
        }
    }
    None
}

/// Tokenize `src`. Returns tokens (excluding EOF) plus a trailing `Eof` token,
/// or the first lexing error. Spans reference `src`.
pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    // Windows editors save a leading byte-order mark; skip it here so spans stay offsets into `src`.
    let mut i = modfoundry_core::text::bom_len(src);
    let mut toks = Vec::new();

    while i < n {
        let c = if bytes[i].is_ascii() {
            bytes[i] as char
        } else {
            src[i..].chars().next().expect("index at a char boundary")
        };
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if c == '/' && i + 1 < n && bytes[i + 1] == b'/' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            let mut closed = false;
            while i + 1 < n {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(LexError::UnterminatedComment { start });
            }
            continue;
        }
        // String literal. Backslashes are literal (paths depend on it) and a
        // quote inside a string is written double (`""`), as CfgConvert's own
        // `-txt` output does; `\"` is not an escape.
        if c == '"' {
            let start = i;
            let Some(s) = read_string(src, &mut i) else {
                return Err(LexError::UnterminatedString { start });
            };
            toks.push(Token {
                kind: TokenKind::Str(s),
                span: Span::new(start, i),
            });
            continue;
        }
        // Preprocessor line: `#` at start of a logical position (we accept it
        // after any whitespace — conservative). Runs to end of line.
        if c == '#' {
            let start = i;
            i += 1;
            while i < n && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            let content_start = i;
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            toks.push(Token {
                kind: TokenKind::Preproc(src[content_start..i].to_string()),
                span: Span::new(start, i),
            });
            continue;
        }
        // Identifier / number-like word. Config identifiers: [A-Za-z_][A-Za-z0-9_.]*
        // Path-like tokens in arrays are strings, so no need to lex `/` here.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < n {
                let b = bytes[i];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
                    i += 1;
                } else {
                    break;
                }
            }
            toks.push(Token {
                kind: TokenKind::Ident(src[start..i].to_string()),
                span: Span::new(start, i),
            });
            continue;
        }
        // A number, or a name that merely starts with a digit.
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && (bytes[i + 1] as char).is_ascii_digit())
        {
            let start = i;
            while i < n {
                let b = bytes[i];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-' || b == b'+' {
                    // Only keep sign if it's an exponent (e.g. 1e-5); do not let a
                    // lone '-' swallow an operator. Simple approach: allow '-'/'+'
                    // only right after [eEdD].
                    let prev = if i > start { bytes[i - 1] } else { 0 };
                    if (b == b'-' || b == b'+') && !matches!(prev, b'e' | b'E' | b'd' | b'D') {
                        break;
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            let text = &src[start..i];
            let kind = if is_number_lexeme(text) {
                TokenKind::Number(text.to_string())
            } else {
                TokenKind::Ident(text.to_string())
            };
            toks.push(Token {
                kind,
                span: Span::new(start, i),
            });
            continue;
        }
        let single = match c {
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            '=' => TokenKind::Equals,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            _ => TokenKind::Other(c),
        };
        toks.push(Token {
            kind: single,
            span: Span::new(i, i + c.len_utf8()),
        });
        i += c.len_utf8();
    }

    toks.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(n, n),
    });
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_class_and_property() {
        let k = kinds(r#"class CfgPatches { units[] = { }; };"#);
        assert_eq!(
            k,
            vec![
                TokenKind::Ident("class".into()),
                TokenKind::Ident("CfgPatches".into()),
                TokenKind::LBrace,
                TokenKind::Ident("units".into()),
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Equals,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Semicolon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn strips_comments() {
        let k = kinds("a = 1; // line\n/* block */ b = 2;");
        // No stray '/' from either comment form, and both idents survived.
        assert!(!k.iter().any(|t| matches!(t, TokenKind::Other('/'))));
        assert!(k.contains(&TokenKind::Ident("a".into())));
        assert!(k.contains(&TokenKind::Ident("b".into())));
    }

    #[test]
    fn strings_keep_backslashes_verbatim() {
        // Measured: `CfgConvert.exe -bin` stores these bytes unchanged, so a
        // `\t` in a path must not become a tab.
        let k = kinds(r#"x = "DZ\characters\tops\a.rvmat";"#);
        match &k[2] {
            TokenKind::Str(s) => assert_eq!(s, r"DZ\characters\tops\a.rvmat"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn strings_decode_utf8() {
        let k = kinds(r#"x = "Защитный контейнер";"#);
        match &k[2] {
            TokenKind::Str(s) => assert_eq!(s, "Защитный контейнер"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn preproc_is_captured() {
        let k = kinds("#define FOO 1\nclass A {}");
        assert!(matches!(&k[0], TokenKind::Preproc(c) if c == "define FOO 1"));
    }

    #[test]
    fn doubled_quote_decodes_to_one_quote() {
        let toks = tokenize(r#"x = "a""b";"#).unwrap();
        match &toks[2].kind {
            TokenKind::Str(s) => assert_eq!(s, "a\"b"),
            other => panic!("expected Str, got {other:?}"),
        }
        assert_eq!(toks[2].span, Span::new(4, 10));
    }

    #[test]
    fn empty_string_is_not_an_escape() {
        let toks = tokenize(r#"a = ""; b = "x";"#).unwrap();
        assert_eq!(toks[2].kind, TokenKind::Str(String::new()));
        assert_eq!(toks[2].span, Span::new(4, 6));
    }

    #[test]
    fn adjacent_strings_stay_separate() {
        let k = kinds(r#"{"a","b"}"#);
        assert_eq!(
            k,
            vec![
                TokenKind::LBrace,
                TokenKind::Str("a".into()),
                TokenKind::Comma,
                TokenKind::Str("b".into()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
        let k = kinds(r#"{"",""}"#);
        assert_eq!(
            k,
            vec![
                TokenKind::LBrace,
                TokenKind::Str(String::new()),
                TokenKind::Comma,
                TokenKind::Str(String::new()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn string_ending_in_escaped_quote() {
        let toks = tokenize(r#""x""""#).unwrap();
        match &toks[0].kind {
            TokenKind::Str(s) => assert_eq!(s, "x\""),
            other => panic!("expected Str, got {other:?}"),
        }
        assert_eq!(toks[0].span, Span::new(0, 5));
    }

    #[test]
    fn unterminated_string_after_escape_errors() {
        assert!(matches!(
            tokenize(r#""x"""#),
            Err(LexError::UnterminatedString { .. })
        ));
    }

    #[test]
    fn digit_leading_words_split_into_ident_or_number() {
        let k = kinds(r#"1kHz_x 1 1.5 .5 1e5 0x1F 1_000 0"#);
        assert_eq!(
            k,
            vec![
                TokenKind::Ident("1kHz_x".into()),
                TokenKind::Number("1".into()),
                TokenKind::Number("1.5".into()),
                TokenKind::Number(".5".into()),
                TokenKind::Number("1e5".into()),
                TokenKind::Number("0x1F".into()),
                TokenKind::Number("1_000".into()),
                TokenKind::Number("0".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unterminated_string_errors() {
        assert!(matches!(
            tokenize(r#"x = "oops;"#),
            Err(LexError::UnterminatedString { .. })
        ));
    }

    #[test]
    fn unterminated_block_comment_errors() {
        assert!(matches!(
            tokenize("/* oops"),
            Err(LexError::UnterminatedComment { .. })
        ));
    }
}
