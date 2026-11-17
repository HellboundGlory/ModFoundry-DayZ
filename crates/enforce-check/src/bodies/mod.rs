mod callbacks;
mod calls;
mod conversions;
mod crash;
mod engine_names;
mod expr;
mod locals;
mod operators;
mod scope;
mod size;
mod statements;
mod strong_refs;
mod templates;

pub use scope::{is_primitive, Locals, Member, Method, Param, Ty, TypeInfo, World};
pub use templates::{Instances, TemplateSource};

use conversions::{Dest, Kind};
use scope::base_name;

use std::collections::HashSet;

use crate::diag::{Diagnostic, Severity};
use crate::index::{is_name, template_lines, TemplatePlace};
use crate::input::{Module, SourceFile};
use crate::parser::{Block, Expr, ExprKind, FileAst, FunctionDecl, Item, Stmt, VarDecl};

// How a member is reached: by its bare name, as a field of a receiver, or as a
// method called on a receiver of the class named.
#[derive(Clone, Copy)]
pub(super) enum Reach<'r> {
    Bare,
    Field,
    Method(&'r str),
}

pub struct BodyChecker<'a> {
    pub(super) world: &'a World,
    instances: &'a mut Instances,
    // How many instances are being compiled, one inside the other.
    depth: u32,
    // Whether a rule the retail targets only warn about is an error here, in
    // which case the rows that follow it print too.
    pub(super) strict: bool,
    pub(super) file: String,
    pub(super) module: Module,
    // The class whose method is being read, if any.
    pub(super) class: Option<String>,
    // The class's own template parameter names, which are types in its body.
    pub(super) template_params: Vec<String>,
    // What those parameters stand for while an instance is compiled, the name
    // the game gives the instance, and the line its diagnostics sit on.
    pub(super) bindings: Vec<(String, Ty)>,
    pub(super) instance: Option<String>,
    report_line: Option<u32>,
    // The game died on an input: it prints nothing after that.
    crashed: bool,
    // The calls being read right now, one in the receiver or an argument of
    // the other.
    pub(super) calls_open: u32,
    // What the function being read has compiled to so far.
    instructions: usize,
    // The variables the function being read holds right now, and where the
    // module first held one too many.
    variables: usize,
    one_too_many: Option<(String, u32)>,
    // The `if`s open right now that are each the whole body of the one
    // before, and the line of the first.
    bare_ifs: usize,
    bare_ifs_line: u32,
    pub(super) static_method: bool,
    pub(super) function_name: String,
    // Whether the function being read declares a value to return, and its
    // type.
    pub(super) returns_value: bool,
    pub(super) return_type: Ty,
    enclosing: Vec<statements::Enclosing>,
    pub(super) locals: Locals,
    // Every body error points at the first line of the statement that holds it,
    // never at a sub-expression.
    pub(super) statement_line: u32,
    diagnostics: Vec<Diagnostic>,
}

// Checks the bodies of one module against everything the modules up to it
// declared, which the caller accumulates in `world`.
pub fn check(
    files: &[(&SourceFile, &FileAst)],
    world: &World,
    instances: &mut Instances,
    strict: bool,
) -> Vec<Diagnostic> {
    let mut checker = BodyChecker {
        world,
        instances,
        depth: 0,
        strict,
        file: String::new(),
        module: Module::Core,
        class: None,
        template_params: Vec::new(),
        bindings: Vec::new(),
        instance: None,
        report_line: None,
        crashed: false,
        calls_open: 0,
        instructions: 0,
        variables: 0,
        one_too_many: None,
        bare_ifs: 0,
        bare_ifs_line: 0,
        static_method: false,
        function_name: String::new(),
        returns_value: false,
        return_type: Ty::Unknown,
        enclosing: Vec::new(),
        locals: Locals::default(),
        statement_line: 1,
        diagnostics: Vec::new(),
    };
    for (file, ast) in files {
        checker.file = file.path.clone();
        checker.module = Module::from(file.layer);
        checker.aliased_instances(ast);
    }
    for (file, ast) in files {
        checker.file = file.path.clone();
        checker.module = Module::from(file.layer);
        checker.declared_instances(ast);
    }
    for (file, ast) in files {
        checker.file = file.path.clone();
        checker.module = Module::from(file.layer);
        checker.file_items(ast);
    }
    checker.die_of_one_variable_too_many();
    checker.diagnostics
}

impl BodyChecker<'_> {
    fn file_items(&mut self, ast: &FileAst) {
        // Global functions first, then each class, then the globals' own
        // initialisers.
        for item in &ast.items {
            if let Item::Function(decl) = item {
                self.function(decl);
            }
        }
        for item in &ast.items {
            if let Item::Class(decl) = item {
                // The merge renames a class it wrapped, and the game prints
                // whichever name it settled on.
                self.class = Some(
                    self.world
                        .merged_name(&self.file, decl.line)
                        .map(str::to_string)
                        .unwrap_or_else(|| decl.name.clone()),
                );
                self.template_params = decl.template.iter().map(|param| param.name.clone()).collect();
                // Bodies of a template class are compiled where it is instantiated.
                let members = match decl.template.is_empty() {
                    true => decl.members.as_slice(),
                    false => &[],
                };
                for member in members {
                    match member {
                        Item::Function(function) => self.function(function),
                        Item::Var(variable) => self.initialiser(variable),
                        _ => {}
                    }
                }
                self.class = None;
                self.template_params.clear();
            }
        }
        for item in &ast.items {
            if let Item::Var(variable) = item {
                self.initialiser(variable);
            }
        }
    }

    // The initialiser of a member or a global: read like a local's, with no
    // function around it.
    fn initialiser(&mut self, decl: &VarDecl) {
        let Some(init) = &decl.init else {
            return;
        };
        if !is_name(&decl.name) {
            return;
        }
        self.locals = Locals::default();
        self.static_method = false;
        self.statement_line = decl.line;
        let ty = self.ty_of_decl(decl);
        let refused = self.strong_ref_initialiser(decl, init, &ty);
        let actual = self.typed_in(init, Some(&ty));
        if refused {
            self.error(format!("Incompatible parameter '{}'", actual.text()));
        } else if let Some(digit) = digit_name(init) {
            // The digit a call was written on is all that is left of the
            // value, and is stored as the number it spells.
            if matches!(self.kind(&ty), Kind::Str | Kind::Vector | Kind::Typename | Kind::Class(_)) {
                self.error(format!("Incompatible parameter '{digit}'"));
            }
        } else if ty.base() != Some("auto") {
            self.store(init, &actual, &ty, Dest::Variable(&decl.name));
        }
    }

    fn function(&mut self, decl: &FunctionDecl) {
        let Some(body) = &decl.body else {
            return;
        };
        self.locals = Locals::default();
        self.static_method = decl.modifiers.iter().any(|modifier| modifier == "static");
        self.function_name = decl.name.clone();
        self.instructions = 0;
        self.variables = 0;
        self.returns_value = decl.return_type.as_ref().is_some_and(|type_| base_name(&type_.name) != "void");
        self.return_type = match self.overridden_return(decl) {
            Some(inherited) => inherited,
            None => decl.return_type.as_ref().map(|type_| self.ty_of_ref(type_)).unwrap_or_default(),
        };
        self.enclosing.clear();
        if self.lacks_return(decl, &body.items) {
            return;
        }
        self.statement_line = decl.line;
        for param in &decl.params {
            if param.init.as_ref().is_some_and(|default| !is_constant(default)) {
                self.error("Unknown type or code outside function".into());
            }
            self.locals.declare(Member {
                name: param.name.clone(),
                ty: self.ty_of_decl(param),
                modifiers: param.modifiers.clone(),
            });
        }
        self.locals.end_parameters();
        for item in &body.items {
            self.item(item);
            // The game is gone: nothing after this is read.
            if self.crashed {
                return;
            }
        }
    }

    // An `override` fills the slot of the method it overrides, so what it
    // returns is held to the type that method declared first.
    fn overridden_return(&self, decl: &FunctionDecl) -> Option<Ty> {
        if !decl.modifiers.iter().any(|modifier| modifier == "override") {
            return None;
        }
        let class = self.class.as_deref()?;
        let written: Vec<Ty> = decl.params.iter().map(|param| self.ty_of_decl(param)).collect();
        let same_count: Vec<&Method> = self
            .world
            .chain(class)
            .into_iter()
            .skip(1)
            .flat_map(|info| info.methods(&decl.name))
            .filter(|method| method.params.len() == decl.params.len())
            .collect();
        // Overloads can share a count, so the parameter types pick among them.
        let same_types = |method: &Method| {
            method.params.iter().zip(&written).all(|(param, ty)| param.ty.base() == ty.base())
        };
        same_count
            .iter()
            .rfind(|method| same_types(method))
            .or(same_count.last())
            .map(|method| method.ret.clone())
            .filter(|ty| matches!(ty, Ty::Named(_) | Ty::Container(..)))
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Block(block) => self.block(block),
            Item::Var(decl) => self.declaration(decl),
            Item::Stmt(statement) => self.statement(statement),
            _ => {}
        }
    }

    // A block gives the room of its variables back when it ends.
    fn block(&mut self, block: &Block) {
        let held = self.variables;
        for item in &block.items {
            self.item(item);
        }
        self.variables = held;
    }

    // A declaration the game refuses takes no room.
    fn declaration(&mut self, decl: &VarDecl) {
        let fresh = self.locals.find(&decl.name).is_none();
        let printed = self.diagnostics.len();
        self.local(decl);
        let refused = self.diagnostics[printed..].iter().any(|diagnostic| diagnostic.severity == Severity::Error);
        if fresh && !refused && !self.crashed && self.locals.find(&decl.name).is_some() {
            self.hold_variable(decl.line);
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        self.statement_line = statement.line();
        self.spend(self.statement_size(statement));
        match statement {
            Stmt::If { cond, then, otherwise, .. } => {
                if let Some(cond) = cond {
                    self.condition(cond);
                }
                let first = self.statement_line;
                self.if_body(then);
                // An `else if` ladder is one link per branch; walking it in a
                // loop keeps a long chain off the stack.
                let mut links = 0;
                let mut link_line = first;
                let mut next = otherwise;
                while let Some(item) = next.as_deref() {
                    let Item::Stmt(link @ Stmt::If { line, cond, then, otherwise }) = item else {
                        self.statement_line = link_line;
                        self.spend(size::JUMP);
                        self.branch(next);
                        break;
                    };
                    links += 1;
                    link_line = *line;
                    self.statement_line = first;
                    self.long_ladder(links);
                    self.statement_line = *line;
                    self.spend(size::JUMP + self.statement_size(link));
                    if let Some(cond) = cond {
                        self.condition(cond);
                    }
                    self.branch(then);
                    next = otherwise;
                }
            }
            Stmt::For { init, cond, step, body, .. } => {
                // The parser takes a `for` with no test; compiling it does
                // not, and goes on to its body all the same.
                if cond.is_none() {
                    self.error("Syntax error".into());
                }
                let held = self.variables;
                if let Some(init) = init {
                    self.item(init);
                }
                if let Some(cond) = cond {
                    self.condition(cond);
                }
                if let Some(step) = step {
                    self.expression_statement(step);
                }
                self.loop_body(body);
                self.variables = held;
            }
            Stmt::Foreach { variables, iter, body, .. } => {
                self.foreach(variables, iter.as_ref());
                self.loop_body(body);
            }
            Stmt::While { cond, body, .. } => {
                if let Some(cond) = cond {
                    self.condition(cond);
                }
                self.loop_body(body);
                self.statement_line = statement.line();
                self.spend(size::JUMP);
            }
            Stmt::Switch { cond, body, .. } => {
                if let Some(cond) = cond {
                    self.typed_in(cond, None);
                }
                self.enter(false);
                let held = self.variables;
                self.switch_body(body);
                self.variables = held;
                self.leave();
            }
            Stmt::Case { value, .. } => {
                if let Some(value) = value {
                    self.typed_in(value, None);
                }
            }
            Stmt::Return { value, .. } => self.return_statement(value.as_ref()),
            Stmt::Delete { value, .. } => {
                if let Some(value) = value {
                    self.delete(value);
                }
            }
            Stmt::Thread { call, .. } => {
                if let Some(call) = call {
                    self.thread(call);
                }
            }
            Stmt::Command { call, .. } => {
                // `Sleep(...)` and `Wait(...)` are statements: their built-in
                // names are not looked up, only the argument is checked.
                if let Some(call) = call {
                    self.command(call);
                }
            }
            Stmt::Expr { expr, .. } => self.expression_statement(expr),
            Stmt::Jump { keyword, target, .. } if keyword == "goto" => self.goto(target.as_deref()),
            Stmt::Jump { keyword, .. } => self.jump(keyword),
            Stmt::Default { .. } | Stmt::Empty { .. } => {}
        }
    }

    fn loop_body(&mut self, body: &Option<Box<Item>>) {
        self.enter(true);
        self.branch(body);
        self.leave();
    }

    // An `if` that is the whole body of another is compiled inside it, as an
    // `else if` is.
    fn if_body(&mut self, then: &Option<Box<Item>>) {
        if !matches!(then.as_deref(), Some(Item::Stmt(Stmt::If { .. }))) {
            return self.branch(then);
        }
        if self.bare_ifs == 0 {
            self.bare_ifs_line = self.statement_line;
        }
        self.bare_ifs += 1;
        self.statement_line = self.bare_ifs_line;
        self.long_ladder(self.bare_ifs);
        self.branch(then);
        self.bare_ifs -= 1;
    }

    // A body gives the room of its variables back, braced or not.
    fn branch(&mut self, item: &Option<Box<Item>>) {
        if let Some(item) = item {
            let held = self.variables;
            self.item(item);
            self.variables = held;
        }
    }

    // A local declaration: its type must resolve, then its initialiser is
    // checked against that type.
    fn local(&mut self, decl: &VarDecl) {
        if !is_name(&decl.name) {
            // Words outside ASCII read as a declaration the game refuses.
            if !decl.name.is_ascii() {
                self.statement_line = decl.line;
                self.error("Error in parameters".into());
                self.error("Error in parameters".into());
            }
            return;
        }
        self.statement_line = decl.line;
        if self.declaration_rules(decl) {
            self.local_with(decl, Ty::Unknown);
        }
        self.statement_line = decl.line;
        self.spend(self.declaration_size(decl));
    }

    // `element` is what the iterable yields, which an `auto` name takes.
    fn local_with(&mut self, decl: &VarDecl, element: Ty) {
        if !is_name(&decl.name) {
            return;
        }
        self.statement_line = decl.line;
        let mut ty = self.ty_of_decl(decl);
        if let Some(type_) = &decl.type_ {
            let name = base_name(&type_.name);
            if !self.template_arguments_fit(name, type_.template.len()) {
                self.locals.declare(Member {
                    name: decl.name.clone(),
                    ty: Ty::Unknown,
                    modifiers: decl.modifiers.clone(),
                });
                return;
            }
            let known = is_primitive(name) || self.world.types.is_known(name) || self.is_template_param(name);
            if !known {
                // The game reads the type twice before it gives up on the
                // declaration.
                for _ in 0..2 {
                    if !decl.modifiers.is_empty() {
                        self.error(format!("Bad type '{name}'"));
                    }
                    self.error(format!("Unknown type '{name}'"));
                }
                self.error("Error in parameters".into());
                ty = Ty::Unknown;
            } else if !self.instantiate(&ty) {
                // The instance did not compile, so the name stands for no type.
                self.error(format!("Bad type '{name}'"));
                self.locals.declare(Member {
                    name: decl.name.clone(),
                    ty: Ty::Unknown,
                    modifiers: decl.modifiers.clone(),
                });
                return;
            }
        }
        if let Some(init) = &decl.init {
            let expected = match ty.is_unknown() {
                true => None,
                false => Some(ty.clone()),
            };
            let printed = self.diagnostics.len();
            let actual = self.typed_in(init, expected.as_ref());
            // A chain that is the whole initialiser takes one step fewer.
            if matches!(init.kind, ExprKind::Member { .. }) && expr::is_full_chain(init) {
                self.error(expr::TOO_COMPLEX.into());
            }
            if ty.base() == Some("auto") {
                // A value that failed has no type for `auto` to take.
                let failed = self.diagnostics[printed..].iter().any(|diagnostic| diagnostic.severity == Severity::Error);
                if failed && actual.is_unknown() {
                    self.error("Unknown type 'auto'".into());
                    self.error("Error in parameters".into());
                }
                ty = actual;
            } else {
                self.store(init, &actual, &ty, Dest::Variable(&decl.name));
            }
        }
        for dim in &decl.dims {
            self.typed_in(dim, None);
        }
        if let Some(size) = decl.dims.first().and_then(literal_int) {
            self.locals.declare_size(&decl.name, size);
            let listed = match decl.init.as_ref().map(|init| &init.kind) {
                Some(ExprKind::List { items }) => items.len() as i64,
                _ => 0,
            };
            if listed > size {
                self.error("Too many values".into());
            }
        }
        if ty.base() == Some("auto") {
            ty = element;
        }
        self.locals.declare(Member {
            name: decl.name.clone(),
            ty,
            modifiers: decl.modifiers.clone(),
        });
    }

    // A local whose type names a template class with the wrong number of
    // arguments. False when the game gives up on the declaration.
    fn template_arguments_fit(&mut self, name: &str, written: usize) -> bool {
        // An alias carries its target's arguments, so only a class is counted.
        let expected = self.world.info(name).map_or(0, |info| info.template.len());
        let lines = template_lines(name, written, expected, TemplatePlace::Local);
        let fits = lines.iter().all(|line| line.soft);
        for line in lines {
            match fits {
                // Dropped arguments fail the module on no target.
                true => self.soft_keeps_loading(line.message),
                false => self.soft_or_error(line.soft, line.message),
            }
        }
        fits
    }

    pub(super) fn soft_or_error(&mut self, soft: bool, message: String) {
        match soft {
            true => self.soft(message),
            false => self.error(message),
        }
    }

    pub(super) fn error(&mut self, message: String) {
        self.report(message, Severity::Error, false);
    }

    // An error line after which the module still loads.
    pub(super) fn error_keeps_loading(&mut self, message: String) {
        self.report_only(message, false);
    }

    // A soft rule that fails the module on no target.
    fn soft_keeps_loading(&mut self, message: String) {
        self.report_only(message, true);
    }

    fn report_only(&mut self, message: String, fixme: bool) {
        let printed = self.diagnostics.len();
        self.report(message, Severity::Error, fixme);
        if self.diagnostics.len() > printed
            && let Some(diagnostic) = self.diagnostics.last_mut()
        {
            diagnostic.reports_only = true;
        }
    }

    // A rule the retail targets print as a `FIX-ME` warning.
    pub(super) fn soft(&mut self, message: String) {
        self.report(message, Severity::Error, true);
    }

    pub(super) fn warning(&mut self, message: String) {
        self.report(message, Severity::Warning, false);
    }

    // An input the game dies on instead of printing a line about it.
    pub(super) fn crash(&mut self, message: String) {
        self.report(message, Severity::Error, false);
        if let Some(diagnostic) = self.diagnostics.last_mut() {
            diagnostic.game_crash = true;
        }
        self.crashed = true;
    }

    // Runs a second read of an expression that was already checked, keeping
    // what it yields and dropping the lines it would print again.
    pub(super) fn quiet<T>(&mut self, read: impl FnOnce(&mut Self) -> T) -> T {
        let printed = self.diagnostics.len();
        let value = read(self);
        self.diagnostics.truncate(printed);
        value
    }

    fn report(&mut self, message: String, severity: Severity, fixme: bool) {
        if self.crashed {
            return;
        }
        self.diagnostics.push(Diagnostic {
            file: self.file.clone(),
            line: self.report_line.unwrap_or(self.statement_line),
            module: self.module,
            severity,
            message,
            fixme,
            game_crash: false,
            reports_only: false,
            related: None,
        });
    }

    // The caller's class and every class it derives from.
    fn ancestors(&self, class: &str) -> HashSet<String> {
        self.world
            .chain(class)
            .into_iter()
            .map(|info| plain_class(&info.name).to_string())
            .collect()
    }

    // One class derives from the other, whichever way round.
    fn related(&self, left: &str, right: &str) -> bool {
        self.ancestors(left).contains(plain_class(right)) || self.ancestors(right).contains(plain_class(left))
    }

    // None when the class being read may reach a member that `declaring`
    // declared, otherwise the word the game prints.
    //
    // Private is the declaring class alone, its `modded` layers included. A
    // class with a template instance among its ancestors still reads the
    // privates it inherits by their bare name (vanilla's `DragQueue` does).
    // A protected field asks for kinship with the declaring class, a protected
    // method for kinship with the class it is called on.
    pub(super) fn access(&self, declaring: &str, modifiers: &[String], reach: Reach) -> Option<&'static str> {
        let has = |word: &str| modifiers.iter().any(|modifier| modifier == word);
        let caller = self.class.as_deref();
        if has("private") {
            let own = caller.is_some_and(|caller| plain_class(caller) == plain_class(declaring));
            let inherited =
                matches!(reach, Reach::Bare) && caller.is_some_and(|caller| self.world.has_template_ancestor(caller));
            return (!own && !inherited).then_some("private");
        }
        if has("protected") {
            let other = match reach {
                Reach::Method(receiver) => receiver,
                Reach::Bare | Reach::Field => declaring,
            };
            let related = caller.is_some_and(|caller| self.related(caller, other));
            return (!related).then_some("protected");
        }
        None
    }

    // A template parameter of the class being read, or of any class the module
    // declares: a base class may name one its subclasses supply, which vanilla
    // does (`PlayerStatBase` reads `T` that `PlayerStat<T>` declares).
    pub(super) fn is_template_param(&self, name: &str) -> bool {
        self.template_params.iter().any(|param| param == name) || self.world.template_names.contains(name)
    }
}

// What a parameter default may be: literals, names and members reached through
// names, alone or joined by operators, which vanilla does for flag sets. A
// call is code, and there is no function around a default to run it in.
fn is_constant(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Leaf { .. } => true,
        ExprKind::Member { object, .. } => is_constant(object),
        ExprKind::Unary { operand, .. } => operand.as_deref().is_none_or(is_constant),
        ExprKind::Binary { left, right, .. } => is_constant(left) && right.as_deref().is_none_or(is_constant),
        _ => false,
    }
}

// The digit a chain of calls and member reads starts from, when the lexer
// read it as a name.
fn digit_name(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::DigitName { text } => Some(text),
        ExprKind::Member { object, .. } | ExprKind::Index { object, .. } => digit_name(object),
        ExprKind::Call { callee, .. } => digit_name(callee),
        _ => None,
    }
}

// A whole number written out in decimal.
pub(super) fn literal_int(expr: &Expr) -> Option<i64> {
    expr.leaf_text()?.parse().ok()
}

// A `modded` class takes the plain name while the class it wrapped keeps a
// renamed one, so both sides of a comparison drop the suffix.
pub(super) fn plain_class(name: &str) -> &str {
    name.split('#').next().unwrap_or(name)
}
