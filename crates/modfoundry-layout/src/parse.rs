//! Parse `.layout` text into widgets, properties and children.

use serde::Serialize;
use std::fmt;

/// A 1-based line and column; the column counts characters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

/// One value of a property, as written.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Value {
    Number(String),
    Word(String),
    Str(String),
}

/// A `key values...` line inside a widget.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Property {
    pub key: String,
    pub pos: Pos,
    pub values: Vec<Value>,
}

/// A widget: its class, optional name, properties and child widgets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Widget {
    pub class: String,
    pub name: Option<String>,
    pub pos: Pos,
    pub end_line: u32,
    pub properties: Vec<Property>,
    pub children: Vec<Widget>,
}

/// The first syntax error in a layout, with its position.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParseError {
    pub pos: Pos,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.column, self.message)
    }
}

const MAX_NESTING: u32 = 256;

/// Parse a layout file into its top-level widgets.
pub fn parse(text: &str) -> Result<Vec<Widget>, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = Parser {
        lines: split_lines(text).into_iter(),
        line: 0,
        pending: Vec::new(),
    };
    let mut widgets = Vec::new();
    while let Some(tokens) = parser.next_line()? {
        match classify(tokens)? {
            Line::Header(widget) => widgets.push(parser.widget(widget, 1)?),
            Line::BareHeader(property) => {
                if !parser.takes_lone_open()? {
                    return Err(error(property.pos, "widget header has no {"));
                }
                widgets.push(parser.widget(header_from(property), 1)?);
            }
            other => return Err(misplaced(other)),
        }
    }
    Ok(widgets)
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['\r', '\n']) {
        lines.push(&rest[..at]);
        let width = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + width..];
    }
    lines.push(rest);
    lines
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Quoted(String),
    Open,
    Close,
}

#[derive(Debug, Clone)]
struct Spanned {
    pos: Pos,
    token: Token,
}

enum Line {
    Header(Widget),
    BareHeader(Property),
    Property(Property),
    Open(Pos),
    Close(Pos),
}

struct Parser<'a> {
    lines: std::vec::IntoIter<&'a str>,
    line: u32,
    pending: Vec<Vec<Spanned>>,
}

impl Parser<'_> {
    fn next_line(&mut self) -> Result<Option<Vec<Spanned>>, ParseError> {
        if let Some(tokens) = self.pending.pop() {
            return Ok(Some(tokens));
        }
        for text in self.lines.by_ref() {
            self.line += 1;
            let mut tokens = lex_line(self.line, text)?;
            if tokens.is_empty() {
                continue;
            }
            if let Some(close) = split_trailing_close(&mut tokens) {
                self.pending.push(close);
            }
            return Ok(Some(tokens));
        }
        Ok(None)
    }

    fn takes_lone_open(&mut self) -> Result<bool, ParseError> {
        let Some(tokens) = self.next_line()? else {
            return Ok(false);
        };
        if is_lone(&tokens, Token::Open) {
            return Ok(true);
        }
        self.pending.push(tokens);
        Ok(false)
    }

    fn widget(&mut self, mut widget: Widget, depth: u32) -> Result<Widget, ParseError> {
        if depth > MAX_NESTING {
            let message = format!("widgets nest more than {MAX_NESTING} levels deep");
            return Err(error(widget.pos, &message));
        }
        loop {
            let Some(tokens) = self.next_line()? else {
                return Err(error(widget.pos, "widget is not closed"));
            };
            match classify(tokens)? {
                Line::Close(pos) => {
                    widget.end_line = pos.line;
                    return Ok(widget);
                }
                Line::BareHeader(property) if is_unopened_header(&property) => {
                    if self.takes_lone_open()? {
                        return Err(error(property.pos, "widget outside a child block"));
                    }
                    return Err(error(property.pos, "widget header has no {"));
                }
                Line::Property(property) if is_unopened_header(&property) => {
                    return Err(misplaced(Line::Property(property)));
                }
                Line::Property(property) | Line::BareHeader(property) => {
                    widget.properties.push(with_value(property)?);
                }
                Line::Open(pos) => widget.children.extend(self.block(pos, depth + 1)?),
                Line::Header(child) => {
                    return Err(error(child.pos, "widget outside a child block"));
                }
            }
        }
    }

    fn block(&mut self, open: Pos, depth: u32) -> Result<Vec<Widget>, ParseError> {
        let mut widgets = Vec::new();
        loop {
            let Some(tokens) = self.next_line()? else {
                return Err(error(open, "block is not closed"));
            };
            match classify(tokens)? {
                Line::Close(_) => return Ok(widgets),
                Line::Header(widget) => widgets.push(self.widget(widget, depth)?),
                Line::BareHeader(property) => {
                    if !self.takes_lone_open()? {
                        return Err(error(property.pos, "widget header has no {"));
                    }
                    widgets.push(self.widget(header_from(property), depth)?);
                }
                other => return Err(misplaced(other)),
            }
        }
    }
}

fn is_unopened_header(property: &Property) -> bool {
    property.key.ends_with("Class")
}

fn with_value(property: Property) -> Result<Property, ParseError> {
    if property.values.is_empty() {
        return Err(error(property.pos, "property has no value"));
    }
    Ok(property)
}

fn misplaced(line: Line) -> ParseError {
    match line {
        Line::BareHeader(property) => error(property.pos, "widget header has no {"),
        Line::Property(property) if is_unopened_header(&property) => {
            error(property.pos, "widget header has no {")
        }
        Line::Property(property) => error(property.pos, "property outside any widget"),
        Line::Open(pos) => error(pos, "block outside any widget"),
        Line::Close(pos) => error(pos, "stray }"),
        Line::Header(widget) => error(widget.pos, "widget header has no {"),
    }
}

fn error(pos: Pos, message: &str) -> ParseError {
    ParseError {
        pos,
        message: message.to_owned(),
    }
}

fn lex_line(line: u32, text: &str) -> Result<Vec<Spanned>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().enumerate().peekable();
    while let Some((index, c)) = chars.next() {
        let pos = Pos {
            line,
            column: index as u32 + 1,
        };
        let token = match c {
            ' ' | '\t' => continue,
            '{' => Token::Open,
            '}' => Token::Close,
            '"' => {
                let mut quoted = String::new();
                loop {
                    match chars.next() {
                        Some((_, '"')) => break,
                        Some((_, '\\')) if matches!(chars.peek(), Some((_, '"' | '\\'))) => {
                            if let Some((_, escaped)) = chars.next() {
                                quoted.push(escaped);
                            }
                        }
                        Some((_, ch)) => quoted.push(ch),
                        None => return Err(error(pos, "unterminated string")),
                    }
                }
                Token::Quoted(quoted)
            }
            _ => {
                let mut word = String::from(c);
                while let Some(&(_, next)) = chars.peek() {
                    if matches!(next, ' ' | '\t' | '"' | '{' | '}') {
                        break;
                    }
                    word.push(next);
                    chars.next();
                }
                Token::Word(word)
            }
        };
        tokens.push(Spanned { pos, token });
    }
    Ok(tokens)
}

fn classify(tokens: Vec<Spanned>) -> Result<Line, ParseError> {
    if is_lone(&tokens, Token::Open) {
        return Ok(Line::Open(tokens[0].pos.clone()));
    }
    if is_lone(&tokens, Token::Close) {
        return Ok(Line::Close(tokens[0].pos.clone()));
    }
    if let Some(next) = token_after_open(&tokens) {
        return Err(error(
            next.pos.clone(),
            "put each property on its own line after the {",
        ));
    }
    if let Some(property) = bare_header(&tokens) {
        return Ok(Line::BareHeader(property));
    }
    match tokens
        .iter()
        .find(|t| matches!(t.token, Token::Open | Token::Close))
        .map(|t| t.pos.clone())
    {
        Some(brace) => widget_header(tokens)
            .map(Line::Header)
            .ok_or_else(|| error(brace, "expected a property, a widget or a brace")),
        None => {
            let start = tokens
                .first()
                .map_or(Pos { line: 0, column: 0 }, |t| t.pos.clone());
            property(tokens).ok_or_else(|| error(start, "expected a property"))
        }
    }
}

fn is_lone(tokens: &[Spanned], want: Token) -> bool {
    matches!(tokens, [Spanned { token, .. }] if *token == want)
}

fn token_after_open(tokens: &[Spanned]) -> Option<&Spanned> {
    let at = tokens.iter().position(|t| t.token == Token::Open)?;
    tokens.get(at + 1).filter(|next| next.token != Token::Close)
}

fn bare_header(tokens: &[Spanned]) -> Option<Property> {
    if tokens.len() > 2 {
        return None;
    }
    let Token::Word(class) = &tokens[0].token else {
        return None;
    };
    if is_number(class) {
        return None;
    }
    let name = match tokens.get(1) {
        None => None,
        Some(Spanned {
            token: Token::Quoted(text),
            ..
        }) => Some(Value::Str(text.clone())),
        Some(Spanned {
            token: Token::Word(text),
            ..
        }) if !is_number(text) => Some(Value::Word(text.clone())),
        Some(_) => return None,
    };
    Some(Property {
        key: class.clone(),
        pos: tokens[0].pos.clone(),
        values: name.into_iter().collect(),
    })
}

fn header_from(property: Property) -> Widget {
    let name = property.values.into_iter().next().map(|value| match value {
        Value::Number(text) | Value::Word(text) | Value::Str(text) => text,
    });
    Widget {
        class: property.key,
        name,
        pos: property.pos,
        end_line: 0,
        properties: Vec::new(),
        children: Vec::new(),
    }
}

fn split_trailing_close(tokens: &mut Vec<Spanned>) -> Option<Vec<Spanned>> {
    if tokens.len() < 2 || tokens.last()?.token != Token::Close {
        return None;
    }
    let before = &tokens[..tokens.len() - 1];
    let opens = before.iter().filter(|t| t.token == Token::Open).count();
    let stays_closed = before.iter().any(|t| t.token == Token::Close);
    let open_is_last = before.last().is_some_and(|t| t.token == Token::Open);
    if stays_closed || opens > 1 || (opens == 1 && !open_is_last) {
        return None;
    }
    Some(vec![tokens.pop()?])
}

fn property(tokens: Vec<Spanned>) -> Option<Line> {
    let mut tokens = tokens.into_iter();
    let first = tokens.next()?;
    let key = match first.token {
        Token::Word(text) | Token::Quoted(text) => text,
        Token::Open | Token::Close => return None,
    };
    let values = tokens.filter_map(|t| into_value(t.token)).collect();
    Some(Line::Property(Property {
        key,
        pos: first.pos,
        values,
    }))
}

fn widget_header(tokens: Vec<Spanned>) -> Option<Widget> {
    let mut parts = tokens.into_iter();
    let class = parts.next()?;
    let Token::Word(class_name) = class.token else {
        return None;
    };
    let open = parts.next_back()?;
    if open.token != Token::Open {
        return None;
    }
    let name = match parts.next() {
        None => None,
        Some(name) => Some(into_text(name.token)?),
    };
    if parts.next().is_some() {
        return None;
    }
    Some(Widget {
        class: class_name,
        name,
        pos: class.pos,
        end_line: 0,
        properties: Vec::new(),
        children: Vec::new(),
    })
}

fn into_text(token: Token) -> Option<String> {
    match token {
        Token::Word(text) | Token::Quoted(text) => Some(text),
        Token::Open | Token::Close => None,
    }
}

fn into_value(token: Token) -> Option<Value> {
    match token {
        Token::Quoted(text) => Some(Value::Str(text)),
        Token::Word(text) if is_number(&text) => Some(Value::Number(text)),
        Token::Word(text) => Some(Value::Word(text)),
        Token::Open | Token::Close => None,
    }
}

fn is_number(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_digit())
        && text.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        && text.parse::<f64>().is_ok()
}
