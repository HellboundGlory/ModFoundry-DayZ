use crate::preproc::{Token, TokenKind};

use super::words::{is_assignment, is_operator, precedence};
use super::{Expr, ExprKind, Parser, TypeRef};

// The loosest precedence level, which an expression starts at: every operator
// the table lists is at or below it.
const LOOSEST_LEVEL: u8 = 14;

impl Parser<'_> {
    // One expression over the words the cursor can still reach on this line.
    pub(crate) fn expression(
        &mut self,
        assign_allowed: bool,
        semicolon_warn: bool,
    ) -> Option<Expr> {
        let line = self.cursor_line();
        let expr = self.binary(LOOSEST_LEVEL, assign_allowed);
        // The warning needs a fresh statement on the next line; a closing
        // brace ends the block instead, unless a string left open ran into it.
        if expr.is_some()
            && semicolon_warn
            && self.at_line_end()
            && self.last_word() != ";"
            && (!self.next_line_starts_with("}") || self.line_ends_in_open_string())
        {
            self.warning_at(line, "Missing ';' at the end of line".into());
        }
        expr
    }

    fn line_ends_in_open_string(&self) -> bool {
        let last = self.tokens.get(self.line_end().saturating_sub(1));
        last.is_some_and(|token| token.kind == TokenKind::UnterminatedString)
    }

    fn last_word(&self) -> &str {
        self.tokens
            .get(self.line_end().saturating_sub(1))
            .map_or("", |token| token.text.as_str())
    }

    fn binary(&mut self, min_prec: u8, assign_allowed: bool) -> Option<Expr> {
        if !self.enter() {
            return None;
        }
        let chain = self.chain();
        let expr = self.binary_inner(min_prec, assign_allowed);
        self.set_chain(chain);
        self.leave();
        expr
    }

    fn binary_inner(&mut self, min_prec: u8, assign_allowed: bool) -> Option<Expr> {
        let mut left = self.unary(assign_allowed)?;
        loop {
            if self.at_line_end() {
                break;
            }
            if !self.extend_chain() {
                return None;
            }
            let word = self.cur_text().to_string();
            // Only a symbol word is an operator; a string or number that spells
            // one is an operand.
            let symbol = self.cur().is_some_and(|token| token.kind == TokenKind::Symbol);
            if symbol && matches!(word.as_str(), ";" | ")" | "]" | "}" | "{" | "," | ":") {
                break;
            }
            // Two operands with no operator between them; the word starts the
            // next operand.
            let Some(prec) = symbol.then(|| precedence(&word)).flatten() else {
                // A number left open has already said so.
                if !self.word.checked_sub(1).is_some_and(|before| open_number(&self.tokens[before])) {
                    self.error("Broken expression (missing ';'?)".into());
                }
                let at = (self.line, self.word);
                if self.unary(assign_allowed).is_none() && (self.line, self.word) == at {
                    self.bump();
                }
                continue;
            };
            // The table's smaller number binds tighter, so an operator looser
            // than the enclosing level (a larger number) continues here and a
            // tighter one closes the level.
            if prec > min_prec {
                break;
            }
            if is_assignment(&word) && !assign_allowed {
                self.error(format!("Assign operator '{word}' not allowed here"));
                break;
            }
            // An operator with nothing on its right; the operator is the word
            // the enclosing parser then reports.
            let operator_at = self.word;
            self.bump();
            // The right side stops below this level, so an operator of equal
            // precedence closes it and the level is left-associative.
            let Some(right) = self.binary(prec.saturating_sub(1), assign_allowed) else {
                if !self.skipping() {
                    let line = self.cursor_line();
                    let at_end = self.at_line_end();
                    self.error_at(line, "Syntax error".into());
                    if at_end {
                        self.warning_at(line, "Missing ';' at the end of line".into());
                    }
                    self.restore(self.line, operator_at);
                }
                break;
            };
            left = Expr {
                line: left.line,
                kind: ExprKind::Binary {
                    op: word,
                    left: Box::new(left),
                    right: Some(Box::new(right)),
                },
            };
        }
        Some(left)
    }

    // The steps that bind tighter than any binary operator: `.name`, `[...]`,
    // `(...)`, `Name<T>.`, and the postfix `++`/`--` a top-level operand takes.
    fn postfix(&mut self, mut left: Expr) -> Option<Expr> {
        loop {
            if self.at_line_end() {
                return Some(left);
            }
            if !self.extend_chain() {
                return None;
            }
            let word = self.cur_text().to_string();
            let symbol = self.cur().is_some_and(|token| token.kind == TokenKind::Symbol);
            if symbol && matches!(word.as_str(), ";" | ")" | "]" | "}" | "{" | "," | ":") {
                return Some(left);
            }
            match word.as_str() {
                "." if symbol => {
                    self.bump();
                    if self.at_line_end() || self.is_any_symbol(&[";", ")", "]", "}", ","]) {
                        self.error("Syntax error".into());
                        return Some(left);
                    }
                    let name = self.cur_text().to_string();
                    self.bump();
                    left = Expr {
                        line: left.line,
                        kind: ExprKind::Member {
                            object: Box::new(left),
                            name,
                        },
                    };
                }
                "[" if symbol => {
                    self.bump();
                    let index = if self.is_symbol("]") {
                        None
                    } else {
                        self.expression(false, false)
                    };
                    if !self.skipping() && !self.eat("]") {
                        let found = self.found_word();
                        self.error(format!("Expected ']', not a '{found}'"));
                    }
                    left = Expr {
                        line: left.line,
                        kind: ExprKind::Index {
                            object: Box::new(left),
                            index: index.map(Box::new),
                        },
                    };
                }
                "(" if symbol => {
                    self.bump();
                    let args = self.arguments();
                    left = Expr {
                        line: left.line,
                        kind: ExprKind::Call {
                            callee: Box::new(left),
                            args,
                        },
                    };
                }
                "++" | "--" if symbol => {
                    self.bump();
                    left = Expr {
                        line: left.line,
                        kind: ExprKind::Unary {
                            op: word,
                            operand: Some(Box::new(left)),
                            postfix: true,
                        },
                    };
                }
                "<" if symbol && self.template_call_ahead() => {
                    left = self.template_call(left);
                }
                _ => return Some(left),
            }
        }
    }

    // `Name<T>.Func()`: the `<` after an operand opens a type when it ends in
    // `>` or `>>` followed by `.`; otherwise `<` is the comparison operator.
    fn template_call_ahead(&mut self) -> bool {
        // No list closes after the last `>` of the line, so nothing past it
        // is read.
        let Some(last) = self.last_closer().filter(|last| *last >= self.word) else {
            return false;
        };
        let mut depth = 0i32;
        for (index, token) in self.tokens[self.word..=last].iter().enumerate() {
            match token.text.as_str() {
                "<" => depth += 1,
                ">" | ">>" if depth > 0 => {
                    depth -= if token.text == ">>" { 2 } else { 1 };
                    if depth <= 0 {
                        let next = self.word + index + 1;
                        return next < self.line_end() && self.tokens[next].text == ".";
                    }
                }
                _ => {}
            }
        }
        false
    }

    // The place of the last `>` on the line under the cursor, found once per
    // line.
    fn last_closer(&mut self) -> Option<usize> {
        if let Some((line, last)) = self.last_closer
            && line == self.line
        {
            return last;
        }
        let from = self.line_start();
        let last = self
            .tokens
            .get(from..self.line_end())
            .and_then(|line| line.iter().rposition(|token| matches!(token.text.as_str(), ">" | ">>")))
            .map(|at| from + at);
        self.last_closer = Some((self.line, last));
        last
    }

    // Skips a balanced `Name<...>` so the `.` after it reads as member access.
    fn template_call(&mut self, callee: Expr) -> Expr {
        let mut depth = 0i32;
        loop {
            let word = self.cur_text().to_string();
            match word.as_str() {
                "<" => {
                    depth += 1;
                    self.bump();
                }
                ">" | ">>" if depth > 0 => {
                    depth -= if word == ">>" { 2 } else { 1 };
                    self.bump();
                }
                "" => break,
                _ if depth > 0 => self.bump(),
                _ => break,
            }
            if depth <= 0 && word == ">>" {
                break;
            }
        }
        callee
    }

    fn unary(&mut self, assign_allowed: bool) -> Option<Expr> {
        if !self.enter() {
            return None;
        }
        let expr = self.unary_inner(assign_allowed);
        self.leave();
        expr
    }

    fn unary_inner(&mut self, assign_allowed: bool) -> Option<Expr> {
        let word = self.cur_text().to_string();
        let symbol = self.cur().is_some_and(|token| token.kind == TokenKind::Symbol);
        // A colon ends an expression; it never opens one.
        if symbol && is_operator(&word) && word != ":" {
            let line = self.cursor_line();
            self.bump();
            let operand = self.unary(assign_allowed);
            return Some(Expr {
                line,
                kind: ExprKind::Unary {
                    op: word,
                    operand: operand.map(Box::new),
                    postfix: false,
                },
            });
        }
        let operand = self.primary(assign_allowed)?;
        self.postfix(operand)
    }

    fn primary(&mut self, assign_allowed: bool) -> Option<Expr> {
        let token = self.cur()?;
        let line = token.line;
        let symbol = token.kind == TokenKind::Symbol;
        match token.text.as_str() {
            "(" if symbol => self.group(assign_allowed),
            "{" if symbol => Some(self.list()),
            "new" if symbol => self.new_expression(),
            ")" | "]" | "}" | ";" | "," | ":" if symbol => None,
            _ => {
                // A string keeps its quotes, so the tree never presents its
                // contents as a name.
                let text = match token.kind {
                    TokenKind::String => format!("\"{}\"", token.text),
                    _ => token.text.clone(),
                };
                let digit_name = token.kind == TokenKind::Identifier && text.starts_with(|c: char| c.is_ascii_digit());
                if open_number(token) {
                    self.error("Broken expression (missing ';'?)".into());
                }
                self.bump();
                let kind = match digit_name {
                    true => ExprKind::DigitName { text },
                    false => ExprKind::Leaf { text },
                };
                Some(Expr { line, kind })
            }
        }
    }

    // `( Name ) operand` is a cast when a non-operator word follows `)` and the
    // line has two more words after it.
    fn group(&mut self, assign_allowed: bool) -> Option<Expr> {
        let line = self.cursor_line();
        let words = self.words_from_cursor();
        let cast = words.len() >= 5
            && words.get(2).is_some_and(|token| token.text == ")")
            && words.get(3).is_some_and(|token| {
                !is_operator(&token.text)
                    && !matches!(
                        token.kind,
                        TokenKind::Symbol if matches!(
                            token.text.as_str(),
                            "(" | ")" | "}" | "[" | "]" | "," | ";" | "::" | ":" | "#"
                        )
                    )
            });

        // `( word )` before a `)` or a `,`, with two more words on the line, is
        // read as a cast with nothing to cast, however many brackets open
        // around it.
        let is = |at: usize, texts: &[&str]| {
            words
                .get(at)
                .is_some_and(|token| token.kind == TokenKind::Symbol && texts.contains(&token.text.as_str()))
        };
        let opened = words.iter().take_while(|token| token.kind == TokenKind::Symbol && token.text == "(").count();
        let word = words.get(opened).is_some_and(|token| token.kind != TokenKind::Symbol);
        if word && is(opened + 1, &[")"]) && is(opened + 2, &[")", ","]) && words.len() >= opened + 4 {
            self.error("Broken casting".into());
            if is(opened + 2, &[","]) {
                self.error("Broken expression (missing ';'?)".into());
            }
            self.note_broken_cast();
            // The cast itself was read; the cursor rests on what follows it.
            for _ in 0..opened + 2 {
                self.bump();
            }
            return None;
        }
        self.bump();
        if cast {
            let name = words[1].text.clone();
            self.bump();
            self.bump();
            let type_ = TypeRef {
                line,
                modifiers: Vec::new(),
                name,
                template: Vec::new(),
                array: false,
            };
            let operand = self.unary(assign_allowed);
            return Some(Expr {
                line,
                kind: ExprKind::Cast {
                    type_,
                    operand: operand.map(Box::new),
                },
            });
        }
        let mut inner = if self.is_symbol(")") {
            None
        } else {
            self.expression(false, false)
        };
        // An error inside already dropped the rest of the line.
        if inner.is_none() && self.skipping() {
            return None;
        }
        if self.eat(")") {
            return Some(inner.unwrap_or(Expr {
                line,
                kind: ExprKind::Leaf {
                    text: String::new(),
                },
            }));
        }
        if self.at_line_end() && self.next_line() {
            if !self.is_symbol(")") && inner.is_none() {
                inner = self.expression(false, false);
            }
            let value = inner.take();
            if self.eat(")") {
                return Some(value.unwrap_or(Expr {
                    line,
                    kind: ExprKind::Leaf {
                        text: String::new(),
                    },
                }));
            }
            inner = value;
        }
        self.soft("Unmatched brackets detected!".into());
        Some(inner.unwrap_or(Expr {
            line,
            kind: ExprKind::Leaf {
                text: String::new(),
            },
        }))
    }

    // `{ a, b }`. A line break also separates items, so commas are optional
    // between lines.
    fn list(&mut self) -> Expr {
        let line = self.cursor_line();
        self.bump();
        let mut items = Vec::new();
        loop {
            if self.at_line_end() && !self.next_line() {
                break;
            }
            if self.is_symbol("}") {
                self.bump();
                break;
            }
            let Some(item) = self.expression(false, false) else {
                break;
            };
            items.push(item);
            if self.is_symbol(",") {
                self.bump();
            } else if !self.is_symbol("}") && !self.at_line_end() {
                break;
            }
        }
        Expr {
            line,
            kind: ExprKind::List { items },
        }
    }

    fn new_expression(&mut self) -> Option<Expr> {
        let line = self.cursor_line();
        self.bump();
        let type_ = self.type_ref(false);
        let args = if self.eat("(") {
            self.arguments()
        } else {
            Vec::new()
        };
        Some(Expr {
            line,
            kind: ExprKind::New { type_, args },
        })
    }

    // Argument list where the opening `(` is already consumed.
    pub(crate) fn arguments(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        loop {
            if self.at_line_end() && !self.next_line() {
                return args;
            }
            if self.is_symbol(")") {
                self.bump();
                return args;
            }
            let Some(arg) = self.expression(false, false) else {
                break;
            };
            args.push(arg);
            // The closing `)` may stand alone on the next line.
            if self.at_line_end() && !self.skipping() && self.next_line_starts_with(")") {
                self.next_line();
            }
            if self.is_symbol(",") {
                self.bump();
            } else if !self.is_symbol(")") {
                if !self.skipping() {
                    let found = self.found_word();
                    self.error(format!("Expected ',' or ')', not a '{found}'"));
                }
                break;
            }
        }
        if self.is_symbol(")") {
            self.bump();
        }
        args
    }
}

// A number that ends in its dot, with no digit or exponent after it.
fn open_number(token: &Token) -> bool {
    matches!(token.kind, TokenKind::DecimalNumber | TokenKind::HexNumber) && token.text.ends_with('.')
}
