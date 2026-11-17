use crate::preproc::TokenKind;

use super::words::{is_class_modifier, is_delimiter, is_variable_modifier};
use super::{Enclosing, Item, Parser};

impl Parser<'_> {
    // The one loop the game runs for the module, class bodies and every block.
    // `start_depth` is the brace depth before this level's own `{`.
    pub(crate) fn parse_level(&mut self, enclosing: Enclosing, start_depth: u32) -> Vec<Item> {
        if !self.enter() {
            return Vec::new();
        }
        let items = self.level_loop(enclosing, start_depth);
        self.leave();
        items
    }

    fn level_loop(&mut self, enclosing: Enclosing, start_depth: u32) -> Vec<Item> {
        let mut items = Vec::new();
        loop {
            if self.halted() {
                break;
            }
            if self.errors_over_limit() {
                let line = self.cursor_line();
                self.error_at(line, "Too many errors".into());
                self.stop_module();
                break;
            }
            if self.take_skip() || self.at_line_end() {
                if self.depth() > start_depth && !self.has_next_line() {
                    let line = self.last_line();
                    self.error_at(line, "Opened scope at the end of file, missing '}' ?".into());
                    break;
                }
                if !self.next_line() {
                    break;
                }
                continue;
            }
            let before = (self.line, self.word);
            self.take_broken_cast();
            match self.step(enclosing) {
                Step::Items(parsed) => {
                    items.extend(parsed);
                    if self.take_leave_level() {
                        break;
                    }
                }
                Step::Nothing => {}
                Step::End => {
                    if enclosing == Enclosing::Module {
                        self.stop_module();
                    }
                    break;
                }
            }
            // A step that read nothing must not be asked again.
            if (self.line, self.word) == before && !self.skipping() {
                self.bump();
            }
        }
        items
    }

    fn step(&mut self, enclosing: Enclosing) -> Step {
        let (count, delimiter) = self.look_ahead();
        let word = self.cur_text().to_string();

        // `class Name` is a class even when the line ends there; the body or the
        // base may follow on the next line.
        let class_line = word == "class"
            || (is_class_modifier(&word) && self.peek_text() == Some("class"));
        let enum_line = word == "enum"
            || (is_class_modifier(&word) && self.peek_text() == Some("enum"));
        if (class_line || enum_line) && count >= 1 {
            // A class or an enum is declared at module level only. Named on a
            // line of its own inside a function or another class, it ends the
            // level it sits in.
            if enclosing != Enclosing::Module && delimiter.is_none() {
                self.error("Class/Enum definition inside block. (missing '}' ?)".into());
                return Step::End;
            }
            if enclosing != Enclosing::Block {
                return Step::Items(vec![match class_line {
                    true => self.class_decl(Vec::new()),
                    false => self.enum_decl(Vec::new()),
                }]);
            }
        }
        // Characters outside ASCII are words the game cannot place. Alone on
        // a line in a function they read as a declaration it then refuses; at
        // module level it warns and gives the module up without an error line.
        let is_string = self.cur().is_some_and(|token| token.kind == TokenKind::String);
        if !word.is_ascii() && !is_string && delimiter.is_none() && enclosing != Enclosing::Class {
            let line = self.cursor_line();
            self.warning_at(line, "Broken expression (missing ';'?)".into());
            if enclosing == Enclosing::Module {
                self.fail_silently();
                return Step::End;
            }
        }
        // A closing bracket with nothing open is no code, and ends the parse.
        if word == "]" && enclosing != Enclosing::Block {
            self.error("Unknown code".into());
            self.halt();
            return Step::End;
        }
        if word == "typedef" {
            if enclosing != Enclosing::Module {
                self.error("Typedef definition inside block. (missing '}' ?)".into());
                return Step::End;
            }
            return Step::Items(vec![self.typedef_decl()]);
        }
        if count == 0 {
            match word.as_str() {
                "{" => return self.open_block(enclosing),
                "}" => return self.close_level(),
                ";" => {
                    self.bump();
                    return Step::Nothing;
                }
                "[" => {
                    // A list that is the whole of its line decorates the next
                    // line. A rejected list drops that declaration: the error
                    // flag then steps past the declaration's own line.
                    let list_only = self.list_fills_line();
                    let attributes = self.attributes();
                    if attributes.is_empty() {
                        if self.skipping() {
                            if list_only {
                                self.next_line();
                            }
                            self.drop_rest_of_line();
                        }
                        return Step::Nothing;
                    }
                    return Step::Items(self.attributed(attributes, enclosing));
                }
                _ => {}
            }
        }
        if matches!(delimiter.as_deref(), Some("(") | Some("::"))
            && count >= 2
            && !self.is_declaration_shape(enclosing, delimiter.as_deref(), count)
        {
            if enclosing == Enclosing::Block {
                self.error("Function definition inside block. (missing '}' ?)".into());
                return Step::End;
            }
            return Step::Items(vec![self.function_decl(Vec::new())]);
        }
        if self.is_declaration_shape(enclosing, delimiter.as_deref(), count) {
            return Step::Items(self.declarations(Vec::new()));
        }
        // A statement never stands at module level. A class body takes one
        // after a function written with no braces.
        if enclosing != Enclosing::Module
            && let Some(item) = self.statement(&word)
        {
            return Step::Items(vec![item]);
        }
        if enclosing == Enclosing::Block {
            let line = self.cursor_line();
            return match self.expression(true, true) {
                Some(expr) => Step::Items(vec![Item::Stmt(super::Stmt::Expr { line, expr })]),
                None => {
                    let found = self.found_word();
                    self.error(format!("Invalid statement '{found}'"));
                    Step::End
                }
            };
        }
        if delimiter.is_none() {
            let line = self.cursor_line();
            return match self.expression(true, true) {
                Some(expr) => Step::Items(vec![Item::Stmt(super::Stmt::Expr { line, expr })]),
                None => Step::Nothing,
            };
        }
        let line = match enclosing {
            Enclosing::Class => {
                // The error points at the `class` line and the cursor goes back
                // there when its brace sits on a line of its own, so the level
                // above reads the lines after it again.
                let line = self.class_line().unwrap_or_else(|| self.cursor_line());
                if !self.line_has_symbol(line, "{") {
                    self.rewind_to_line(line);
                }
                line
            }
            _ => self.cursor_line(),
        };
        self.error_at(line, "Syntax error".into());
        Step::End
    }

    fn open_block(&mut self, enclosing: Enclosing) -> Step {
        if enclosing != Enclosing::Block {
            self.error("Unexpected scope".into());
            return Step::End;
        }
        let start_depth = self.depth();
        let items = self.braced_body(Enclosing::Block, start_depth, None);
        Step::Items(vec![Item::Block(items)])
    }

    // A `}` at module level with no scope open is dropped; elsewhere it closes
    // the level and the loop returns to its caller.
    fn close_level(&mut self) -> Step {
        self.bump();
        if self.depth() == 0 {
            return Step::Nothing;
        }
        self.set_depth(self.depth() - 1);
        Step::End
    }

    fn attributed(&mut self, attributes: Vec<super::Expr>, enclosing: Enclosing) -> Vec<Item> {
        // The declaration an attribute list applies to may be on the next line.
        if self.at_line_end() && !self.skipping() {
            self.next_line();
        }
        let word = self.cur_text().to_string();
        let modifier_enum = is_class_modifier(&word) && self.peek_text() == Some("enum");
        if word == "enum" || modifier_enum {
            return vec![self.enum_decl(attributes)];
        }
        if word == "class" || is_class_modifier(&word) {
            return vec![self.class_decl(attributes)];
        }
        let (count, delimiter) = self.look_ahead();
        if matches!(delimiter.as_deref(), Some("(") | Some("::"))
            && count >= 2
            && !self.is_declaration_shape(enclosing, delimiter.as_deref(), count)
        {
            return vec![self.function_decl(attributes)];
        }
        self.declarations(attributes)
    }

    // A body between braces. The caller owns the depth before the `{`, which is
    // the depth the level is measured against when it runs out of lines.
    pub(crate) fn braced_body(
        &mut self,
        enclosing: Enclosing,
        start_depth: u32,
        class_line: Option<u32>,
    ) -> super::Block {
        let line = self.cursor_line();
        self.bump();
        self.set_depth(start_depth + 1);
        let previous = class_line.map(|class_line| self.set_class_line(Some(class_line)));
        let items = self.parse_level(enclosing, start_depth);
        if let Some(previous) = previous {
            self.set_class_line(previous);
        }
        self.set_depth(start_depth);
        super::Block { line, items }
    }

    fn attributes(&mut self) -> Vec<super::Expr> {
        let mut items = Vec::new();
        let line = self.cursor_line();
        self.bump();
        loop {
            if self.at_line_end() {
                break;
            }
            if self.is_symbol("]") {
                self.bump();
                return items;
            }
            let Some(expr) = self.expression(false, false) else {
                break;
            };
            if !matches!(expr.kind, super::ExprKind::Call { .. }) {
                self.error_at(line, "Expected attribute call".into());
                self.drop_rest_of_line();
                return Vec::new();
            }
            items.push(expr);
            if self.is_symbol(",") {
                self.bump();
            } else if !self.is_symbol("]") {
                break;
            }
        }
        if !self.eat("]") && !self.skipping() {
            let found = self.found_word();
            self.error(format!("Expected ']', not a '{found}'"));
        }
        items
    }

    // The declaration test: what the game decides about a line before parsing it.
    fn is_declaration_shape(&self, enclosing: Enclosing, delimiter: Option<&str>, count: usize) -> bool {
        let shape = match delimiter {
            // `int a[4];` puts `[` before both `;` and `=`.
            Some(";") | Some("=") | Some("[") => true,
            Some("(") if enclosing == Enclosing::Block => true,
            None => true,
            _ => false,
        };
        if !shape || count < 2 {
            return false;
        }
        let words = self.words_from_cursor();
        let Some(first) = words.first() else {
            return false;
        };
        if is_variable_modifier(&first.text) {
            return true;
        }
        if is_delimiter(&first.text) {
            return false;
        }
        if !matches!(first.kind, TokenKind::Symbol | TokenKind::Identifier) {
            return false;
        }
        let Some(second) = words.get(1) else {
            return false;
        };
        // The name must be a plain word; `int local;` fails here because
        // `local` is a reserved word, and the line becomes an expression.
        if is_delimiter(&second.text) {
            return false;
        }
        second.kind == TokenKind::Identifier || second.text == "<"
    }

    // Word count before the first delimiter word on this line.
    pub(crate) fn look_ahead(&self) -> (usize, Option<String>) {
        let words = self.words_from_cursor();
        let mut count = 0;
        for token in words {
            if is_delimiter(&token.text) {
                return (count, Some(token.text.clone()));
            }
            count += 1;
        }
        (count, None)
    }

    fn has_next_line(&self) -> bool {
        self.line + 1 < self.line_count()
    }
}

enum Step {
    Items(Vec<Item>),
    Nothing,
    End,
}
