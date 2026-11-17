use super::words::{is_function_modifier, is_soft_name, is_variable_modifier};
use super::{ClassDecl, EnumDecl, Expr, FunctionDecl, Item, Parser, TypeRef, TypedefDecl, VarDecl};

impl Parser<'_> {
    pub(crate) fn type_ref(&mut self, refs_allowed: bool) -> Option<TypeRef> {
        let line = self.cursor_line();
        let modifiers = self.type_modifiers(refs_allowed);
        if self.at_line_end() {
            return None;
        }
        let name = self.cur_text().to_string();
        self.bump();
        let template = self.template_arguments();
        let mut array = false;
        if self.is_symbol("[") {
            self.bump();
            if self.eat("]") {
                array = true;
            } else if !self.skipping() {
                let found = self.found_word();
                self.error(format!("Expected ']', not a '{found}'"));
            }
        }
        Some(TypeRef {
            line,
            modifiers,
            name,
            template,
            array,
        })
    }

    fn type_modifiers(&mut self, refs_allowed: bool) -> Vec<String> {
        let mut modifiers = Vec::new();
        while matches!(self.cur_text(), "ref" | "autoptr") {
            if !refs_allowed {
                let word = self.cur_text().to_string();
                self.soft(format!("Unexpected statement '{word}'"));
            }
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        modifiers
    }

    // `<...>` after a type name. The words are read as types; `>>` closes the
    // list it is in and the enclosing one, so the count of closers is tracked.
    pub(crate) fn template_arguments(&mut self) -> Vec<TypeRef> {
        if self.template_open() >= TYPE_NESTING {
            self.stop_parse();
            return Vec::new();
        }
        if !self.enter() {
            return Vec::new();
        }
        let arguments = self.template_arguments_inner();
        self.leave();
        arguments
    }

    fn template_arguments_inner(&mut self) -> Vec<TypeRef> {
        let mut inner = Vec::new();
        if !self.is_symbol("<") {
            return inner;
        }
        // One counter for the whole nested list: a `>>` closes this list and
        // the enclosing one, so each call stops once the count drops below
        // its own level.
        self.open_template_lists();
        let level = self.template_open();
        self.bump();
        loop {
            if self.template_open() < level {
                if self.template_open() <= 0 {
                    self.close_template_lists();
                }
                return inner;
            }
            match self.cur_text() {
                ">" if self.is_symbol(">") => {
                    self.bump();
                    self.nest_template(-1);
                }
                ">>" if self.is_symbol(">>") => {
                    self.bump();
                    self.nest_template(-2);
                }
                "" => {
                    self.close_template_lists();
                    return inner;
                }
                "," if self.is_symbol(",") => self.bump(),
                _ => match self.type_ref(true) {
                    Some(type_) => {
                        inner.push(type_);
                        continue;
                    }
                    None => {
                        self.close_template_lists();
                        return inner;
                    }
                },
            }
        }
    }

    // A declaration line, including the `int a, b;` repeat after a comma. The
    // attributes belong to the first name only.
    pub(crate) fn declarations(&mut self, attributes: Vec<Expr>) -> Vec<Item> {
        let line = self.cursor_line();
        let mut modifiers = Vec::new();
        while is_variable_modifier(self.cur_text()) {
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        let type_ = self.type_ref(true);
        let mut items = Vec::new();
        let mut attributes = Some(attributes);
        loop {
            let var = self.declaration_name(
                line,
                type_.clone(),
                &modifiers,
                attributes.take().unwrap_or_default(),
            );
            let ended = self.skipping() || self.at_line_end();
            items.push(Item::Var(var));
            if ended || !self.is_symbol(",") {
                break;
            }
            self.bump();
        }
        if let Some(Item::Var(var)) = items.last()
            && self.init_ended_early(var.init.is_some(), var.ctor_args.is_some())
        {
            self.warning_at(line, "Broken expression (missing ';'?)".into());
        }
        items
    }

    // One name only: parameters, foreach variables and template parameters.
    pub(crate) fn one_declaration(&mut self) -> VarDecl {
        let line = self.cursor_line();
        let mut modifiers = Vec::new();
        while is_variable_modifier(self.cur_text()) {
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        let type_ = self.type_ref(true);
        self.declaration_name(line, type_, &modifiers, Vec::new())
    }

    fn declaration_name(
        &mut self,
        line: u32,
        type_: Option<TypeRef>,
        modifiers: &[String],
        attributes: Vec<Expr>,
    ) -> VarDecl {
        let mut decl = VarDecl {
            line,
            attributes,
            modifiers: modifiers.to_vec(),
            type_,
            name: String::new(),
            dims: Vec::new(),
            init: None,
            ctor_args: None,
        };
        if self.skipping() {
            return decl;
        }
        decl.name = self.cur_text().to_string();
        self.bump();
        while self.is_symbol("[") {
            self.bump();
            if self.is_symbol("]") {
                // `name[]` is an array of whatever size its value has.
                if let Some(type_) = decl.type_.as_mut() {
                    type_.array = true;
                }
            } else if let Some(dim) = self.expression(false, false) {
                decl.dims.push(dim);
            }
            if self.skipping() {
                return decl;
            }
            if !self.eat("]") {
                let found = self.found_word();
                self.error(format!("Expected ']', not a '{found}'"));
                break;
            }
        }
        if self.is_symbol("=") {
            self.bump();
            // `int x = ;` is accepted and declares the name with no value.
            if self.is_symbol(";") {
                return decl;
            }
            // A `{ }` list initialiser may open on the next line.
            if self.at_line_end() {
                self.next_line();
            }
            if !self.at_line_end() {
                decl.init = self.expression(false, true);
            }
        } else if self.is_symbol("(") {
            self.bump();
            decl.ctor_args = Some(self.arguments());
        }
        decl
    }

    pub(crate) fn class_decl(&mut self, attributes: Vec<Expr>) -> Item {
        let line = self.cursor_line();
        let mut modifiers = Vec::new();
        while matches!(self.cur_text(), "modded" | "sealed") {
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        self.bump();
        let name = self.checked_name();
        let template = self.template_parameters();
        let mut base = None;
        if self.is_any_symbol(&[":", "extends"]) {
            self.bump();
            base = self.type_ref(false);
        }
        let mut class = ClassDecl {
            line,
            end_line: line,
            attributes,
            modifiers,
            name,
            template,
            base,
            forward: false,
            members: Vec::new(),
        };
        // The body or a trailing `;` may sit on the next line.
        if self.at_line_end() && !self.skipping() {
            self.next_line();
        }
        if self.is_symbol(";") {
            self.bump();
            self.warning_at(line, "Forward declarations are deprecated!".into());
            class.forward = true;
            return Item::Class(class);
        }
        if self.skipping() {
            return Item::Class(class);
        }
        let start_depth = self.depth();
        class.members = self
            .braced_body(super::Enclosing::Class, start_depth, Some(line))
            .items;
        class.end_line = self.previous_line();
        Item::Class(class)
    }

    fn template_parameters(&mut self) -> Vec<VarDecl> {
        let mut params = Vec::new();
        if !self.is_symbol("<") {
            return params;
        }
        self.bump();
        loop {
            if self.is_any_symbol(&[">", ">>"]) || self.at_line_end() {
                self.eat(">");
                break;
            }
            params.push(self.one_declaration());
            if self.is_symbol(",") {
                self.bump();
            } else if !self.is_any_symbol(&[">", ">>"]) {
                break;
            }
        }
        params
    }

    pub(crate) fn enum_decl(&mut self, attributes: Vec<Expr>) -> Item {
        let line = self.cursor_line();
        let mut modifiers = Vec::new();
        while matches!(self.cur_text(), "modded" | "sealed") {
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        self.bump();
        let name = self.checked_name();
        let mut base = None;
        if self.is_any_symbol(&[":", "extends"]) {
            self.bump();
            base = self.type_ref(false);
        }
        if self.at_line_end() && !self.skipping() {
            self.next_line();
        }
        let mut members = Vec::new();
        if self.is_symbol("{") {
            self.bump();
            let start_depth = self.depth();
            self.set_depth(start_depth + 1);
            loop {
                if self.at_line_end() {
                    if !self.next_line() {
                        break;
                    }
                    continue;
                }
                if self.is_symbol("}") {
                    self.bump();
                    break;
                }
                if self.is_symbol(",") {
                    self.bump();
                    continue;
                }
                let item_line = self.cursor_line();
                if self.take_skip() {
                    continue;
                }
                match self.expression(true, false) {
                    Some(expr) => members.push(Item::Stmt(super::Stmt::Expr {
                        line: item_line,
                        expr,
                    })),
                    None => break,
                }
                if self.is_symbol(",") {
                    self.bump();
                } else if !self.is_symbol("}") && !self.at_line_end() {
                    let found = self.found_word();
                    self.soft(format!("Expected ',' or '}}', not a '{found}'"));
                    self.drop_rest_of_line();
                }
            }
            self.set_depth(start_depth);
        }
        Item::Enum(EnumDecl {
            line,
            attributes,
            modifiers,
            name,
            base,
            members,
        })
    }

    pub(crate) fn typedef_decl(&mut self) -> Item {
        let line = self.cursor_line();
        self.bump();
        if self.is_symbol("func") {
            self.bump();
            let name = self.cur_text().to_string();
            self.bump();
            let mut params = Vec::new();
            if self.eat("(") {
                loop {
                    if self.is_symbol(")") {
                        break;
                    }
                    params.push(self.one_declaration());
                    if !self.eat(",") {
                        break;
                    }
                }
                self.eat(")");
            }
            self.end_declaration();
            return Item::Typedef(TypedefDecl {
                line,
                name,
                type_: None,
                params,
            });
        }
        let type_ = self.type_ref(false);
        let name = self.cur_text().to_string();
        self.bump();
        while self.is_symbol("[") {
            self.bump();
            if !self.eat("]") {
                break;
            }
        }
        self.end_declaration();
        Item::Typedef(TypedefDecl {
            line,
            name,
            type_,
            params: Vec::new(),
        })
    }

    fn end_declaration(&mut self) {
        if !self.skipping() && !self.at_line_end() && !self.eat(";") {
            self.error("Syntax error".into());
        }
    }

    // A declaration with an initialiser whose next word is not `;` warns once
    // and the rest of the line is dropped.
    fn init_ended_early(&mut self, has_init: bool, has_ctor: bool) -> bool {
        if !(has_init || has_ctor) {
            return false;
        }
        self.at_line_end() || !self.is_symbol(";")
    }

    pub(crate) fn function_decl(&mut self, attributes: Vec<Expr>) -> Item {
        let line = self.cursor_line();
        let mut modifiers = Vec::new();
        while is_function_modifier(self.cur_text()) {
            modifiers.push(self.cur_text().to_string());
            self.bump();
        }
        let return_type = self.type_ref(true);
        let destructor = self.eat("~");
        let name = self.checked_name();
        let name_line = self.cursor_line();
        let params = self.parameters();
        // The `;` or body brace may sit on the next line after the parameter list.
        if self.at_line_end() && !self.skipping() && !self.is_any_symbol(&[";", "{"]) {
            self.next_line();
        }
        let body = if self.is_symbol(";") || self.skipping() {
            self.eat(";");
            None
        } else if self.is_symbol("{") {
            let start_depth = self.depth();
            Some(self.braced_body(super::Enclosing::Block, start_depth, None))
        } else {
            self.soft_at(name_line, "Missing function scope".into());
            None
        };
        Item::Function(FunctionDecl {
            line,
            attributes,
            name_line,
            modifiers,
            return_type,
            name,
            destructor,
            params,
            prototype: body.is_none(),
            body,
        })
    }

    fn parameters(&mut self) -> Vec<VarDecl> {
        let mut params = Vec::new();
        if !self.eat("(") {
            return params;
        }
        loop {
            // A parameter list may continue on the next line after `(` or `,`.
            if self.at_line_end() && !self.skipping() && !self.next_line() {
                break;
            }
            if self.is_symbol(")") || self.skipping() {
                break;
            }
            params.push(self.one_declaration());
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
        self.eat(")");
        params
    }

    // Class, enum and function names: a plain word or one of the soft names.
    fn checked_name(&mut self) -> String {
        let token = self.cur().cloned();
        let text = self.cur_text().to_string();
        self.bump();
        let Some(token) = token else {
            return text;
        };
        if token.kind == crate::preproc::TokenKind::Identifier || is_soft_name(&text) {
            return text;
        }
        self.soft_keeping_line_at(token.line, format!("Invalid identifier '{text}'"));
        text
    }
}

// Type lists nested deeper than any script writes end the parse on any stack:
// what the later stages do with a type grows much faster than its depth.
const TYPE_NESTING: i32 = 100;
