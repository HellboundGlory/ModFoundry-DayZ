use crate::parser::{Expr, FunctionDecl, Item, Stmt, VarDecl};

use super::conversions::{Dest, Kind, Verdict};
use super::operators::place_name;
use super::scope::{base_name, Ty};
use super::{plain_class, BodyChecker};

// A loop or a `switch` the statement being read sits in, with the `break`s
// seen inside it so far.
pub(super) struct Enclosing {
    is_loop: bool,
    breaks: usize,
}

// How many `break`s the game keeps waiting at once.
const PENDING_BREAKS: usize = 256;

impl BodyChecker<'_> {
    // The game looks at the shape of a non-void function before it reads the
    // body: a `return` among the top-level statements, or an `if` whose two
    // branches each hold one. A body that fails is reported once and skipped.
    pub(super) fn lacks_return(&mut self, decl: &FunctionDecl, items: &[Item]) -> bool {
        if !self.returns_value || items.is_empty() || returns(items) {
            return false;
        }
        let name = match &self.class {
            Some(class) => format!("{}.{}", self.class_label(plain_class(class)), decl.name),
            None => decl.name.clone(),
        };
        self.statement_line = decl.line;
        self.error(format!("No return statement in function returning non-void '{name}'"));
        true
    }

    pub(super) fn return_statement(&mut self, value: Option<&Expr>) {
        match value {
            Some(value) if self.returns_value => {
                let wanted = self.return_type.clone();
                let ty = self.typed_in(value, Some(&wanted));
                self.store(value, &ty, &wanted, Dest::Variable("#return"));
            }
            Some(value) => {
                self.typed_in(value, None);
                self.error("return-statement with a value, in function returning 'void'".into());
            }
            None if self.returns_value => self.error("Function must return value".into()),
            None => {}
        }
    }

    pub(super) fn enter(&mut self, is_loop: bool) {
        self.enclosing.push(Enclosing { is_loop, breaks: 0 });
    }

    pub(super) fn leave(&mut self) {
        self.enclosing.pop();
    }

    // `break` needs a loop or a `switch` around it, `continue` a loop.
    pub(super) fn jump(&mut self, keyword: &str) {
        // The `break`s waiting for their loop or `switch` to end, in every
        // scope still open.
        let pending: usize = self.enclosing.iter().map(|scope| scope.breaks).sum();
        let placed = match keyword {
            "break" => match self.enclosing.last_mut() {
                Some(scope) => {
                    scope.breaks += 1;
                    if pending >= PENDING_BREAKS {
                        self.error("Too many breaks per scope".into());
                    }
                    true
                }
                None => false,
            },
            "continue" => self.enclosing.iter().any(|scope| scope.is_loop),
            _ => true,
        };
        if !placed {
            self.error("Misplaced break/case/continue".into());
        }
    }

    // A `case` reached from a body above it that neither returned nor sits
    // after a `break`: the game counts the `break`s still open in this switch
    // and everything around it, so only the cases before the first one are held
    // to it. The line is the last statement read, not the `case`.
    pub(super) fn switch_body(&mut self, body: &Option<Box<Item>>) {
        let Some(Item::Block(block)) = body.as_deref() else {
            self.branch(body);
            return;
        };
        let mut falls_through = false;
        for item in &block.items {
            match item {
                Item::Stmt(Stmt::Case { .. }) => {
                    let breaks: usize = self.enclosing.iter().map(|scope| scope.breaks).sum();
                    if falls_through && breaks == 0 {
                        self.error("Switch: fallthrough cases can't have a body".into());
                    }
                    falls_through = false;
                }
                Item::Stmt(Stmt::Default { .. }) => falls_through = false,
                _ => falls_through = !closes_case(item),
            }
            self.item(item);
        }
    }

    // What a `foreach` hands its variables: a map its key then its value, any
    // other iterable an index then its element.
    pub(super) fn foreach(&mut self, variables: &[VarDecl], iter: Option<&Expr>) {
        let iter_ty = iter.map(|iter| self.typed_in(iter, None)).unwrap_or_default();
        // The game does not walk a static array that a member of the class
        // holds, and has a name of its own for it.
        let refused = match (self.kind(&iter_ty), iter.and_then(place_name)) {
            _ if self.over_member_static_array(iter) => Some("#array"),
            (Kind::Class(class), Some(name)) if !self.is_iterable(&class) => Some(name),
            _ => None,
        };
        if let Some(name) = refused {
            self.error(format!("'{name}' is not array"));
            // Its variables are declared all the same.
            for variable in variables {
                self.local_with(variable, Ty::Unknown);
            }
            return;
        }
        let int = Ty::Named("int".into());
        let (key, value) = match (&iter_ty, self.container(&iter_ty)) {
            (Ty::Array(element), _) => (int, (**element).clone()),
            (_, Some(Ty::Container(name, arguments))) if name == "map" => (
                arguments.first().cloned().unwrap_or_default(),
                arguments.get(1).cloned().unwrap_or_default(),
            ),
            (_, Some(container)) => (int, container.element()),
            _ => (Ty::Unknown, Ty::Unknown),
        };
        for (index, variable) in variables.iter().enumerate() {
            let yielded = match index == 0 && variables.len() > 1 {
                true => key.clone(),
                false => value.clone(),
            };
            let declared = self.ty_of_decl(variable);
            if self.convert(&yielded, &declared) == Verdict::Incompatible {
                self.statement_line = variable.line;
                self.error(format!("Incompatible parameter '{}'", variable.name));
            }
            self.local_with(variable, yielded);
        }
    }

    // A class the game can walk: one built on a container, or one whose
    // ancestry the scripts do not show in full.
    fn is_iterable(&self, class: &str) -> bool {
        self.world.reaches_engine_base(class)
            || self
                .world
                .chain(class)
                .iter()
                .any(|info| matches!(base_name(&info.name), "array" | "set" | "map"))
    }

    pub(super) fn delete(&mut self, value: &Expr) {
        let ty = self.typed_in(value, None);
        if matches!(self.kind(&ty), Kind::Int | Kind::Float | Kind::Bool | Kind::Str | Kind::Vector) {
            self.error("Incompatible parameter 'on delete'".into());
        }
    }
}

fn returns(items: &[Item]) -> bool {
    items.iter().any(statement_returns)
}

// A statement the game counts as a `return`: one itself, or an `if` whose
// every branch holds one, down to a closing `else`. The ladder is walked in a
// loop, so its length costs no stack.
fn statement_returns(item: &Item) -> bool {
    let mut item = item;
    loop {
        match item {
            Item::Stmt(Stmt::Return { .. }) => return true,
            Item::Stmt(Stmt::If { then, otherwise, .. }) if branch_returns(then) => match otherwise.as_deref() {
                Some(Item::Block(block)) => return returns(&block.items),
                Some(next) => item = next,
                None => return false,
            },
            _ => return false,
        }
    }
}

fn branch_returns(branch: &Option<Box<Item>>) -> bool {
    match branch.as_deref() {
        Some(Item::Block(block)) => returns(&block.items),
        Some(item) => statement_returns(item),
        None => false,
    }
}

// Whether a case body's last statement leaves the switch by returning.
fn closes_case(item: &Item) -> bool {
    match item {
        Item::Block(block) => block.items.last().is_some_and(closes_case),
        _ => statement_returns(item),
    }
}
