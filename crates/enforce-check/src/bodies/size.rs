use crate::parser::{is_assignment, Expr, ExprKind, Item, Stmt, VarDecl};

use super::conversions::number_literal;
use super::expr::MEMBER_STEPS;
use super::scope::{Member, Method, Ty};
use super::BodyChecker;

// The game compiles a function into room for this many instructions, and
// dies on the statement that passes it.
const INSTRUCTIONS: usize = 8192;

// The test and jump of each value an `if`, a `while` or a `for` tests.
const BRANCH: usize = 3;
// The jump over an `else`, the one back to the top of a loop, a `break` and a
// `continue`.
pub(super) const JUMP: usize = 1;
// A value copied into a variable, a member or what a function returns, and
// one put in a variable of its own before it is used: the first of two
// numbers under an operator, a member of this class or `null` handed on.
const STORE: usize = 2;
const OPERATOR: usize = 2;
// `v = v + 1`, `++v`, and a call to an engine function before its arguments.
const STEP: usize = 1;
const CALL: usize = 2;
// An engine method declared `owned external`, on top of the call (measured
// on `ClassName`).
const OWNED: usize = 3;
// A call or a member read off the result of a call, and off one that was
// itself made on the result of a call.
const OFF_CALL: usize = 1;
const OFF_CHAINED_CALL: usize = 4;
// An element read where it is used.
const ELEMENT: usize = 2;
const NEW: usize = 1;
// Each `.name` through an object, kept or passed on. An operator reads it for
// two less.
const MEMBER: usize = 4;
// A tested value kept as a `bool`: a store on each side and the jump between.
const TRUTH: usize = 5;
const RETURN: usize = 3;
// A `switch` with no `case`; each `case` is six and the first one less. A
// `default` is nothing.
const SWITCH: usize = 2;
const CASE: usize = 6;
// A `foreach`, its body aside: over a static array; over an `array` or a
// `set` that a local or a parameter holds; over an `array` that a member of
// this class holds; over a `map` with both its variables.
const FOREACH: usize = 12;
const ARRAY_FOREACH: usize = 15;
const MEMBER_FOREACH: usize = 18;
const MAP_FOREACH: usize = 25;
const CAST: usize = 5;
const CAST_TO: usize = 4;

// Every count below is either what the game was measured to compile a shape
// to, or the least an unmeasured shape could be: the checker must never
// report a function the game compiles.
impl BodyChecker<'_> {
    // Adds what one piece of the function being read compiles to.
    pub(super) fn spend(&mut self, instructions: usize) {
        self.instructions += instructions;
        if self.instructions > INSTRUCTIONS && !self.crashed {
            self.error("Too many instructions per function".into());
            self.crash(format!("A function of more than {INSTRUCTIONS} instructions"));
        }
    }

    // What a statement compiles to, the statements inside it left out.
    pub(super) fn statement_size(&self, statement: &Stmt) -> usize {
        let of = |expr: &Option<Expr>| expr.as_ref().map_or(0, |expr| self.expression_size(expr));
        match statement {
            Stmt::If { cond, .. } | Stmt::While { cond, .. } => cond.as_ref().map_or(BRANCH, |cond| self.condition_size(cond)),
            Stmt::Expr { expr, .. } => self.expression_size(expr),
            // One jump to the test and one back from the step. A `for` with
            // no test is refused, and its jumps are compiled all the same.
            Stmt::For { cond, step, .. } => cond.as_ref().map_or(0, |cond| self.condition_size(cond)) + JUMP + JUMP + of(step),
            Stmt::Foreach { variables, iter, .. } => of(iter) + self.foreach_size(variables.len(), iter),
            Stmt::Switch { cond, body, .. } => SWITCH + of(cond) - usize::from(has_case(body)),
            Stmt::Case { value, .. } => CASE + of(value),
            Stmt::Default { .. } => 0,
            Stmt::Return { value, .. } => RETURN + value.as_ref().map_or(0, |value| self.kept_size(None, value)),
            Stmt::Jump { .. } => JUMP,
            Stmt::Delete { value, .. } | Stmt::Thread { call: value, .. } | Stmt::Command { call: value, .. } => of(value),
            Stmt::Empty { .. } => 0,
        }
    }

    // A declaration is its initialiser stored: `int w;` is nothing.
    pub(super) fn declaration_size(&self, decl: &VarDecl) -> usize {
        decl.init.as_ref().map_or(0, |init| self.kept_size(None, init))
    }

    // Not measured, and taken for the cheapest: a `map` walked with one
    // variable, a `set` or a `map` that a member holds, and anything that is
    // not a plain name.
    fn foreach_size(&self, variables: usize, iter: &Option<Expr>) -> usize {
        let Some(name) = iter.as_ref().and_then(|iter| iter.leaf_text()) else {
            return FOREACH;
        };
        let container = |ty: &Ty, class: &str| matches!(ty, Ty::Container(name, _) if name == class);
        match (self.locals.find(name), self.own_member(name)) {
            (Some(local), _) if container(&local.ty, "array") || container(&local.ty, "set") => ARRAY_FOREACH,
            (Some(local), _) if container(&local.ty, "map") && variables == 2 => MAP_FOREACH,
            (None, Some(member)) if container(&member.ty, "array") => MEMBER_FOREACH,
            // Refused, so nothing of it is compiled but its body.
            (None, Some(member)) if matches!(member.ty, Ty::Array(_)) => 0,
            _ => FOREACH,
        }
    }

    // Each value tested is a branch: a comparison, and each side of `&&` or
    // `||`. `!` only turns the jump round. Nothing is worked out while
    // compiling: `if (true)` is a branch too.
    fn condition_size(&self, expr: &Expr) -> usize {
        match &expr.kind {
            ExprKind::Binary { op, left, right } if is_comparison(op) => BRANCH + self.expression_size(left) + self.operand_size(right),
            ExprKind::Binary { op, left, right } if is_logic(op) => {
                self.condition_size(left) + right.as_deref().map_or(0, |right| self.condition_size(right))
            }
            ExprKind::Unary { op, operand, .. } if op == "!" => operand.as_deref().map_or(BRANCH, |operand| self.condition_size(operand)),
            _ => BRANCH + self.expression_size(expr),
        }
    }

    // What an expression compiles to as a statement, or as a value an
    // operator or a test reads.
    fn expression_size(&self, expr: &Expr) -> usize {
        self.sized(expr).0
    }

    // The size, and the type of the value where its names give it: what a
    // call costs depends on what it is called on.
    fn sized(&self, expr: &Expr) -> (usize, Option<Ty>) {
        let size = match &expr.kind {
            ExprKind::Leaf { text } => return (0, self.leaf_type(text)),
            ExprKind::DigitName { .. } => 0,
            // An assignment kept by another is refused whole.
            ExprKind::Binary { op, left, right } if is_assignment(op) && (is_store(left) || right.as_deref().is_some_and(is_store)) => 0,
            ExprKind::Binary { op, left, right } if op == "=" => match right.as_deref() {
                Some(value) => self.store_size(left, value),
                None => self.expression_size(left),
            },
            // `a += b` is the operator and then the store.
            ExprKind::Binary { op, left, right } if is_assignment(op) => {
                OPERATOR + STORE + self.place_size(left) + self.operand_size(right)
            }
            ExprKind::Binary { op, .. } if is_comparison(op) || is_logic(op) => self.condition_size(expr) + TRUTH,
            // Of two numbers or constants the left one is put in a variable
            // first. Not measured: beside a variable it is read as it is.
            ExprKind::Binary { left, right, .. } => {
                let loaded = self.is_number(left) && right.as_deref().is_some_and(|right| self.is_number(right));
                OPERATOR + if loaded { STORE } else { 0 } + self.expression_size(left) + self.operand_size(right)
            }
            ExprKind::Unary { op, .. } if op == "!" => self.condition_size(expr) + TRUTH,
            // `a++` keeps the old value first.
            ExprKind::Unary { op, operand, postfix } if op == "++" || op == "--" => {
                STEP + if *postfix { STORE } else { 0 } + operand.as_deref().map_or(0, |operand| self.place_size(operand))
            }
            // Not measured: a sign on a number is taken for part of the number.
            ExprKind::Unary { op, operand: Some(operand), .. } => {
                let signs = (op == "-" || op == "~") && number_literal(operand).is_none();
                self.expression_size(operand) + if signs { OPERATOR } else { 0 }
            }
            ExprKind::Unary { operand: None, .. } => 0,
            ExprKind::Cast { operand, .. } => self.operand_size(operand),
            ExprKind::Call { callee, args } => return self.call_size(callee, args),
            ExprKind::Member { .. } => return self.member_size(expr),
            ExprKind::Index { object, index } => {
                let (object, ty) = self.sized(object);
                return (object + self.operand_size(index) + ELEMENT, ty.map(|ty| ty.element()).filter(|ty| !ty.is_unknown()));
            }
            ExprKind::New { args, .. } => NEW + self.passed_sizes(args),
            // A new array and a call to add each item.
            ExprKind::List { items } => NEW + CALL * items.len() + self.passed_sizes(items),
        };
        (size, None)
    }

    fn operand_size(&self, expr: &Option<Box<Expr>>) -> usize {
        expr.as_deref().map_or(0, |expr| self.expression_size(expr))
    }

    // What is written to: an element is not read first.
    fn place_size(&self, place: &Expr) -> usize {
        match &place.kind {
            ExprKind::Index { object, index } => self.expression_size(object) + self.operand_size(index),
            _ => self.expression_size(place),
        }
    }

    // A value handed on as it is: an argument or an item of a list. A member
    // read through an object is whole here, and a member of this class or
    // `null` is put in a variable first.
    fn passed_size(&self, expr: &Expr) -> usize {
        let held = self.reads_member(expr) || self.names_own_member(expr) || expr.leaf_text() == Some("null");
        self.expression_size(expr) + if held { STORE } else { 0 }
    }

    // A member the class itself declares, not `static` or `const`, named
    // bare in one of its methods. Not measured, and left out: one it inherits.
    fn own_member(&self, name: &str) -> Option<&Member> {
        if self.static_method || self.locals.find(name).is_some() {
            return None;
        }
        let class = self.world.chain(self.class.as_deref()?).into_iter().next()?;
        let member = class.member(name)?;
        (!member.has("static") && !member.has("const")).then_some(member)
    }

    // The arguments of a call. One handed to an `out` parameter is written,
    // not read.
    fn argument_sizes(&self, found: &[&Method], args: &[Expr]) -> usize {
        let written = |index: usize| {
            let param = found.first().and_then(|method| method.params.get(index));
            param.is_some_and(|param| param.modifiers.iter().any(|modifier| modifier == "out" || modifier == "inout"))
        };
        let size = |(index, arg): (usize, &Expr)| if written(index) { self.expression_size(arg) } else { self.passed_size(arg) };
        args.iter().enumerate().map(size).sum()
    }

    fn names_own_member(&self, expr: &Expr) -> bool {
        expr.leaf_text().is_some_and(|name| self.own_member(name).is_some())
    }

    pub(super) fn over_member_static_array(&self, iter: Option<&Expr>) -> bool {
        let member = iter.and_then(|iter| iter.leaf_text()).and_then(|name| self.own_member(name));
        member.is_some_and(|member| matches!(member.ty, Ty::Array(_)))
    }

    fn passed_sizes(&self, exprs: &[Expr]) -> usize {
        exprs.iter().map(|expr| self.passed_size(expr)).sum()
    }

    fn is_number(&self, expr: &Expr) -> bool {
        number_literal(expr).is_some() || expr.leaf_text().is_some_and(|name| self.names_constant(name))
    }

    fn reads_member(&self, expr: &Expr) -> bool {
        matches!(expr.kind, ExprKind::Member { .. }) && self.member_steps(expr).0 > 0
    }

    // A chain of `.name`: four a step through an object, less the two an
    // operator saves. A name read off a type is no step, nor are those the
    // game refuses as too complex.
    fn member_size(&self, expr: &Expr) -> (usize, Option<Ty>) {
        let mut names = Vec::new();
        let mut base = expr;
        while let ExprKind::Member { object, name } = &base.kind {
            names.push(name.as_str());
            base = object;
        }
        let type_name = base.leaf_text().filter(|name| self.names_type(name));
        let (reached, mut ty) = match type_name {
            Some(name) => (0, Some(Ty::Named(name.to_string()))),
            None => self.object_size(base),
        };
        for name in names.iter().rev().take(MEMBER_STEPS + 1) {
            ty = ty.and_then(|ty| self.member_type(&ty, name));
        }
        let steps = names.len().min(MEMBER_STEPS) - usize::from(type_name.is_some());
        ((MEMBER * steps).saturating_sub(STORE) + reached, ty)
    }

    // What a chain starts from, as the object its first name is read off: a
    // member of this class is read first.
    fn object_size(&self, base: &Expr) -> (usize, Option<Ty>) {
        let (size, ty) = self.sized(base);
        (size + off_call(base) + if self.names_own_member(base) { STORE } else { 0 }, ty)
    }

    // The `.name` steps of a chain that go through an object, and what the
    // chain starts from.
    fn member_steps<'e>(&self, expr: &'e Expr) -> (usize, &'e Expr) {
        let mut steps = 0;
        let mut base = expr;
        while let ExprKind::Member { object, .. } = &base.kind {
            steps += 1;
            base = object;
        }
        let off_a_type = base.leaf_text().is_some_and(|name| self.names_type(name));
        (steps.min(MEMBER_STEPS).saturating_sub(usize::from(off_a_type)), base)
    }

    fn call_size(&self, callee: &Expr, args: &[Expr]) -> (usize, Option<Ty>) {
        match &callee.kind {
            ExprKind::Leaf { text } => {
                let methods = self.class.as_deref().map(|class| self.world.methods_of(class, text)).unwrap_or_default();
                let found = match methods.is_empty() {
                    true => self.world.functions.get(text).map(|found| found.iter().collect()).unwrap_or_default(),
                    false => methods,
                };
                (call_cost(&found, false, args.len()) + self.argument_sizes(&found, args), returned(&found))
            }
            ExprKind::Member { object, name } => {
                if let Some(class) = object.leaf_text().filter(|text| self.names_type(text)) {
                    return match (name.as_str(), args.len()) {
                        ("Cast", 1) => (CAST + self.passed_sizes(args), Some(Ty::Named(class.to_string()))),
                        ("CastTo", 2) => (CAST_TO + self.expression_size(&args[0]) + self.passed_size(&args[1]), None),
                        _ => {
                            let found = self.world.methods_of(class, name);
                            (call_cost(&found, false, args.len()) + self.argument_sizes(&found, args), returned(&found))
                        }
                    };
                }
                let (receiver, ty) = self.receiver_size(object);
                let class = ty.as_ref().and_then(|ty| self.class_named(ty));
                let found = class.map(|class| self.world.methods_of(class, name)).unwrap_or_default();
                // Measured on these three; any other receiver is free.
                let on_value = matches!(&ty, Some(Ty::Named(name)) if matches!(name.as_str(), "string" | "vector" | "int"));
                let result = match (&ty, class, found.first()) {
                    (Some(ty), Some(class), Some(method)) => Some(self.world.return_for(ty, class, method)).filter(is_value),
                    _ => None,
                };
                (call_cost(&found, on_value, args.len()) + receiver + self.argument_sizes(&found, args), result)
            }
            _ => (CALL + self.expression_size(callee) + self.passed_sizes(args), None),
        }
    }

    // What a method is called on: a member is read first.
    fn receiver_size(&self, object: &Expr) -> (usize, Option<Ty>) {
        let (size, ty) = self.sized(object);
        let held = self.reads_member(object) || self.names_own_member(object);
        (size + off_call(object) + if held { STORE } else { 0 }, ty)
    }

    // `place = value`.
    fn store_size(&self, place: &Expr, value: &Expr) -> usize {
        match &place.kind {
            ExprKind::Member { .. } => {
                let (steps, base) = self.member_steps(place);
                if steps == 0 {
                    return self.kept_size(None, value);
                }
                self.object_size(base).0 + MEMBER * (steps - 1) + STORE + self.kept_size(None, value)
            }
            ExprKind::Index { .. } => self.place_size(place) + self.kept_size(None, value),
            _ => self.expression_size(place) + self.kept_size(place.leaf_text(), value),
        }
    }

    // A value and its store into a variable, an element, a member or what
    // the function returns. An arithmetic operator and a test write there
    // themselves.
    fn kept_size(&self, place: Option<&str>, value: &Expr) -> usize {
        let size = self.expression_size(value);
        match &value.kind {
            ExprKind::Binary { op, .. } if op == "=" || is_assignment(op) => size + STORE,
            ExprKind::Binary { op, .. } if is_comparison(op) || is_logic(op) => size,
            ExprKind::Binary { .. } if place.is_some_and(|name| in_place(name, value)) => STEP,
            ExprKind::Binary { .. } => size,
            ExprKind::Unary { op, .. } if op == "-" => size.max(STORE),
            ExprKind::Index { object, index } if self.read_in_place(object, index) => size,
            _ => size + STORE,
        }
    }

    // Whether an element is read straight into where it is kept: one of a
    // static array or a `vector` with a name for its index. Not measured: so
    // is one of a value whose type the names do not give.
    fn read_in_place(&self, object: &Expr, index: &Option<Box<Expr>>) -> bool {
        let numbered = index.as_deref().is_some_and(|index| number_literal(index).is_some());
        match self.sized(object).1 {
            Some(ty) if is_array_class(&ty) => false,
            Some(Ty::Array(_)) => !numbered,
            Some(Ty::Named(name)) if name == "vector" => !numbered,
            _ => true,
        }
    }

    fn names_constant(&self, name: &str) -> bool {
        self.world.constants.contains_key(name) && self.locals.find(name).is_none()
    }

    fn names_type(&self, name: &str) -> bool {
        self.locals.find(name).is_none() && self.world.types.is_known(name)
    }

    fn leaf_type(&self, name: &str) -> Option<Ty> {
        if let Some(local) = self.locals.find(name) {
            return Some(local.ty.clone()).filter(is_value);
        }
        let own = self.class.as_deref().and_then(|class| match name {
            "this" => Some(Ty::Named(class.to_string())),
            "super" => self.world.base_type(class),
            _ => self.world.member_of(class, name).map(|member| member.ty.clone()),
        });
        own.or_else(|| self.world.globals.get(name).map(|global| global.ty.clone())).filter(is_value)
    }

    fn member_type(&self, of: &Ty, name: &str) -> Option<Ty> {
        let class = self.class_named(of)?;
        self.world.member_for(of, class, name).map(|member| member.ty).filter(is_value)
    }

    fn class_named<'t>(&'t self, ty: &'t Ty) -> Option<&'t str> {
        match ty {
            Ty::Named(name) | Ty::Container(name, _) => self.world.class_of(name),
            _ => None,
        }
    }
}

// What a call compiles to, its arguments and what it is called on aside. One
// written in script is two, and one more for every second argument (measured
// to six). An engine one is one, and one more for every second argument
// starting with the first (measured to three); a `string`, a `vector` or an
// `int` it is called on counts as an argument. Not measured: a name nothing
// declares is taken for an engine one, the cheaper.
fn call_cost(found: &[&Method], on_value: bool, arguments: usize) -> usize {
    if !found.is_empty() && found.iter().all(|method| method.body) {
        return CALL + arguments / 2;
    }
    let has = |word: &str| found.first().is_some_and(|method| method.modifiers.iter().any(|modifier| modifier == word));
    STEP + (arguments + usize::from(on_value)).div_ceil(2) + if has("owned") && has("external") { OWNED } else { 0 }
}

// `a = b` or `a += b`, as a value.
fn is_store(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Binary { op, .. } if is_assignment(op))
}

fn has_case(body: &Option<Box<Item>>) -> bool {
    matches!(body.as_deref(), Some(Item::Block(block)) if block.items.iter().any(|item| matches!(item, Item::Stmt(Stmt::Case { .. }))))
}

// What using the result of a call as an object adds.
fn off_call(base: &Expr) -> usize {
    let ExprKind::Call { callee, .. } = &base.kind else {
        return 0;
    };
    match &callee.kind {
        ExprKind::Member { object, .. } if matches!(object.kind, ExprKind::Call { .. }) => OFF_CHAINED_CALL,
        _ => OFF_CALL,
    }
}

fn returned(found: &[&Method]) -> Option<Ty> {
    found.first().map(|method| method.ret.clone()).filter(is_value)
}

fn is_value(ty: &Ty) -> bool {
    !matches!(ty, Ty::Unknown | Ty::Void)
}

// `v = v + 1`, `v = 1 + v` and `v = v - 1` are a single instruction, and no
// other operator with a `1` is.
fn in_place(name: &str, value: &Expr) -> bool {
    let ExprKind::Binary { op, left, right: Some(right) } = &value.kind else {
        return false;
    };
    let is = |side: &Expr, text: &str| side.leaf_text() == Some(text);
    match op.as_str() {
        "+" => (is(left, name) && is(right, "1")) || (is(left, "1") && is(right, name)),
        "-" => is(left, name) && is(right, "1"),
        _ => false,
    }
}

fn is_array_class(ty: &Ty) -> bool {
    matches!(ty, Ty::Container(name, _) if name == "array")
}

fn is_comparison(op: &str) -> bool {
    matches!(op, "==" | "!=" | "<" | ">" | "<=" | ">=")
}

fn is_logic(op: &str) -> bool {
    matches!(op, "&&" | "||")
}
