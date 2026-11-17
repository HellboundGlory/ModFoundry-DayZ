use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity};
use crate::input::{Module, SourceFile};

use super::tables::{OPERATORS, RESERVED_WORDS};
use super::{LexOutput, Token, TokenKind};

// Inside an inactive branch the four directives are matched by prefix at any
// position, so a longer prefix must be tried before the one it contains.
const INACTIVE_PREFIXES: [(&str, Prefix); 4] = [
    ("#ifnde", Prefix::Open),
    ("#ifde", Prefix::Open),
    ("#endi", Prefix::Close),
    ("#els", Prefix::Flip),
];

enum Prefix {
    Open,
    Close,
    Flip,
}

pub fn scan(file: &SourceFile, defines: &[String]) -> LexOutput {
    let mut scanner = Scanner {
        chars: file.text.chars().collect(),
        at: 0,
        line: 1,
        file,
        defines: defines.iter().map(|name| (name.clone(), None)).collect(),
        levels: Vec::new(),
        blocked: 0,
        crashed: false,
        leaks: Vec::new(),
        output: LexOutput::default(),
    };
    scanner.run();
    scanner.finish()
}

fn is_identifier_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

struct Scanner<'a> {
    chars: Vec<char>,
    at: usize,
    line: u32,
    file: &'a SourceFile,
    defines: HashMap<String, Option<String>>,
    levels: Vec<bool>,
    // How many open levels are inactive; zero means the scan is active.
    blocked: usize,
    crashed: bool,
    leaks: Vec<Diagnostic>,
    output: LexOutput,
}

impl Scanner<'_> {
    fn run(&mut self) {
        while self.at < self.chars.len() {
            if self.active() {
                self.step();
            } else {
                self.skip_inactive();
            }
        }
    }

    // The game dies while expanding a macro that has a body, before anything
    // from the define line reaches the log.
    fn finish(self) -> LexOutput {
        let mut output = self.output;
        output.defines = self.defines.into_keys().collect();
        output.defines.sort();
        if !self.crashed {
            output.diags.extend(self.leaks);
        }
        output
    }

    fn active(&self) -> bool {
        self.blocked == 0
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn step(&mut self) {
        let c = self.chars[self.at];
        match c {
            '\n' => {
                self.line += 1;
                self.at += 1;
            }
            '\r' | ' ' | '\t' => self.at += 1,
            '/' if self.peek(1) == Some('/') => self.line_comment(),
            '/' if self.peek(1) == Some('*') => self.block_comment(),
            '"' => self.string(),
            '-' if self.number_end(self.at + 1).is_some() => self.number(),
            '0'..='9' => self.number(),
            c if is_identifier_start(c) => self.identifier(),
            '#' => self.directive(),
            _ => self.symbol(),
        }
    }

    fn emit(&mut self, kind: TokenKind, text: &str, line: u32) {
        self.output.tokens.push(Token {
            kind,
            text: text.to_string(),
            line,
        });
    }

    fn diagnostic(&self, line: u32, message: String) -> Diagnostic {
        Diagnostic {
            file: self.file.path.clone(),
            line,
            module: Module::from(self.file.layer),
            severity: Severity::Error,
            message,
            fixme: false,
            game_crash: false,
            reports_only: false,
            related: None,
        }
    }

    fn error(&mut self, line: u32, message: String) {
        let diagnostic = self.diagnostic(line, message);
        self.output.diags.push(diagnostic);
    }

    fn number_end(&self, from: usize) -> Option<(usize, TokenKind)> {
        if !self.chars.get(from).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        if self.chars[from] == '0' && self.chars.get(from + 1) == Some(&'x') {
            let mut end = from + 2;
            while self.chars.get(end).is_some_and(|c| c.is_ascii_hexdigit()) {
                end += 1;
            }
            if end > from + 2 {
                // A dot with no digit after it belongs to the number.
                if self.chars.get(end) == Some(&'.') && !self.chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
                    end += 1;
                }
                return Some((end, TokenKind::HexNumber));
            }
        }
        let mut end = from;
        while self.chars.get(end).is_some_and(|c| c.is_ascii_digit()) {
            end += 1;
        }
        if self.chars.get(end) == Some(&'.') {
            // One digit before `.Name` is no number; a longer run is, and
            // takes the dot with it.
            if end - from == 1 && !self.chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
                return None;
            }
            end += 1;
            while self.chars.get(end).is_some_and(|c| c.is_ascii_digit()) {
                end += 1;
            }
        }
        if self.chars.get(end) == Some(&'e') {
            let mut after = end + 1;
            if matches!(self.chars.get(after), Some('+') | Some('-')) {
                after += 1;
            }
            let digits = after;
            while self.chars.get(after).is_some_and(|c| c.is_ascii_digit()) {
                after += 1;
            }
            if after > digits {
                end = after;
            }
        }
        // A digit run that stops at `_` is not a number; the underscore and
        // what follows it become an identifier.
        if self.chars.get(end) == Some(&'_') {
            return None;
        }
        Some((end, TokenKind::DecimalNumber))
    }

    fn number(&mut self) {
        let start = self.at;
        let unsigned = usize::from(self.chars[start] == '-');
        match self.number_end(start + unsigned) {
            Some((end, kind)) => {
                let text: String = self.chars[start..end].iter().collect();
                self.at = end;
                self.emit(kind, &text, self.line);
            }
            // Only reached for a digit run ending in `_`, which the game
            // splits into junk plus an identifier.
            None => {
                let text = self.chars[start].to_string();
                self.at += 1;
                self.emit(TokenKind::Identifier, &text, self.line);
            }
        }
    }

    fn identifier(&mut self) {
        let start = self.at;
        let mut end = start;
        while self.chars.get(end).is_some_and(|c| is_identifier_char(*c)) {
            end += 1;
        }
        let text: String = self.chars[start..end].iter().collect();
        self.at = end;
        let line = self.line;
        match text.as_str() {
            "__LINE__" => {
                let number = line.to_string();
                self.emit(TokenKind::String, &number, line);
            }
            "__FILE__" => {
                let path = self.file.path.clone();
                self.emit(TokenKind::String, &path, line);
            }
            _ if RESERVED_WORDS.contains(&text.as_str()) => {
                self.emit(TokenKind::Symbol, &text, line);
            }
            _ => {
                self.expanded_macro(&text, line);
                self.emit(TokenKind::Identifier, &text, line);
            }
        }
    }

    // A macro body word is re-emitted with a length the game cannot copy, and
    // the process dies there without printing anything. A name with no body is
    // only an #ifdef target and expands to nothing.
    fn expanded_macro(&mut self, name: &str, line: u32) {
        if self.crashed || !matches!(self.defines.get(name), Some(Some(_))) {
            return;
        }
        self.crashed = true;
        let mut diagnostic =
            self.diagnostic(line, format!("Expanding macro '{name}'"));
        diagnostic.game_crash = true;
        self.output.diags.push(diagnostic);
    }

    // Moves to `end`, counting the line breaks an escape swallowed inside a string.
    fn advance_to(&mut self, end: usize) {
        let breaks = self.chars[self.at..end].iter().filter(|c| **c == '\n').count();
        self.line += breaks as u32;
        self.at = end;
    }

    fn string(&mut self) {
        let line = self.line;
        let mut i = self.at + 1;
        let mut text = String::new();
        loop {
            let Some(&c) = self.chars.get(i) else {
                self.advance_to(i);
                self.unterminated_string(line, text);
                return;
            };
            match c {
                '\n' => {
                    self.advance_to(i);
                    self.unterminated_string(line, text);
                    return;
                }
                '"' => {
                    self.advance_to(i + 1);
                    self.emit(TokenKind::String, &text, line);
                    return;
                }
                '\\' => i += self.escape(i, &mut text),
                _ => {
                    text.push(c);
                    i += 1;
                }
            }
        }
    }

    fn escape(&self, i: usize, text: &mut String) -> usize {
        match self.chars.get(i + 1) {
            None => 1,
            Some('n') => {
                text.push('\n');
                2
            }
            Some('r') => {
                text.push('\r');
                2
            }
            Some('t') => {
                text.push('\t');
                2
            }
            // A backslash before a line break continues the string without
            // storing anything.
            Some('\n') => 2,
            Some('\r') => 3,
            Some(other) => {
                text.push(*other);
                2
            }
        }
    }

    fn unterminated_string(&mut self, line: u32, text: String) {
        self.emit(TokenKind::UnterminatedString, &text, line);
        let message = format!(
            "CParser: quoted string not closed on line {line}, module {}",
            self.file.path
        );
        self.error(line, message);
    }

    fn line_comment(&mut self) {
        self.at += 2;
        while self.chars.get(self.at).is_some_and(|c| *c != '\n') {
            self.at += 1;
        }
    }

    fn block_comment(&mut self) {
        self.at += 2;
        while let Some(&c) = self.chars.get(self.at) {
            if c == '\n' {
                self.line += 1;
                self.at += 1;
            } else if c == '*' && self.peek(1) == Some('/') {
                self.at += 2;
                return;
            } else {
                self.at += 1;
            }
        }
    }

    fn symbol(&mut self) {
        if let Some(entry) = OPERATORS.iter().find(|entry| self.matches(entry)) {
            let line = self.line;
            self.at += entry.len();
            self.emit(TokenKind::Symbol, entry, line);
            return;
        }
        let text = self.chars[self.at].to_string();
        self.at += 1;
        self.emit(TokenKind::Identifier, &text, self.line);
    }

    fn matches(&self, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(i, c)| self.chars.get(self.at + i) == Some(&c))
    }

    fn word_at(&self, from: usize) -> String {
        let mut end = from;
        while self.chars.get(end).is_some_and(|c| is_identifier_char(*c)) {
            end += 1;
        }
        self.chars[from..end].iter().collect()
    }

    fn skip_blanks(&mut self) {
        while matches!(self.peek(0), Some(' ') | Some('\t')) {
            self.at += 1;
        }
    }

    fn directive(&mut self) {
        let word = format!("#{}", self.word_at(self.at + 1));
        let line = self.line;
        match word.as_str() {
            "#ifdef" | "#ifndef" => {
                self.at += word.len();
                self.conditional(word == "#ifdef");
            }
            "#define" => {
                self.at += word.len();
                self.define();
            }
            // Reading the target file needs the file system, so the include
            // line is consumed and does not bring any words in.
            "#include" => {
                self.at += word.len();
                self.skip_line();
            }
            "#else" | "#endif" if self.levels.is_empty() => {
                self.at += word.len();
                self.emit(TokenKind::Symbol, &word, line);
            }
            "#else" => {
                self.at += word.len();
                self.flip_branch();
            }
            "#endif" => {
                self.at += word.len();
                self.pop_level();
            }
            _ => {
                self.at += 1;
                self.emit(TokenKind::Symbol, "#", line);
            }
        }
    }

    fn conditional(&mut self, ifdef: bool) {
        self.skip_blanks();
        let name = self.word_at(self.at);
        // A number is no name; it stays in the text as a word of its own.
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            if self.levels.is_empty() {
                let line = self.line;
                let message = "CParser: #if[n]def expected an identifier".to_string();
                self.error(line, message);
            } else {
                self.push_level(false);
            }
            return;
        }
        self.at += name.len();
        let defined = self.defines.contains_key(&name);
        let outer = self.active();
        self.push_level(outer && defined == ifdef);
    }

    fn flip_branch(&mut self) {
        let Some(top) = self.levels.len().checked_sub(1) else {
            return;
        };
        let was_open = self.levels[top];
        let outer = self.blocked - usize::from(!was_open) == 0;
        let now_open = outer && !was_open;
        self.levels[top] = now_open;
        self.blocked = self.blocked + usize::from(!now_open) - usize::from(!was_open);
    }

    fn push_level(&mut self, open: bool) {
        self.blocked += usize::from(!open);
        self.levels.push(open);
    }

    fn pop_level(&mut self) {
        if self.levels.pop() == Some(false) {
            self.blocked -= 1;
        }
    }

    fn define(&mut self) {
        self.skip_blanks();
        let name = self.word_at(self.at);
        if name.is_empty() {
            return;
        }
        self.at += name.len();
        self.skip_blanks();
        let body = self.word_at(self.at);
        if body.is_empty() {
            self.defines.insert(name, None);
            return;
        }
        let line = self.line;
        let expected = self.diagnostic(line, format!("Expected 'class', not a '{body}'"));
        let mut invalid = self.diagnostic(line, format!("Invalid identifier '{body}'"));
        invalid.fixme = true;
        self.leaks.push(expected);
        self.leaks.push(invalid);
        self.defines.insert(name, Some(body));
        self.skip_line();
    }

    fn skip_line(&mut self) {
        loop {
            let mut last = None;
            while self.chars.get(self.at).is_some_and(|c| *c != '\n') {
                if !self.chars[self.at].is_whitespace() {
                    last = Some(self.chars[self.at]);
                }
                self.at += 1;
            }
            if last == Some('\\') && self.chars.get(self.at) == Some(&'\n') {
                self.line += 1;
                self.at += 1;
            } else {
                return;
            }
        }
    }

    fn skip_inactive(&mut self) {
        let c = self.chars[self.at];
        match c {
            '\n' => {
                self.line += 1;
                self.at += 1;
            }
            '/' if self.peek(1) == Some('/') => self.line_comment(),
            '/' if self.peek(1) == Some('*') => self.block_comment(),
            _ => {
                for (prefix, action) in &INACTIVE_PREFIXES {
                    if !self.matches(prefix) {
                        continue;
                    }
                    self.at += prefix.len();
                    while self
                        .chars
                        .get(self.at)
                        .is_some_and(|c| is_identifier_char(*c))
                    {
                        self.at += 1;
                    }
                    match action {
                        Prefix::Open => self.push_level(false),
                        Prefix::Close => self.pop_level(),
                        Prefix::Flip => self.flip_branch(),
                    }
                    return;
                }
                self.at += 1;
            }
        }
    }
}
