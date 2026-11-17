use crate::parser::{is_assignment, Expr, ExprKind};

use super::conversions::{number_literal, Dest, Kind};
use super::scope::Ty;
use super::BodyChecker;

// The source path the compiler names in its own failure line: the retail
// server is built on Linux, the Diag build on Windows.
const COMPILER_SOURCE: &str =
    "/data/jenkins-agent/workspace/_Continuous_branches_stable_0130/CodeSVN/Enfusion/src/script/enf_scriptcompiler.cpp";
const DIAG_COMPILER_SOURCE: &str =
    "D:\\jenkins\\cpu\\workspace\\_Continuous_branches_stable_0130\\CodeSVN\\Enfusion\\src\\script\\enf_scriptcompiler.cpp";

impl BodyChecker<'_> {
    // An expression used as a statement: the one place `=`, a compound
    // assignment or a bare `++` may stand.
    pub(super) fn expression_statement(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Binary { op, left, right } if op == "=" => self.assign(left, right.as_deref()),
            ExprKind::Binary { op, left, right } if is_assignment(op) => {
                self.compound(op, left, right.as_deref())
            }
            // A subscript whose value goes nowhere is not read at all.
            ExprKind::Index { object, index } => {
                self.typed_in(object, None);
                if let Some(index) = index {
                    self.typed_in(index, None);
                }
            }
            _ => {
                self.typed_in(expr, None);
            }
        }
    }

    fn assign(&mut self, left: &Expr, right: Option<&Expr>) {
        let left_ty = self.typed_in(left, None);
        let Some(right) = right else {
            return;
        };
        let right_ty = self.typed_in(right, Some(&left_ty));
        self.read_only(left, false);
        if let Some(name) = place_name(left) {
            self.store(right, &right_ty, &left_ty, Dest::Variable(name));
        }
        // A write past the end of a static array has nowhere to land.
        if let Some((name, _, _)) = self.past_the_end(left) {
            self.error(format!("Incompatible parameter '{name}'"));
        }
        self.strong_ref(left, right);
    }

    fn compound(&mut self, op: &str, left: &Expr, right: Option<&Expr>) {
        if matches!(left.kind, ExprKind::Index { .. }) {
            self.error(format!("Operator '{op}' not supported with array accessor [] (TODO)"));
            return;
        }
        let left_ty = self.typed_in(left, None);
        let right_ty = match right {
            Some(right) => self.typed_in(right, Some(&left_ty)),
            None => Ty::Unknown,
        };
        self.read_only(left, false);
        let (left_kind, right_kind) = (self.kind(&left_ty), self.kind(&right_ty));
        if (left_kind == Kind::StaticArray) != (right_kind == Kind::StaticArray) {
            if left_kind != Kind::Unknown && right_kind != Kind::Unknown {
                self.error("Mixed value and array".into());
            }
            return;
        }
        // The right side is first built as the left's type. A number literal
        // cannot be a string or a class; into a vector the game names the
        // temporary it made instead.
        if let Some(text) = right.and_then(number_literal) {
            let name = match left_kind {
                Kind::Str | Kind::Class(_) => Some(text),
                Kind::Vector if op == "/=" => Some("@v00"),
                _ => None,
            };
            if let Some(name) = name {
                self.error(format!("Incompatible parameter '{name}'"));
                return;
            }
        }
        // The compiler has no instruction for the pair and says so itself.
        if !defined_for(op.trim_end_matches('='), &left_kind) {
            let source = match self.strict {
                true => DIAG_COMPILER_SOURCE,
                false => COMPILER_SOURCE,
            };
            self.error(format!("Internal compiler error {source} : 11363"));
        }
    }

    pub(super) fn binary(&mut self, op: &str, left: &Expr, right: Option<&Expr>, expected: Option<&Ty>) -> Ty {
        if is_assignment(op) {
            // Inside an expression an assignment is not a value.
            let left_ty = self.typed_in(left, None);
            if let Some(right) = right {
                self.typed_in(right, Some(&left_ty));
            }
            if op == "=" {
                self.error("Unknown operator '='".into());
            }
            return Ty::Unknown;
        }
        match op {
            "==" | "!=" | "<" | ">" | "<=" | ">=" => self.comparison(left, right),
            "&&" | "||" => {
                self.condition(left);
                if let Some(right) = right {
                    self.condition(right);
                }
                Ty::Named("bool".into())
            }
            _ => self.arithmetic(op, left, right, expected),
        }
    }

    // A value read as true or false: anything but `void` has a zero to compare
    // against.
    pub(super) fn condition(&mut self, expr: &Expr) -> Ty {
        let ty = self.typed_in(expr, None);
        if ty == Ty::Void {
            self.error("Can't condition 'void' type".into());
        }
        ty
    }

    // Both sides are read with no expected type, so a type name on either is a
    // `typename` constant; the right side must then become the left's type.
    fn comparison(&mut self, left: &Expr, right: Option<&Expr>) -> Ty {
        let left_ty = self.condition(left);
        let bool_ty = Ty::Named("bool".into());
        let Some(right) = right else {
            return bool_ty;
        };
        let left_kind = self.kind(&left_ty);
        let right_ty = match left_kind {
            Kind::Vector => self.typed_in(right, Some(&left_ty)),
            _ => self.condition(right),
        };
        let right_kind = self.kind(&right_ty);
        if let Some(text) = number_literal(right) {
            // A number literal is built as the left side's type; beside a
            // vector it is the vector the game names.
            let name = match left_kind {
                Kind::Str | Kind::Typename | Kind::Class(_) => text,
                Kind::Vector => place_name(left).unwrap_or("(null)"),
                _ => return bool_ty,
            };
            self.error(format!("Incompatible parameter '{name}'"));
            return Ty::Unknown;
        }
        if value_mismatch(&left_kind, &right_kind) {
            // The left side is what the game names, and a literal has no name.
            let name = match number_literal(left) {
                Some(_) => Some("(null)"),
                None => place_name(left),
            };
            if let Some(name) = name {
                self.error(format!("Incompatible parameter '{name}'"));
                return Ty::Unknown;
            }
        }
        bool_ty
    }

    // The left side is read against the type the whole expression is wanted
    // as, the right side against the left's.
    fn arithmetic(&mut self, op: &str, left: &Expr, right: Option<&Expr>, expected: Option<&Ty>) -> Ty {
        let left_ty = self.typed_in(left, expected);
        let Some(right) = right else {
            return Ty::Unknown;
        };
        let right_ty = self.typed_in(right, Some(&left_ty));
        if self.divides_by_zero(op, left, right) {
            return Ty::Unknown;
        }
        let (left_kind, right_kind) = (self.kind(&left_ty), self.kind(&right_ty));
        if left_kind == Kind::Unknown || right_kind == Kind::Unknown {
            return Ty::Unknown;
        }
        if (left_kind == Kind::StaticArray) != (right_kind == Kind::StaticArray) {
            self.error("Mixed value and array".into());
            return Ty::Unknown;
        }
        // The right side becomes the left's type before the operator is looked
        // up, so a value that cannot is the line printed.
        if !self.operand_fits(op, left, &left_kind, right, &right_kind) {
            return Ty::Unknown;
        }
        if !defined_for(op, &left_kind) {
            self.error(format!("Unknown operator '{op}'"));
            return Ty::Unknown;
        }
        match (&left_kind, &right_kind) {
            (Kind::Str, _) => Ty::Named("string".into()),
            (Kind::Vector, _) | (_, Kind::Vector) => Ty::Named("vector".into()),
            (Kind::Float, _) | (_, Kind::Float) => Ty::Named("float".into()),
            (left, right) if left.is_number() && right.is_number() => Ty::Named("int".into()),
            _ => Ty::Unknown,
        }
    }

    // Whether the right operand can stand beside the left one, reporting it
    // when it cannot.
    fn operand_fits(&mut self, op: &str, left: &Expr, left_kind: &Kind, right: &Expr, right_kind: &Kind) -> bool {
        let literal = number_literal(right);
        let fits = match (left_kind, right_kind) {
            // A string takes any value through its `ToString` when the two are
            // joined; for any other operator a number literal is built as a
            // string, which it cannot be.
            (Kind::Str, _) => op == "+" || literal.is_none(),
            // A vector scales by a number, and a number by a vector.
            (Kind::Vector, kind) if kind.is_number() => op == "*",
            (kind, Kind::Vector) if kind.is_number() => op == "*",
            (kind, Kind::Str) if kind.is_number() => false,
            // A number literal cannot be built as a class, nor a class read as
            // a number.
            (Kind::Class(_), _) => literal.is_none(),
            (kind, Kind::Class(_)) if kind.is_number() => false,
            _ => true,
        };
        if fits {
            return true;
        }
        let class_on_the_right = matches!(right_kind, Kind::Class(_));
        if let Some(text) = literal {
            self.error(format!("Incompatible parameter '{text}'"));
        } else if let Some(name) = place_name(right).filter(|_| class_on_the_right) {
            self.error(format!("Incompatible parameter '{name}'"));
        } else if let Some(name) = place_name(left) {
            self.error(format!("Incompatible parameter '{name}'"));
        }
        false
    }

    pub(super) fn unary(&mut self, op: &str, operand: Option<&Expr>) -> Ty {
        let Some(operand) = operand else {
            return Ty::Unknown;
        };
        match op {
            "!" => {
                self.condition(operand);
                Ty::Named("bool".into())
            }
            "~" => {
                self.typed_in(operand, None);
                Ty::Named("int".into())
            }
            "++" | "--" => {
                // A subscript on a container and a call both yield a value
                // with no variable behind it.
                if self.is_temporary(operand) {
                    self.typed_in(operand, None);
                    self.error(format!("Operator '{op}' is not supported on temporary values (TODO)"));
                    return Ty::Unknown;
                }
                let ty = self.typed_in(operand, None);
                self.read_only(operand, true);
                ty
            }
            "-" => {
                let ty = self.typed_in(operand, None);
                if matches!(self.kind(&ty), Kind::Str | Kind::Class(_))
                    && let Some(name) = place_name(operand)
                {
                    self.error(format!("Bad type '{name}'"));
                    return Ty::Unknown;
                }
                ty
            }
            "+" => self.typed_in(operand, None),
            // Any other operator standing where an operand belongs.
            _ => {
                self.typed_in(operand, None);
                self.error("Syntax error".into());
                Ty::Unknown
            }
        }
    }

    // A call result, or what a subscript on a container hands back. An element
    // of a static array is a place of its own.
    fn is_temporary(&mut self, operand: &Expr) -> bool {
        match &operand.kind {
            ExprKind::Call { .. } => true,
            ExprKind::Index { object, .. } => {
                let object_ty = self.quiet(|checker| checker.typed_in(object, None));
                self.kind(&object_ty) != Kind::StaticArray
            }
            _ => false,
        }
    }

    // Reports a place that cannot be written: a `const` variable, or a number
    // literal, which has no variable behind it and so no name to print.
    // Observed for `K++`: the module loads after the error; `--` is assumed alike.
    pub(super) fn read_only(&mut self, place: &Expr, keeps_loading: bool) -> bool {
        let name = match &place.kind {
            ExprKind::Leaf { .. } if number_literal(place).is_some() => "(null)",
            ExprKind::Leaf { text } if self.variable(text).is_some_and(|variable| variable.has("const")) => text,
            _ => return false,
        };
        let message = format!("Trying to modify write protected variable '{name}'");
        match keeps_loading {
            true => self.error_keeps_loading(message),
            false => self.error(message),
        }
        true
    }
}

// The name the game prints for a place a value sits in: a variable, a literal
// as written, or the member a `.` reaches.
pub(super) fn place_name(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Leaf { text } => Some(text),
        ExprKind::Member { name, .. } => Some(name),
        _ => None,
    }
}

// A number beside a string or a vector, in either order: the table has no row
// between them.
fn value_mismatch(left: &Kind, right: &Kind) -> bool {
    let text_like = |kind: &Kind| matches!(kind, Kind::Str | Kind::Vector);
    (left.is_number() && text_like(right)) || (text_like(left) && right.is_number())
}

// Whether the operator exists for a left operand of this kind.
fn defined_for(op: &str, kind: &Kind) -> bool {
    match kind {
        Kind::Float => matches!(op, "+" | "-" | "*" | "/"),
        Kind::Vector => matches!(op, "+" | "-" | "*"),
        Kind::Str => op == "+",
        Kind::Class(_) => false,
        _ => true,
    }
}
