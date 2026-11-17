use super::words::is_variable_modifier;
use super::{Enclosing, Expr, Item, Parser, Stmt};

const CONTROL_WORDS: [&str; 12] = [
    "if", "for", "foreach", "while", "switch", "case", "break", "continue", "delete", "return",
    "goto", "debug",
];

// Branches of one `if` chain; the tree is as deep as the chain is long.
const LADDER_LIMIT: usize = 2000;

enum ElseBranch {
    If(u32),
    Body(Option<Box<Item>>),
    None,
}

impl Parser<'_> {
    pub(crate) fn statement(&mut self, word: &str) -> Option<Item> {
        let line = self.cursor_line();
        match word {
            "if" => Some(self.if_statement(line)),
            "for" => Some(self.for_statement(line)),
            "foreach" => Some(self.foreach_statement(line)),
            "while" => Some(self.while_statement(line)),
            "switch" => Some(self.switch_statement(line)),
            "case" => Some(self.case_statement(line)),
            "default" => {
                self.bump();
                self.eat(":");
                Some(Item::Stmt(Stmt::Default { line }))
            }
            "return" => {
                self.bump();
                let value = if self.at_line_end() {
                    None
                } else {
                    self.expression(true, false)
                };
                self.end_command();
                Some(Item::Stmt(Stmt::Return { line, value }))
            }
            "break" | "continue" => {
                self.bump();
                self.end_command();
                Some(Item::Stmt(Stmt::Jump {
                    line,
                    keyword: word.to_string(),
                    target: None,
                }))
            }
            "goto" | "debug" => {
                self.bump();
                let target = if self.at_line_end() {
                    None
                } else {
                    let text = self.cur_text().to_string();
                    self.bump();
                    Some(text)
                };
                self.end_command();
                Some(Item::Stmt(Stmt::Jump {
                    line,
                    keyword: word.to_string(),
                    target,
                }))
            }
            "delete" => {
                self.bump();
                let value = self.expression(true, false);
                self.end_command();
                Some(Item::Stmt(Stmt::Delete { line, value }))
            }
            "thread" => {
                self.bump();
                let call = self.expression(true, false);
                self.end_command();
                Some(Item::Stmt(Stmt::Thread { line, call }))
            }
            "Sleep" | "Wait" if self.peek_text() == Some("(") => {
                let call = self.expression(true, false);
                Some(Item::Stmt(Stmt::Command { line, call }))
            }
            _ => None,
        }
    }

    pub(crate) fn is_statement_word(&self, word: &str) -> bool {
        CONTROL_WORDS.contains(&word) || matches!(word, "default" | "Sleep" | "Wait" | "else")
    }

    fn end_command(&mut self) {
        if self.skipping() || self.at_line_end() {
            return;
        }
        if !self.eat(";") {
            self.error("Syntax error".into());
        }
    }

    // An `else if` ladder is read in a loop; recursing per branch would cost the
    // stack one level for every branch.
    fn if_statement(&mut self, line: u32) -> Item {
        let mut links: Vec<(u32, Option<Expr>, Option<Box<Item>>)> = Vec::new();
        let mut line = line;
        let mut broken = false;
        let tail = loop {
            self.bump();
            let cond = self.condition();
            // After a cast with nothing to cast the rest of the line is gone,
            // and the body is still read from the next one.
            if self.take_broken_cast() {
                broken = true;
                if self.take_skip() {
                    self.next_line();
                }
            }
            if self.is_symbol(";") {
                self.bump();
                self.warning_at(line, "condition with no body?".into());
                links.push((line, cond, None));
                break None;
            }
            let then = self.body_of().map(Box::new);
            // Past the limit the branches are read and dropped: the game is
            // long gone by then, and the tree stays shallow enough to walk.
            if links.len() < LADDER_LIMIT {
                links.push((line, cond, then));
            }
            match self.else_clause() {
                ElseBranch::If(next) => line = next,
                ElseBranch::Body(body) => break body,
                ElseBranch::None => break None,
            }
        };
        // A cast with nothing to cast in a condition ends the level the `if`
        // stands in, once the statement itself is read.
        if broken {
            self.end_level_after_statement();
        }
        let mut otherwise = tail;
        while let Some((line, cond, then)) = links.pop() {
            let item = Item::Stmt(Stmt::If {
                line,
                cond,
                then,
                otherwise: otherwise.take(),
            });
            if links.is_empty() {
                return item;
            }
            otherwise = Some(Box::new(item));
        }
        Item::Stmt(Stmt::Empty { line })
    }

    fn else_clause(&mut self) -> ElseBranch {
        if self.is_symbol("else") {
            self.bump();
            return self.after_else();
        }
        if self.skipping() {
            return ElseBranch::None;
        }
        let saved_line = self.line;
        let saved_word = self.word;
        if self.next_line() && self.is_symbol("else") {
            self.bump();
            return self.after_else();
        }
        self.restore(saved_line, saved_word);
        ElseBranch::None
    }

    fn after_else(&mut self) -> ElseBranch {
        if !self.skipping() && !self.at_line_end() && self.is_symbol("if") {
            return ElseBranch::If(self.cursor_line());
        }
        ElseBranch::Body(self.body_of().map(Box::new))
    }

    // The single statement a control keyword takes, on this line or the next.
    pub(crate) fn body_of(&mut self) -> Option<Item> {
        if !self.enter() {
            return None;
        }
        let body = self.body_of_inner();
        self.leave();
        body
    }

    fn body_of_inner(&mut self) -> Option<Item> {
        if self.at_line_end() && !self.next_line() {
            return None;
        }
        if self.skipping() {
            return None;
        }
        if self.is_symbol("{") {
            let start_depth = self.depth();
            return Some(Item::Block(self.braced_body(Enclosing::Block, start_depth, None)));
        }
        if self.is_symbol(";") {
            let line = self.cursor_line();
            self.bump();
            return Some(Item::Stmt(Stmt::Empty { line }));
        }
        // A closing brace after a bare `if (x)` is the empty body in vanilla.
        if self.is_symbol("}") {
            return None;
        }
        // The body may be a declaration: `if (p) vector pos = p.GetPosition();`.
        if is_variable_modifier(self.cur_text()) || self.starts_declaration() {
            return Some(Item::Var(self.one_declaration()));
        }
        self.one_statement()
    }

    fn one_statement(&mut self) -> Option<Item> {
        let word = self.cur_text().to_string();
        let line = self.cursor_line();
        if self.is_statement_word(&word) && word != "else" {
            return self.statement(&word);
        }
        let expr = self.expression(true, true);
        match expr {
            Some(expr) => {
                // Leaves the cursor on a same-line `else`.
                self.eat(";");
                Some(Item::Stmt(Stmt::Expr { line, expr }))
            }
            None => {
                let found = self.found_word();
                self.error(format!("Invalid statement '{found}'"));
                None
            }
        }
    }

    fn condition(&mut self) -> Option<Expr> {
        if !self.eat("(") {
            let found = self.found_word();
            self.error(format!("Expected '(', not a '{found}'"));
            return None;
        }
        let cond = self.expression(false, false);
        if !self.is_symbol(")") {
            let found = self.found_word();
            self.error(format!("Expected ')', not a '{found}'"));
            return cond;
        }
        self.bump();
        cond
    }

    fn for_statement(&mut self, line: u32) -> Item {
        self.bump();
        if !self.eat("(") {
            let found = self.found_word();
            self.error(format!("Expected '(', not a '{found}'"));
            return Item::Stmt(Stmt::For {
                line,
                init: None,
                cond: None,
                step: None,
                body: None,
            });
        }
        let init = self.for_init();
        let cond = if self.is_symbol(";") {
            self.bump();
            None
        } else {
            let cond = self.expression(false, false);
            self.eat(";");
            cond
        };
        // Vanilla has `for (a; b;;)`; the second `;` is accepted.
        self.eat(";");
        let step = if self.is_symbol(")") {
            None
        } else {
            self.expression(true, false)
        };
        if !self.skipping() && !self.eat(")") {
            let found = self.found_word();
            self.error(format!("Expected ')', not a '{found}'"));
        }
        let body = if self.skipping() {
            None
        } else {
            self.body_of().map(Box::new)
        };
        Item::Stmt(Stmt::For {
            line,
            init,
            cond,
            step,
            body,
        })
    }

    fn for_init(&mut self) -> Option<Box<Item>> {
        if self.is_symbol(";") {
            self.bump();
            return None;
        }
        if is_variable_modifier(self.cur_text()) || self.starts_declaration() {
            // `int i = 0, count = n` declares both names, so the header keeps
            // the whole list rather than the last one.
            let line = self.cursor_line();
            let items = self.declarations(Vec::new());
            self.eat(";");
            let item = match items.len() {
                1 => items.into_iter().next(),
                _ => Some(Item::Block(super::Block { line, items })),
            };
            return Some(Box::new(item?));
        }
        let line = self.cursor_line();
        let expr = self.expression(true, false)?;
        self.eat(";");
        Some(Box::new(Item::Stmt(Stmt::Expr { line, expr })))
    }

    fn while_statement(&mut self, line: u32) -> Item {
        self.bump();
        let cond = self.condition();
        let body = if self.is_symbol(";") {
            self.bump();
            self.warning_at(line, "condition with no body?".into());
            None
        } else if self.skipping() {
            None
        } else {
            self.body_of().map(Box::new)
        };
        Item::Stmt(Stmt::While { line, cond, body })
    }

    fn foreach_statement(&mut self, line: u32) -> Item {
        self.bump();
        let mut variables = Vec::new();
        let mut iter = None;
        if self.eat("(") {
            while !self.is_symbol(")") && !self.cur_text().is_empty() {
                variables.push(self.one_declaration());
                if !self.eat(",") {
                    break;
                }
            }
            if self.eat(":") {
                iter = self.expression(false, false);
            }
            if !self.skipping() && !self.eat(")") {
                let found = self.found_word();
                self.error(format!("Expected ')', not a '{found}'"));
            }
        }
        let body = if self.skipping() {
            None
        } else {
            self.body_of().map(Box::new)
        };
        Item::Stmt(Stmt::Foreach {
            line,
            variables,
            iter,
            body,
        })
    }

    fn switch_statement(&mut self, line: u32) -> Item {
        self.bump();
        let cond = self.condition();
        let body = if self.skipping() {
            None
        } else {
            self.body_of().map(Box::new)
        };
        Item::Stmt(Stmt::Switch { line, cond, body })
    }

    fn case_statement(&mut self, line: u32) -> Item {
        self.bump();
        if self.is_symbol(":") {
            self.error("Syntax error".into());
            self.bump();
            return Item::Stmt(Stmt::Case { line, value: None });
        }
        let value = self.expression(true, false);
        if !self.skipping() && !self.eat(":") {
            let found = self.found_word();
            self.error(format!("Expected ':', not a '{found}'"));
        }
        Item::Stmt(Stmt::Case { line, value })
    }
}
