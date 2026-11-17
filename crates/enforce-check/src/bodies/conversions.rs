use crate::parser::{Expr, ExprKind};

use super::scope::{base_name, is_value_type, Ty};
use super::{plain_class, BodyChecker};

// What a type is to the conversion table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Int,
    Float,
    Bool,
    Str,
    Vector,
    Typename,
    Func,
    Void,
    Null,
    Class(String),
    StaticArray,
    // A type nothing is known about: every rule stays silent on it.
    Unknown,
}

impl Kind {
    pub(super) fn is_number(&self) -> bool {
        matches!(self, Kind::Int | Kind::Float | Kind::Bool)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Fits,
    Incompatible,
    Unrelated,
    Downcast,
}

// Where a value is going, which words the lines the game prints about it.
#[derive(Clone, Copy)]
pub(super) enum Dest<'a> {
    Variable(&'a str),
    Argument { index: usize, function: &'a str },
}

impl BodyChecker<'_> {
    pub(super) fn kind(&self, ty: &Ty) -> Kind {
        match ty {
            Ty::Unknown => Kind::Unknown,
            Ty::Void => Kind::Void,
            Ty::Null => Kind::Null,
            Ty::Typename(_) => Kind::Typename,
            Ty::Array(_) => Kind::StaticArray,
            Ty::Named(name) | Ty::Container(name, _) => self.named_kind(name),
        }
    }

    fn named_kind(&self, name: &str) -> Kind {
        let name = base_name(name);
        if self.is_template_param(name) || self.world.handles.contains(name) {
            return Kind::Unknown;
        }
        if let Some(kind) = value_kind(name) {
            return kind;
        }
        let Some(class) = self.world.class_of(name) else {
            return Kind::Unknown;
        };
        if let Some(kind) = value_kind(class) {
            return kind;
        }
        let Some(info) = self.world.info(class) else {
            return Kind::Unknown;
        };
        if info.is_enum {
            return Kind::Int;
        }
        // A class built on a value type is that value with methods added; which
        // rows the game applies to it is not known.
        if self.world.chain(class).iter().any(|info| is_value_type(&info.name)) {
            return Kind::Unknown;
        }
        Kind::Class(class.to_string())
    }

    pub(super) fn convert(&self, from: &Ty, to: &Ty) -> Verdict {
        match (self.kind(to), self.kind(from)) {
            (Kind::Unknown | Kind::Void | Kind::StaticArray, _)
            | (_, Kind::Unknown | Kind::Void | Kind::StaticArray | Kind::Null) => Verdict::Fits,
            (dest, source) if dest.is_number() && source.is_number() => Verdict::Fits,
            (Kind::Func, Kind::Int) => Verdict::Fits,
            (Kind::Class(dest), Kind::Class(source)) => self.class_verdict(&source, &dest),
            // Whether a value type passes where a root class is declared is
            // not known.
            (Kind::Class(dest), _) if is_root(&dest) => Verdict::Fits,
            (dest, source) if dest == source => Verdict::Fits,
            _ => Verdict::Incompatible,
        }
    }

    fn class_verdict(&self, source: &str, dest: &str) -> Verdict {
        // Every script class is reference counted, so each passes as `Managed`
        // whether or not it names that base.
        if is_root(dest) || plain_class(source) == plain_class(dest) {
            return Verdict::Fits;
        }
        if self.derives(source, dest) {
            return Verdict::Fits;
        }
        if self.derives(dest, source) {
            return Verdict::Downcast;
        }
        // An ancestry that runs into a class no script declares may meet the
        // other class there.
        if self.world.reaches_engine_base(source) || self.world.reaches_engine_base(dest) {
            return Verdict::Fits;
        }
        Verdict::Unrelated
    }

    fn derives(&self, class: &str, ancestor: &str) -> bool {
        self.world
            .chain(class)
            .iter()
            .any(|info| plain_class(&info.name) == plain_class(ancestor))
    }

    // What the value's own shape rules out for this destination, whatever its
    // operands are.
    fn shape_error(&self, value: &Expr, dest_kind: &Kind, dest_ty: &Ty) -> Option<String> {
        let text_like = matches!(dest_kind, Kind::Str | Kind::Vector | Kind::Typename)
            || matches!(dest_kind, Kind::Class(name) if !is_root(name));
        match &value.kind {
            // A comparison or a logical step yields a number.
            ExprKind::Binary { op, .. }
                if text_like && matches!(op.as_str(), "==" | "!=" | "<" | ">" | "<=" | ">=" | "&&" | "||") =>
            {
                Some("Can't condition to non numeric type".to_string())
            }
            ExprKind::Unary { op, .. } if op == "~" && *dest_kind == Kind::Float => Some(format!(
                "'~' : illegal on operands of type '{}'",
                dest_ty.text()
            )),
            _ => None,
        }
    }

    fn names_method(&self, value: &Expr) -> bool {
        match &value.kind {
            ExprKind::Leaf { text } => self.is_method(text),
            ExprKind::Member { .. } => true,
            _ => false,
        }
    }

    // `(Type)value` between two classes takes the class rows of an assignment
    // and none of the rows that name a destination. False when a row failed it.
    pub(super) fn cast_fits(&mut self, value_ty: &Ty, dest_ty: &Ty) -> bool {
        if !matches!((self.kind(value_ty), self.kind(dest_ty)), (Kind::Class(_), Kind::Class(_))) {
            return true;
        }
        self.class_rows(self.convert(value_ty, dest_ty), value_ty, dest_ty)
    }

    // The lines a class value prints on its way into another class. False when
    // the value did not get there.
    fn class_rows(&mut self, verdict: Verdict, value_ty: &Ty, dest_ty: &Ty) -> bool {
        let (source, target) = (value_ty.text(), dest_ty.text());
        match verdict {
            Verdict::Downcast => {
                self.soft(format!("Unsafe down-casting, use '{target}.Cast' for safe down-casting"));
                // As a warning the line stands alone; as an error the rows
                // after it print too.
                if !self.strict {
                    return true;
                }
                self.error(format!("Types '{source}' and '{target}' are unrelated"));
                false
            }
            Verdict::Unrelated => {
                self.error(format!("Types '{source}' and '{target}' are unrelated"));
                false
            }
            Verdict::Incompatible => false,
            Verdict::Fits => true,
        }
    }

    // A parameter written `T name[N]` takes a static array of `T`, of any
    // size. Anything else is named and refused; an array of another type is
    // refused without being named.
    fn sized_argument(&mut self, value: &Expr, value_ty: &Ty, element: &Ty, index: usize, function: &str) {
        // An array of `void` takes anything, a dynamic array included.
        let wanted = self.kind(element);
        if matches!(wanted, Kind::Unknown | Kind::Void) {
            return;
        }
        if let Some(text) = signed_number(value) {
            self.error(format!("Incompatible parameter '{text}'"));
            return;
        }
        match value_ty {
            Ty::Array(given) => {
                let given = self.kind(given);
                if given == wanted || given == Kind::Unknown {
                    return;
                }
                // An array of a derived class fits too.
                if let (Kind::Class(given), Kind::Class(wanted)) = (&given, &wanted)
                    && self.derives(given, wanted)
                {
                    return;
                }
            }
            _ if matches!(self.kind(value_ty), Kind::Unknown | Kind::Void | Kind::Null) => return,
            _ => {
                let name = match &value.kind {
                    ExprKind::Leaf { text } if !is_string_literal(value) => text.as_str(),
                    ExprKind::Call { .. } => "#return",
                    _ => "(null)",
                };
                self.error(format!("Incompatible parameter '{name}'"));
            }
        }
        let (source, target) = (value_ty.text(), element.text());
        self.error(format!("Cannot convert '{source}' to '{target}' for argument '{index}' to method '{function}'"));
    }

    // An array declared in the function body, of a class written in script
    // from the roots up, is named and refused by a parameter of that class or
    // of an ancestor, or of an array of one. A member and a global pass, and
    // so does an array of a class with an engine class among its ancestors
    // (vanilla's `ItemBase`).
    fn script_class_local_array(&mut self, value: &Expr, value_ty: &Ty, dest_ty: &Ty, index: usize, function: &str) -> bool {
        let (ExprKind::Leaf { text }, Ty::Array(given)) = (&value.kind, value_ty) else {
            return false;
        };
        let wanted = match dest_ty {
            Ty::Array(element) => element.as_ref(),
            other => other,
        };
        if !self.locals.declared_in_body(text) || !matches!((given.as_ref(), wanted), (Ty::Named(_), Ty::Named(_))) {
            return false;
        }
        let (Kind::Class(given_class), Kind::Class(wanted_class)) = (self.kind(given), self.kind(wanted)) else {
            return false;
        };
        let chain = self.world.chain(&given_class);
        let engine_built = chain.iter().any(|info| info.early && !is_root(&info.name));
        if is_root(&wanted_class) || engine_built || !self.derives(&given_class, &wanted_class) {
            return false;
        }
        self.error(format!("Incompatible parameter '{text}'"));
        let (source, target) = (given.text(), wanted.text());
        self.error(format!("Cannot convert '{source}' to '{target}' for argument '{index}' to method '{function}'"));
        true
    }

    // A value stored into a typed place: a declared variable, the left side of
    // `=`, or a parameter.
    pub(super) fn store(&mut self, value: &Expr, value_ty: &Ty, dest_ty: &Ty, dest: Dest<'_>) {
        if let Dest::Argument { index, function } = dest
            && self.script_class_local_array(value, value_ty, dest_ty, index, function)
        {
            return;
        }
        if let (Ty::Array(element), Dest::Argument { index, function }) = (dest_ty, dest) {
            return self.sized_argument(value, value_ty, element, index, function);
        }
        let dest_kind = self.kind(dest_ty);
        if let Some(text) = number_literal(value) {
            // A number literal is built directly as the type wanted, which only
            // the number types can be.
            if matches!(dest_kind, Kind::Str | Kind::Vector | Kind::Typename | Kind::Class(_)) {
                self.error(format!("Incompatible parameter '{text}'"));
            }
            return;
        }
        if let Some(message) = self.shape_error(value, &dest_kind, dest_ty) {
            self.error(message);
            return;
        }
        // An element of a `vector` read with a local's name for its index does
        // not go into an `int`. Not measured: what a function returns.
        if let (ExprKind::Index { object, index }, Dest::Variable(name), Kind::Int) = (&value.kind, dest, &dest_kind)
            && name != "#return"
            && index.as_deref().and_then(Expr::leaf_text).is_some_and(|index| self.locals.find(index).is_some())
        {
            let object_ty = self.quiet(|checker| checker.typed_in(object, None));
            if self.kind(&object_ty) == Kind::Vector {
                self.error(format!("Incompatible parameter '{name}'"));
                return;
            }
        }
        // `"1 2 3"` is read as the three numbers of a vector.
        if dest_kind == Kind::Vector && is_string_literal(value) {
            return;
        }
        // A method named where a `func` is wanted is the callback built from it,
        // even when a field shares the name.
        if dest_kind == Kind::Func && self.names_method(value) {
            return;
        }
        // Two vectors multiplied into a float are their dot product.
        if dest_kind == Kind::Float
            && self.kind(value_ty) == Kind::Vector
            && matches!(&value.kind, ExprKind::Binary { op, .. } if op == "*")
        {
            return;
        }
        let verdict = self.convert(value_ty, dest_ty);
        if self.class_rows(verdict, value_ty, dest_ty) {
            return;
        }
        let (source, target) = (value_ty.text(), dest_ty.text());
        match dest {
            Dest::Variable(name) => self.error(format!("Incompatible parameter '{name}'")),
            Dest::Argument { index, function } => self.error(format!(
                "Cannot convert '{source}' to '{target}' for argument '{index}' to method '{function}'"
            )),
        }
    }
}

fn is_root(class: &str) -> bool {
    matches!(class, "Class" | "Managed")
}

fn value_kind(name: &str) -> Option<Kind> {
    Some(match name {
        "int" => Kind::Int,
        "float" => Kind::Float,
        "bool" => Kind::Bool,
        "string" => Kind::Str,
        "vector" => Kind::Vector,
        "typename" => Kind::Typename,
        "func" => Kind::Func,
        "void" => Kind::Void,
        "auto" => Kind::Unknown,
        _ => return None,
    })
}

// The text of a number literal, as written.
pub(super) fn number_literal(expr: &Expr) -> Option<&str> {
    let ExprKind::Leaf { text } = &expr.kind else {
        return None;
    };
    text.starts_with(|c: char| c.is_ascii_digit()).then_some(text.as_str())
}

// A number literal with its sign, which the lexer reads as one word.
fn signed_number(expr: &Expr) -> Option<&str> {
    let text = expr.leaf_text()?;
    text.trim_start_matches('-').starts_with(|c: char| c.is_ascii_digit()).then_some(text)
}

pub(super) fn is_string_literal(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Leaf { text } if text.starts_with('"'))
}
