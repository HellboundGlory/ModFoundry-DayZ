//! Pure syntax highlighting for config.cpp: token kinds mapped to the spans a
//! frontend can paint. No IO; tolerant of partial input.

use crate::token::{tokenize, LexError, Token, TokenKind};

/// Coarse category of a highlighted config region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigToken {
    Keyword,
    ClassName,
    Property,
    Str,
    Number,
    Preproc,
    Comment,
    Punct,
    Plain,
}

/// A half-open byte range `[start, end)` of the source and its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigSpan {
    pub start: usize,
    pub end: usize,
    pub kind: ConfigToken,
}

/// Highlight `text` as sorted, non-overlapping spans that cover only
/// non-whitespace. When lexing fails, everything from the failure on is left
/// unhighlighted (except an unterminated block comment, which runs to EOF).
pub fn highlight(text: &str) -> Vec<ConfigSpan> {
    match tokenize(text) {
        Ok(tokens) => token_spans(text, &tokens),
        Err(error) => {
            let start = match error {
                LexError::UnterminatedString { start }
                | LexError::UnterminatedComment { start } => start,
            };
            let prefix = &text[..start];
            let mut spans =
                tokenize(prefix).map_or_else(|_| Vec::new(), |tokens| token_spans(prefix, &tokens));
            if matches!(error, LexError::UnterminatedComment { .. }) {
                spans.push(trimmed_span(text, start, text.len(), ConfigToken::Comment));
            }
            spans
        }
    }
}

fn token_spans(text: &str, tokens: &[Token]) -> Vec<ConfigSpan> {
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect();
    let mut spans = Vec::new();
    let mut gap = modfoundry_core::text::bom_len(text);
    let mut state = State::default();

    for (index, token) in tokens.iter().enumerate() {
        if token.span.start > gap {
            comment_spans(text, gap, token.span.start, &mut spans);
        }
        gap = token.span.end;
        let next = tokens.get(index + 1).map(|next| &next.kind);
        let kind = state.kind_of(token, next);
        let end = match kind {
            ConfigToken::Preproc => trim_end(text, token.span.start, token.span.end),
            _ => token.span.end,
        };
        spans.push(ConfigSpan {
            start: token.span.start,
            end,
            kind,
        });
        state.advance(token);
    }
    if text.len() > gap {
        comment_spans(text, gap, text.len(), &mut spans);
    }
    spans
}

/// Where the parser-free scanner is in the class nesting it tracks.
#[derive(Default)]
struct State {
    member_start: bool,
    class_name_pending: bool,
    in_bases: bool,
}

impl State {
    fn kind_of(&self, token: &Token, next: Option<&TokenKind>) -> ConfigToken {
        match &token.kind {
            TokenKind::Ident(name) => {
                if matches!(name.as_str(), "class" | "delete" | "external") {
                    ConfigToken::Keyword
                } else if self.class_name_pending || self.in_bases {
                    ConfigToken::ClassName
                } else if self.member_start && starts_a_member(next) {
                    ConfigToken::Property
                } else {
                    ConfigToken::Plain
                }
            }
            TokenKind::Str(_) => ConfigToken::Str,
            TokenKind::Number(_) => ConfigToken::Number,
            TokenKind::Preproc(_) => ConfigToken::Preproc,
            _ => ConfigToken::Punct,
        }
    }

    fn advance(&mut self, token: &Token) {
        match &token.kind {
            TokenKind::Ident(name) if name == "class" => {
                self.class_name_pending = true;
                self.in_bases = false;
            }
            TokenKind::Ident(_) => {
                self.class_name_pending = false;
            }
            TokenKind::LBrace | TokenKind::RBrace | TokenKind::Semicolon => {
                self.class_name_pending = false;
                self.in_bases = false;
                self.member_start = true;
                return;
            }
            TokenKind::Colon => {
                self.class_name_pending = false;
                self.in_bases = true;
            }
            _ => {
                self.class_name_pending = false;
            }
        }
        self.member_start = false;
    }
}

/// A member name is whatever an ident is followed by at the start of a member.
fn starts_a_member(next: Option<&TokenKind>) -> bool {
    matches!(
        next,
        Some(TokenKind::Equals | TokenKind::LBracket | TokenKind::Other('+'))
    )
}

/// Comments live in the whitespace the lexer dropped, so they are read out of
/// the gaps between consecutive token spans.
fn comment_spans(text: &str, start: usize, end: usize, spans: &mut Vec<ConfigSpan>) {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < end {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && i + 1 < end && bytes[i + 1] == b'/' {
            let mut line_end = i + 2;
            while line_end < end && bytes[line_end] != b'\n' {
                line_end += 1;
            }
            spans.push(trimmed_span(text, i, line_end, ConfigToken::Comment));
            i = line_end;
            continue;
        }
        if bytes[i] == b'/' && i + 1 < end && bytes[i + 1] == b'*' {
            let mut close = i + 2;
            while close + 1 < end && !(bytes[close] == b'*' && bytes[close + 1] == b'/') {
                close += 1;
            }
            let comment_end = if close + 1 < end { close + 2 } else { end };
            spans.push(trimmed_span(text, i, comment_end, ConfigToken::Comment));
            i = comment_end;
            continue;
        }
        break;
    }
}

fn trimmed_span(text: &str, start: usize, end: usize, kind: ConfigToken) -> ConfigSpan {
    ConfigSpan {
        start,
        end: trim_end(text, start, end),
        kind,
    }
}

fn trim_end(text: &str, start: usize, mut end: usize) -> usize {
    let bytes = text.as_bytes();
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}
