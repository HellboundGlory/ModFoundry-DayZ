//! Recursive-descent parser over the config.cpp token stream.
//!
//! Grammar (C-like superset, no expression evaluation in v1):
//!
//! ```text
//! file     := member*
//! member   := class_def | property | delete_stmt
//! class_def:= 'class' IDENT [ ':' IDENT (',' IDENT)* ] '{' member* '}'
//! property := IDENT ['[' ']'] '=' value ';'
//! delete   := 'delete' IDENT ';'
//! value    := scalar | array
//! array    := '{' [ value (',' value)* [','] ] '}'
//! scalar   := Str | Number | Ident        (a clean single token)
//!           | <raw tail>                  (unclassified expression, tolerated)
//! ```
//!
//! Preprocessor lines are legal between members; the parser skips them (v1 does
//! not evaluate `#if`/`#ifdef` — both branches are parsed conservatively, which
//! is safe for structural diagnostics because only one branch ever type-checks
//! anyway). Recovery: a malformed member is reported and skipped to the next `;`
//! or `}`, so later, possibly-independent findings still surface.

use crate::ast::{
    ClassSpans, ConfigClass, ConfigFile, ConfigProperty, Member, PropertySpans, Value,
};
use crate::rap::MAX_NESTING;
use crate::token::{tokenize, LexError, Token, TokenKind};
use modfoundry_core::{Diagnostic, Severity, Span};

/// A successfully-recognized parse issue. Distinguish parse-level syntax
/// failures from lexer errors so callers can tier severity if desired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    Lex,
    Syntax,
}

impl ParseErrorKind {
    /// `MF-C1xx` range is syntax; lex errors get their own stable prefix too but
    /// we keep them in the same band for config.cpp (see validator for meaning).
    pub fn code(self) -> &'static str {
        match self {
            ParseErrorKind::Lex => "MF-C100",
            ParseErrorKind::Syntax => "MF-C101",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParseIssue {
    pub kind: ParseErrorKind,
    pub message: String,
    pub span: Span,
}

/// Result of parsing: the tree (best-effort) plus any issues encountered.
/// The tree is never `None` — even a fully-broken file yields an empty one so
/// callers have a uniform shape to check.
#[derive(Debug)]
pub struct ParseResult {
    pub file: ConfigFile,
    pub issues: Vec<ParseIssue>,
}

impl ParseResult {
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.issues
            .iter()
            .map(|i| Diagnostic::at(i.kind.code(), Severity::Error, i.span, &i.message))
            .collect()
    }
}

struct Parser<'a> {
    text: &'a str,
    toks: &'a [Token],
    pos: usize,
    class_depth: usize,
    array_depth: usize,
    issues: Vec<ParseIssue>,
}

/// Byte extents of a parsed value, plus its array structure.
struct ValueSpans {
    span: Span,
    items: Vec<Span>,
    braces: Option<(Span, Span)>,
}

impl ValueSpans {
    fn scalar(span: Span) -> Self {
        Self {
            span,
            items: Vec::new(),
            braces: None,
        }
    }
}

fn is_include(token: &Token) -> bool {
    matches!(&token.kind, TokenKind::Preproc(s) if s.trim_start().starts_with("include"))
}

/// The value a string, number or identifier token stands for.
fn scalar(kind: &TokenKind) -> Option<Value> {
    match kind {
        TokenKind::Str(s) => Some(Value::Str(s.clone())),
        TokenKind::Number(n) => Some(Value::Number(n.clone())),
        TokenKind::Ident(s) => Some(Value::Ident(s.clone())),
        _ => None,
    }
}

pub fn parse(text: &str) -> ParseResult {
    let toks = match tokenize(text) {
        Ok(t) => t,
        Err(e) => {
            let (msg, start) = match e {
                LexError::UnterminatedString { start } => ("unterminated string literal", start),
                LexError::UnterminatedComment { start } => ("unterminated block comment", start),
            };
            return ParseResult {
                file: ConfigFile {
                    members: vec![],
                    ..Default::default()
                },
                issues: vec![ParseIssue {
                    kind: ParseErrorKind::Lex,
                    message: msg.into(),
                    span: Span::new(start, start),
                }],
            };
        }
    };

    let has_include = toks.iter().any(is_include);

    let mut p = Parser {
        text,
        toks: &toks,
        pos: 0,
        class_depth: 0,
        array_depth: 0,
        issues: vec![],
    };
    let members = p.parse_members(/*in_class=*/ false);
    // After the top-level loop there must only be EOF left; if a stray `}` or
    // junk remains it is reported by parse_members already.
    ParseResult {
        file: ConfigFile {
            members,
            has_include,
            ..Default::default()
        },
        issues: p.issues,
    }
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn bump(&mut self) -> &'a Token {
        let toks = self.toks;
        let i = self.pos.min(toks.len() - 1);
        if self.pos < toks.len() - 1 {
            self.pos += 1;
        }
        &toks[i]
    }
    fn expected(&mut self, what: impl std::fmt::Display) {
        let msg = format!("expected {what}, found {}", self.peek().kind.describe());
        self.error(self.peek().span, msg);
    }
    fn at(&self, k: &TokenKind) -> bool {
        &self.peek().kind == k
    }
    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.issues.push(ParseIssue {
            kind: ParseErrorKind::Syntax,
            message: msg.into(),
            span,
        });
    }

    /// Parse members until the enclosing `}` or (at top level) EOF. Returns
    /// members in source order. When `in_class` is false the loop runs to EOF;
    /// otherwise it stops at the class's closing `}`.
    fn parse_members(&mut self, in_class: bool) -> Vec<Member> {
        let mut out = Vec::new();
        loop {
            if self.at(&TokenKind::Eof) {
                if in_class {
                    self.error(
                        self.peek().span,
                        "unexpected end of file inside class body; missing `}`",
                    );
                }
                break;
            }
            if self.at(&TokenKind::RBrace) {
                if in_class {
                    self.bump(); // consume `}`
                    break;
                }
                // Recovery stops on `}`, so a stray top-level one has to be
                // consumed here or it is re-parsed forever.
                self.error(self.peek().span, "unexpected `}` at top level");
                self.bump();
                continue;
            }
            // Empty statements: config.cpp idiom `};` after a class close leaves
            // a stray `;`. Tolerate at any member boundary.
            if self.at(&TokenKind::Semicolon) {
                self.bump();
                continue;
            }
            if let TokenKind::Preproc(_) = self.peek().kind {
                self.bump();
                continue;
            }
            match self.parse_member() {
                Some(m) => out.push(m),
                None => {
                    // Recovery: advance to the next `;`, `}`, or EOF.
                    loop {
                        if self.at(&TokenKind::Semicolon) {
                            self.bump();
                            break;
                        }
                        if self.at(&TokenKind::RBrace) || self.at(&TokenKind::Eof) {
                            break;
                        }
                        self.bump();
                    }
                }
            }
        }
        out
    }

    /// Attempt one member. Returns `None` on a syntax error (after reporting it).
    fn parse_member(&mut self) -> Option<Member> {
        let t = self.peek();
        match &t.kind {
            TokenKind::Ident(word) => {
                let kw = word.as_str();
                match kw {
                    "class" => self.parse_class(),
                    "delete" => self.parse_delete(),
                    "external" => {
                        // `external class X : Y;` / `external` prefix — skip the
                        // keyword and parse the following class/member.
                        let external = self.bump().span;
                        match self.parse_member() {
                            Some(Member::Class(mut c)) => {
                                c.spans.statement =
                                    Span::new(external.start, c.spans.statement.end);
                                Some(Member::Class(c))
                            }
                            other => other,
                        }
                    }
                    _ => self.parse_property(),
                }
            }
            TokenKind::Str(_) => {
                // Some configs quote property names; treat as a property name.
                self.parse_property()
            }
            _ => {
                self.error(
                    t.span,
                    format!(
                        "unexpected {}; expected `class`, `delete`, or a property name",
                        t.kind.describe()
                    ),
                );
                None
            }
        }
    }

    fn parse_class(&mut self) -> Option<Member> {
        let kw_span = self.bump().span; // 'class'
        let name_tok = self.bump();
        let name = match &name_tok.kind {
            TokenKind::Ident(s) | TokenKind::Str(s) => s.clone(),
            _ => {
                self.error(
                    name_tok.span,
                    format!(
                        "expected class name after `class`, found {}",
                        name_tok.kind.describe()
                    ),
                );
                return None;
            }
        };
        let name_span = name_tok.span;

        let mut parents = Vec::new();
        let mut parents_span: Option<Span> = None;
        if self.at(&TokenKind::Colon) {
            let colon = self.bump().span;
            let mut last_end = colon.end;
            loop {
                let p = self.bump();
                match &p.kind {
                    TokenKind::Ident(s) | TokenKind::Str(s) => {
                        parents.push(s.clone());
                        last_end = p.span.end;
                    }
                    _ => {
                        self.error(
                            p.span,
                            format!(
                                "expected parent class name after `:`, found {}",
                                p.kind.describe()
                            ),
                        );
                        break;
                    }
                }
                if self.at(&TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
            if !parents.is_empty() {
                parents_span = Some(Span::new(colon.start, last_end));
            }
        }

        if !self.at(&TokenKind::LBrace) {
            // `class Name;` — a forward declaration with no body.
            if self.at(&TokenKind::Semicolon) {
                let end = self.bump().span.end;
                return Some(Member::Class(ConfigClass {
                    name,
                    span: Span::new(kw_span.start, end),
                    spans: ClassSpans {
                        keyword: kw_span,
                        name: name_span,
                        parents: parents_span,
                        open_brace: None,
                        close_brace: None,
                        statement: Span::new(kw_span.start, end),
                    },
                    parents,
                    forward: true,
                    members: Vec::new(),
                }));
            }
            self.expected(format!("`{{` to open class `{name}`"));
            return None;
        }
        let open_brace = self.bump().span; // '{'
        let members = if self.class_depth >= MAX_NESTING {
            self.error(
                open_brace,
                format!("class body nested deeper than {MAX_NESTING} levels"),
            );
            self.skip_nested();
            Vec::new()
        } else {
            self.class_depth += 1;
            let members = self.parse_members(/*in_class=*/ true);
            self.class_depth -= 1;
            members
        };
        let end = if self.pos > 0 {
            self.toks[self.pos - 1].span.end
        } else {
            name_span.end
        };
        let close_brace = if self.pos > 0 && self.toks[self.pos - 1].kind == TokenKind::RBrace {
            Some(self.toks[self.pos - 1].span)
        } else {
            None
        };
        let statement_end = if self.at(&TokenKind::Semicolon) {
            self.peek().span.end
        } else {
            end
        };

        Some(Member::Class(ConfigClass {
            name,
            span: Span::new(kw_span.start, end),
            spans: ClassSpans {
                keyword: kw_span,
                name: name_span,
                parents: parents_span,
                open_brace: Some(open_brace),
                close_brace,
                statement: Span::new(kw_span.start, statement_end),
            },
            parents,
            forward: false,
            members,
        }))
    }

    fn parse_delete(&mut self) -> Option<Member> {
        let kw_span = self.bump().span;
        let name_tok = self.bump();
        let name = match &name_tok.kind {
            TokenKind::Ident(s) | TokenKind::Str(s) => s.clone(),
            _ => {
                self.error(
                    name_tok.span,
                    format!(
                        "expected name after `delete`, found {}",
                        name_tok.kind.describe()
                    ),
                );
                return None;
            }
        };
        let span = Span::new(kw_span.start, name_tok.span.end);
        let semi_end = if !self.at(&TokenKind::Semicolon) {
            self.expected(format!("`;` after `delete {name}`"));
            span.end
        } else {
            self.bump().span.end
        };
        Some(Member::Delete {
            name,
            span,
            statement: Span::new(kw_span.start, semi_end),
        })
    }

    fn parse_property(&mut self) -> Option<Member> {
        let name_tok = self.bump();
        let name = match &name_tok.kind {
            TokenKind::Ident(s) | TokenKind::Str(s) => s.clone(),
            _ => return None, // parse_member only calls this for an Ident or Str name
        };
        let name_span = name_tok.span;
        let mut declared_array = false;
        let mut brackets = None;
        if self.at(&TokenKind::LBracket) {
            let open = self.bump().span;
            if !self.at(&TokenKind::RBracket) {
                self.expected(format!("`]` in array declaration of `{name}`"));
                return None;
            }
            let close = self.bump().span;
            declared_array = true;
            brackets = Some(Span::new(open.start, close.end));
        }

        let mut array_append = false;
        let mut operator = None;
        if self.at(&TokenKind::Other('+')) {
            // `name[] += { ... };` — append to the inherited array.
            let plus = self.bump().span;
            if !self.at(&TokenKind::Equals) {
                self.expected(format!("`=` after `+` in `{name}`"));
                return None;
            }
            array_append = true;
            operator = Some(Span::new(plus.start, self.peek().span.end));
        }

        if !self.at(&TokenKind::Equals) {
            // A bare declaration with no value (rare) — accept and move on.
            if self.at(&TokenKind::Semicolon) {
                let semi_end = self.bump().span.end;
                let statement = Span::new(name_span.start, semi_end);
                let decl_end = brackets.map_or(name_span.end, |b| b.end);
                return Some(Member::Property(ConfigProperty {
                    name,
                    span: statement,
                    spans: PropertySpans {
                        name: name_span,
                        brackets,
                        operator: operator.unwrap_or(Span::new(decl_end, decl_end)),
                        value: None,
                        value_items: Vec::new(),
                        array_braces: None,
                        statement,
                    },
                    declared_array,
                    array_append,
                    value: None,
                }));
            }
            self.expected(format!("`=` after `{name}`"));
            return None;
        }
        let operator = operator.unwrap_or(Span::new(self.peek().span.start, self.peek().span.end));
        self.bump(); // '='
                     // A missing value drops the property; recovery then stops at the `;` or `}`.
        let (value, spans) = self.parse_value()?;

        if !self.at(&TokenKind::Semicolon) {
            self.expected(format!("`;` after value of `{name}`"));
            return None;
        }
        let semi_end = self.bump().span.end;
        let statement = Span::new(name_span.start, semi_end);
        Some(Member::Property(ConfigProperty {
            name,
            span: statement,
            spans: PropertySpans {
                name: name_span,
                brackets,
                operator,
                value: Some(spans.span),
                value_items: spans.items,
                array_braces: spans.braces,
                statement,
            },
            declared_array,
            array_append,
            value: Some(value),
        }))
    }

    fn parse_value(&mut self) -> Option<(Value, ValueSpans)> {
        if self.at(&TokenKind::LBrace) {
            return self.parse_array();
        }
        let (value, span) = match scalar(&self.peek().kind) {
            Some(value) => (value, self.bump().span),
            // Numbers are lexed from their first digit, so a sign arrives as a
            // separate token (`x = -0.5;`).
            None if self.at(&TokenKind::Other('-')) => {
                self.bump();
                self.negated_number()?
            }
            None => {
                let other = self.peek().kind.clone();
                // A `;` or `}` is left for the caller's recovery to stop on.
                let span = if matches!(other, TokenKind::Semicolon | TokenKind::RBrace) {
                    self.peek().span
                } else {
                    self.bump().span
                };
                self.error(
                    span,
                    format!("expected a value, found {}", other.describe()),
                );
                return None;
            }
        };
        // An expression tail (`a = x + y;`) is folded verbatim into `Other` so
        // the source text is not lost; it is still reported as unsupported.
        if !self.at(&TokenKind::Semicolon)
            && !self.at(&TokenKind::Comma)
            && !self.at(&TokenKind::RBrace)
            && !self.at(&TokenKind::Eof)
        {
            let end = self.fold_tail();
            self.error(
                Span::new(span.end, end),
                "unsupported expression in value (v1 parses plain scalars and arrays only)",
            );
            let raw = self.text[span.start..end].to_string();
            return Some((
                Value::Other(raw),
                ValueSpans::scalar(Span::new(span.start, end)),
            ));
        }
        Some((value, ValueSpans::scalar(span)))
    }

    /// Consume tokens up to the next `;`, `,`, `}` or EOF; returns the last
    /// consumed token's end offset.
    fn fold_tail(&mut self) -> usize {
        let mut end = self.peek().span.end;
        while !self.at(&TokenKind::Semicolon)
            && !self.at(&TokenKind::RBrace)
            && !self.at(&TokenKind::Comma)
            && !self.at(&TokenKind::Eof)
        {
            end = self.bump().span.end;
        }
        end
    }

    /// Value and extent of a `-` already consumed, e.g. `-2` in an array.
    fn negated_number(&mut self) -> Option<(Value, Span)> {
        let minus = self.toks[self.pos - 1].span;
        match self.peek().kind.clone() {
            TokenKind::Number(n) => {
                let end = self.bump().span.end;
                Some((Value::Number(format!("-{n}")), Span::new(minus.start, end)))
            }
            other => {
                self.error(
                    minus,
                    format!("expected a number after `-`, found {}", other.describe()),
                );
                None
            }
        }
    }

    fn parse_array(&mut self) -> Option<(Value, ValueSpans)> {
        let open = self.bump().span;
        if self.array_depth >= MAX_NESTING {
            self.error(
                open,
                format!("array nested deeper than {MAX_NESTING} levels"),
            );
            let close = self.skip_nested();
            return Some((
                Value::Array(Vec::new()),
                ValueSpans {
                    span: Span::new(open.start, close.map_or(open.end, |c| c.end)),
                    items: Vec::new(),
                    braces: close.map(|c| (open, c)),
                },
            ));
        }
        self.array_depth += 1;
        let mut close = None;
        let mut items = Vec::new();
        let mut item_spans = Vec::new();
        loop {
            if self.at(&TokenKind::RBrace) {
                close = Some(self.bump().span);
                break;
            }
            if self.at(&TokenKind::Eof) {
                self.error(self.peek().span, "unterminated array; missing `}`");
                break;
            }
            if self.at(&TokenKind::Comma) {
                // Allow stray commas (trailing/leading) per Arma tolerance.
                self.bump();
                continue;
            }
            if self.at(&TokenKind::LBrace) {
                if let Some((value, spans)) = self.parse_array() {
                    items.push(value);
                    item_spans.push(spans.span);
                }
                continue;
            }
            let t = self.bump();
            let (item, span) = match scalar(&t.kind) {
                Some(value) => (value, t.span),
                None => match &t.kind {
                    TokenKind::Other('-') => match self.negated_number() {
                        Some(pair) => pair,
                        None => continue,
                    },
                    // A #define inside an array body — skip (already bumped).
                    TokenKind::Preproc(_) => continue,
                    other => {
                        self.error(t.span, format!("unexpected {} in array", other.describe()));
                        continue;
                    }
                },
            };
            if !self.at(&TokenKind::Comma)
                && !self.at(&TokenKind::RBrace)
                && !self.at(&TokenKind::Eof)
                && !matches!(self.peek().kind, TokenKind::Preproc(_))
            {
                let end = self.fold_tail();
                items.push(Value::Other(self.text[span.start..end].to_string()));
                item_spans.push(Span::new(span.start, end));
                continue;
            }
            items.push(item);
            item_spans.push(span);
        }
        self.array_depth -= 1;
        let end = close.map_or_else(|| item_spans.last().map_or(open.end, |s| s.end), |c| c.end);
        Some((
            Value::Array(items),
            ValueSpans {
                span: Span::new(open.start, end),
                items: item_spans,
                braces: close.map(|c| (open, c)),
            },
        ))
    }

    /// Consume tokens through the `}` matching a brace the caller has already
    /// consumed, so a refused level is skipped without recursing.
    fn skip_nested(&mut self) -> Option<Span> {
        let mut level = 1usize;
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::LBrace) {
                level += 1;
            } else if self.at(&TokenKind::RBrace) {
                level -= 1;
                if level == 0 {
                    return Some(self.bump().span);
                }
            }
            self.bump();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Value;

    fn parse_ok(text: &str) -> ConfigFile {
        let r = parse(text);
        assert!(r.issues.is_empty(), "unexpected issues: {:?}", r.issues);
        r.file
    }

    #[test]
    fn non_ascii_outside_a_string_is_an_issue_not_a_panic() {
        for src in [
            "class A { x = abcà; };",
            "class A { x = Р; };",
            "class A { y[] = {х}; };",
        ] {
            let r = parse(src);
            assert!(!r.issues.is_empty(), "no issue for {src}");
        }
    }

    #[test]
    fn non_ascii_value_error_names_the_character() {
        let r = parse("class A { x = à; };");
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
        assert!(
            r.issues[0].message.contains('à'),
            "message: {}",
            r.issues[0].message
        );
    }

    #[test]
    fn digit_leading_class_name_parses() {
        let f = parse_ok("class 1kHz_x { a = 1; };");
        let c = f.find_class("1kHz_x").expect("class 1kHz_x");
        assert_eq!(c.name, "1kHz_x");
        assert!(matches!(
            c.property("a").and_then(|p| p.value.as_ref()),
            Some(Value::Number(n)) if n == "1"
        ));
    }

    #[test]
    fn digit_leading_property_name_parses() {
        let f = parse_ok("class A { 1kHz_x = 2; };");
        let c = f.find_class("A").unwrap();
        assert_eq!(c.property("1kHz_x").unwrap().name, "1kHz_x");
    }

    #[test]
    fn doubled_quote_value_decodes() {
        let f = parse_ok(r#"class A { init = "exec ""x.sqf"";"; };"#);
        assert!(matches!(
            f.find_class("A").and_then(|c| c.property("init")).and_then(|p| p.value.as_ref()),
            Some(Value::Str(s)) if s == r#"exec "x.sqf";"#
        ));
    }

    #[test]
    fn parses_cfgmods_shape() {
        let f = parse_ok(
            r#"class CfgPatches
{
	class MyMod
	{
		units[]={};
		weapons[]={};
		requiredVersion=0.1;
		requiredAddons[]={ "DZ_Data" };
	};
};
class CfgMods
{
	class MyMod
	{
		type = "mod";
		class defs
		{
			class gameScriptModule
			{
				value = "";
				files[] = { "MyMod/Scripts/3_Game" };
			};
		};
	};
};"#,
        );
        let patches = f.find_class("CfgPatches").unwrap();
        let mod_class = patches.child_class("MyMod").unwrap();
        let addons = mod_class.property("requiredAddons").unwrap();
        match &addons.value {
            Some(Value::Array(items)) => {
                assert_eq!(items, &[Value::Str("DZ_Data".into())])
            }
            other => panic!("expected array, got {other:?}"),
        }

        let cfgmods = f.find_class("CfgMods").unwrap();
        let mm = cfgmods.child_class("MyMod").unwrap();
        let defs = mm.child_class("defs").unwrap();
        let game = defs.child_class("gameScriptModule").unwrap();
        let files = game.property("files").unwrap();
        match &files.value {
            Some(Value::Array(items)) => {
                assert_eq!(items, &[Value::Str("MyMod/Scripts/3_Game".into())])
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn parses_class_inheritance() {
        let f = parse_ok("class Foo : Bar { x = 1; };\nclass Baz : Foo, Qux { y = 2; };");
        let foo = f.find_class("Foo").unwrap();
        assert_eq!(foo.parents, vec!["Bar"]);
        let baz = f.find_class("Baz").unwrap();
        assert_eq!(baz.parents, vec!["Foo", "Qux"]);
    }

    #[test]
    fn tolerates_comments_and_preproc() {
        let f = parse_ok(
            "/* header */\n#define DEBUG\nclass A { x = 1; }; // line\n#include \"cfg.hpp\"\nclass B : A { y[] = { 1, 2, 3 }; };",
        );
        assert!(f.find_class("A").is_some());
        assert!(f.find_class("B").is_some());
    }

    #[test]
    fn reports_missing_semicolon() {
        let r = parse("class A { x = 1 }");
        assert!(!r.issues.is_empty());
        assert_eq!(r.issues[0].kind, ParseErrorKind::Syntax);
    }

    #[test]
    fn reports_unbalanced_class() {
        let r = parse("class A { x = 1;");
        assert!(!r.issues.is_empty());
    }

    #[test]
    fn a_missing_value_keeps_the_next_property() {
        let r = parse("class A { x = ; y = 2; };");
        let a = r.file.find_class("A").expect("class A");
        assert!(
            a.property("x").is_none(),
            "a property with no value is dropped"
        );
        assert!(matches!(
            a.property("y").and_then(|p| p.value.as_ref()),
            Some(Value::Number(n)) if n == "2"
        ));
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
    }

    #[test]
    fn a_missing_value_before_a_close_keeps_the_class_closed() {
        let r = parse("class A { x = }; class B { q = 4; };");
        assert!(r.file.find_class("B").is_some());
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
    }

    #[test]
    fn array_with_trailing_and_nested() {
        let f = parse_ok("class A { a[] = { \"x\", }; b[] = { { 1, 2 }, 3 }; };");
        let a = f.find_class("A").unwrap();
        match a.property("a").and_then(|p| p.value.as_ref()) {
            Some(Value::Array(items)) => {
                assert_eq!(items, &[Value::Str("x".into())])
            }
            _ => panic!("expected array"),
        }
        match a.property("b").and_then(|p| p.value.as_ref()) {
            Some(Value::Array(items)) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(&items[0], Value::Array(_)));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn delete_member() {
        let f = parse_ok("class A : Base { delete OverrideMe; };");
        let a = f.find_class("A").unwrap();
        assert!(a
            .members
            .iter()
            .any(|m| matches!(m, Member::Delete { name, .. } if name == "OverrideMe")));
    }

    #[test]
    fn forward_class_declaration() {
        let f = parse_ok("class Base;\nexternal class Other: Base;\nclass Use: base { x = 1; };");
        let base = f.find_class("Base").unwrap();
        assert!(base.forward && base.members.is_empty());
        let other = f.find_class("Other").unwrap();
        assert!(other.forward);
        assert_eq!(other.parents, vec!["Base".to_string()]);
        assert_eq!(
            f.find_class("Use").unwrap().parents,
            vec!["base".to_string()]
        );
    }

    #[test]
    fn array_append_property() {
        let f = parse_ok("class A { x[] += { 1, 2 }; };");
        let x = f.find_class("A").unwrap().property("x").unwrap();
        assert!(x.declared_array && x.array_append);
        assert!(matches!(&x.value, Some(Value::Array(items)) if items.len() == 2));
    }

    #[test]
    fn stray_top_level_brace_terminates() {
        let r = parse("class A\n{\n};\n};");
        assert_eq!(r.file.members.len(), 1);
        assert_eq!(r.issues.len(), 1);
        assert!(r.issues[0].message.contains("unexpected `}`"));
    }

    #[test]
    fn a_config_nested_past_the_limit_reports_one_error() {
        let arrays = format!("x[] = {}{};", "{".repeat(65), "}".repeat(65));
        let r = parse(&arrays);
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
        assert_eq!(r.issues[0].kind, ParseErrorKind::Syntax);
        assert!(
            r.issues[0].message.contains("nested deeper than 64 levels"),
            "message: {}",
            r.issues[0].message
        );

        let classes = format!("{}{}", "class A {".repeat(65), "}".repeat(65));
        let r = parse(&classes);
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
        assert!(
            r.issues[0].message.contains("nested deeper than 64 levels"),
            "message: {}",
            r.issues[0].message
        );

        let deep = format!("x[] = {}{};", "{".repeat(100_000), "}".repeat(100_000));
        let r = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || parse(&deep))
            .expect("spawns")
            .join()
            .expect("the parser must not overflow its stack");
        assert_eq!(r.issues.len(), 1, "issues: {:?}", r.issues);
    }

    #[test]
    fn negative_numbers_are_values() {
        let f = parse_ok("class A { a = -100; b = -0.5; c[] = { -1, 2 }; };");
        let a = f.find_class("A").unwrap();
        assert!(
            matches!(a.property("a").unwrap().value, Some(Value::Number(ref n)) if n == "-100")
        );
        assert!(
            matches!(a.property("b").unwrap().value, Some(Value::Number(ref n)) if n == "-0.5")
        );
        assert!(matches!(
            a.property("c").unwrap().value,
            Some(Value::Array(ref items))
                if matches!(&items[0], Value::Number(n) if n == "-1")
        ));
    }

    fn slice(text: &str, span: Span) -> &str {
        &text[span.start..span.end]
    }

    fn siblings_are_sorted_and_disjoint(text: &str, members: &[Member]) {
        let mut spans: Vec<Span> = members
            .iter()
            .map(|m| match m {
                Member::Class(c) => c.spans.statement,
                Member::Property(p) => p.spans.statement,
                Member::Delete { statement, .. } => *statement,
            })
            .collect();
        let sorted = spans.clone();
        spans.sort_by_key(|s| s.start);
        assert_eq!(spans, sorted, "statements out of order: {spans:?}");
        for pair in spans.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "overlapping statements {:?} in {:?}",
                pair,
                slice(text, pair[0])
            );
        }
        for m in members {
            if let Member::Class(c) = m {
                siblings_are_sorted_and_disjoint(text, &c.members);
            }
        }
    }

    #[test]
    fn class_spans_are_exact() {
        let text = "class A : B\n{\n\tx = 1;\n};";
        let f = parse_ok(text);
        let c = f.find_class("A").unwrap();
        assert_eq!(slice(text, c.spans.keyword), "class");
        assert_eq!(slice(text, c.spans.name), "A");
        assert_eq!(slice(text, c.spans.parents.unwrap()), ": B");
        assert_eq!(slice(text, c.spans.open_brace.unwrap()), "{");
        assert_eq!(slice(text, c.spans.close_brace.unwrap()), "}");
        assert_eq!(slice(text, c.spans.statement), text);
        siblings_are_sorted_and_disjoint(text, &f.members);
    }

    #[test]
    fn one_line_and_kr_class_spans() {
        let one_line = "class A { x = 1; };";
        let f = parse_ok(one_line);
        let c = f.find_class("A").unwrap();
        assert_eq!(slice(one_line, c.spans.statement), one_line);
        assert!(c.spans.parents.is_none());

        let kr = "class A: B {\n\tx = 1;\n}";
        let f = parse_ok(kr);
        let c = f.find_class("A").unwrap();
        assert_eq!(slice(kr, c.spans.statement), kr);
        assert_eq!(slice(kr, c.spans.close_brace.unwrap()), "}");
        siblings_are_sorted_and_disjoint(kr, &f.members);
    }

    #[test]
    fn external_and_forward_class_spans() {
        let text = "external class Use: Base;\nclass Base;\nclass Cfg: Use\n{\n};";
        let f = parse_ok(text);
        let use_cls = f.find_class("Use").unwrap();
        assert!(use_cls.forward);
        assert_eq!(slice(text, use_cls.spans.keyword), "class");
        assert_eq!(slice(text, use_cls.spans.name), "Use");
        assert_eq!(slice(text, use_cls.spans.parents.unwrap()), ": Base");
        assert!(use_cls.spans.open_brace.is_none());
        assert_eq!(
            slice(text, use_cls.spans.statement),
            "external class Use: Base;"
        );

        let base = f.find_class("Base").unwrap();
        assert_eq!(slice(text, base.spans.statement), "class Base;");

        siblings_are_sorted_and_disjoint(text, &f.members);
    }

    #[test]
    fn property_spans_cover_arrays_and_operators() {
        let text = "class A\n{\n\tx[] += { 1, -2, { 3 } };\n\ty = \"v\";\n};";
        let f = parse_ok(text);
        let a = f.find_class("A").unwrap();

        let x = a.property("x").unwrap();
        assert_eq!(slice(text, x.spans.name), "x");
        assert_eq!(slice(text, x.spans.brackets.unwrap()), "[]");
        assert_eq!(slice(text, x.spans.operator), "+=");
        assert_eq!(slice(text, x.spans.value.unwrap()), "{ 1, -2, { 3 } }");
        let (open, close) = x.spans.array_braces.unwrap();
        assert_eq!(slice(text, open), "{");
        assert_eq!(slice(text, close), "}");
        let items: Vec<&str> = x
            .spans
            .value_items
            .iter()
            .map(|s| slice(text, *s))
            .collect();
        assert_eq!(items, vec!["1", "-2", "{ 3 }"]);
        assert_eq!(slice(text, x.spans.statement), "x[] += { 1, -2, { 3 } };");

        let y = a.property("y").unwrap();
        assert_eq!(slice(text, y.spans.operator), "=");
        assert_eq!(slice(text, y.spans.value.unwrap()), "\"v\"");
        assert!(y.spans.array_braces.is_none());
        assert!(y.spans.value_items.is_empty());
        siblings_are_sorted_and_disjoint(text, &f.members);
    }

    #[test]
    fn quoted_name_and_negative_value_spans() {
        let text = "class A { \"q\" = -0.5; };";
        let f = parse_ok(text);
        let a = f.find_class("A").unwrap();
        let q = a.property("q").unwrap();
        assert_eq!(slice(text, q.spans.name), "\"q\"");
        assert_eq!(slice(text, q.spans.value.unwrap()), "-0.5");
    }

    #[test]
    fn expression_tail_span_covers_every_token() {
        let text = "class A { a = x + y; };";
        let f = parse(text).file;
        let a = f.find_class("A").unwrap();
        let p = a.property("a").unwrap();
        assert_eq!(p.value, Some(Value::Other("x + y".into())));
        assert_eq!(slice(text, p.spans.value.unwrap()), "x + y");
    }

    #[test]
    fn delete_statement_span_includes_semicolon() {
        let text = "class A : Base { delete X; };";
        let f = parse_ok(text);
        let a = f.find_class("A").unwrap();
        let stmt = a
            .members
            .iter()
            .find_map(|m| match m {
                Member::Delete { statement, .. } => Some(*statement),
                _ => None,
            })
            .unwrap();
        assert_eq!(slice(text, stmt), "delete X;");
        siblings_are_sorted_and_disjoint(text, &f.members);
    }

    #[test]
    fn include_flag_follows_the_include_directive() {
        let text = "#define DEBUG\n#include \"h.hpp\"\nclass A\n{\n#ifdef X\n\tx = 1;\n#endif\n};";
        assert!(parse_ok(text).has_include);

        let plain = parse_ok("class A { x = 1; };");
        assert!(!plain.has_include);

        let other_directives = parse_ok("#define X\nclass A\n{\n#ifdef X\n\tx = 1;\n#endif\n};");
        assert!(!other_directives.has_include);
    }
}
