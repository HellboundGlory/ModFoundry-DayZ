use crate::parser::{Expr, ExprKind};

use super::conversions::Kind;
use super::scope::{base_name, Member, Method, Ty};
use super::{engine_names, plain_class, BodyChecker, Reach};

impl BodyChecker<'_> {
    // `expected` is the type the surrounding construct wants, which decides
    // whether a bare type name is a variable or a `typename` constant.
    pub(super) fn typed_in(&mut self, expr: &Expr, expected: Option<&Ty>) -> Ty {
        match &expr.kind {
            ExprKind::Leaf { text } => self.leaf(text, expected),
            ExprKind::DigitName { text } => {
                self.error(format!("Can't find variable '{text}'"));
                Ty::Unknown
            }
            ExprKind::Member { object, name } => self.member(object, name),
            ExprKind::Call { callee, args } => self.call(callee, args),
            ExprKind::Index { object, index } => {
                let ty = self.typed_in(object, None);
                if let Some(index) = index {
                    self.typed_in(index, None);
                }
                if let Some((name, size, at)) = self.past_the_end(expr) {
                    self.error(format!("Index out of range ({size} >= {at}) '{name}'"));
                }
                // A class deriving a container indexes into that container, so
                // the element comes from the base's arguments when the name
                // itself carries none.
                self.container_element(&ty)
            }
            ExprKind::Unary { op, operand, .. } => self.unary(op, operand.as_deref()),
            ExprKind::Binary { op, left, right } => self.binary(op, left, right.as_deref(), expected),
            ExprKind::New { type_, args } => {
                let ty = type_.as_ref().map_or(Ty::Unknown, |type_| self.ty_of_ref(type_));
                self.new_object(&ty, args);
                match self.instantiate(&ty) {
                    true => ty,
                    false => Ty::Unknown,
                }
            }
            ExprKind::Cast { type_, operand } => {
                let ty = self.ty_of_ref(type_);
                let Some(operand) = operand else {
                    return ty;
                };
                let operand_ty = self.typed_in(operand, None);
                match self.cast_fits(&operand_ty, &ty) {
                    true => ty,
                    false => Ty::Unknown,
                }
            }
            ExprKind::List { items } => {
                for item in items {
                    self.typed_in(item, None);
                }
                self.list_destination(expected);
                Ty::Unknown
            }
        }
    }

    // A name on its own: a literal, `this`, `super`, a local, a member, a
    // global, an enum value, or a type used as a value.
    fn leaf(&mut self, text: &str, expected: Option<&Ty>) -> Ty {
        match text {
            "this" | "super" => {
                // A static method has no instance: `this` where a typed value is
                // wanted is refused, while an engine `void` parameter takes it.
                if self.static_method && self.class.is_some() && expected.is_some_and(|ty| self.is_typed(ty)) {
                    self.error("'this' or 'super' inside static method is not valid".into());
                    self.error(format!("Can't find variable '{text}'"));
                    return Ty::Unknown;
                }
                return self.receiver(text);
            }
            "true" | "false" => return Ty::Named("bool".into()),
            "null" | "NULL" => return Ty::Null,
            _ => {}
        }
        if let Some(literal) = literal_type(text) {
            // `"1 2 3"` where a vector is wanted is that vector.
            if text.starts_with('"') && expected.is_some_and(|ty| self.kind(ty) == Kind::Vector) {
                return Ty::Named("vector".into());
            }
            return literal;
        }
        // With no type wanted, or `typename` wanted, a type name is the
        // `typename` constant even when a member or an enum value shares it.
        let type_first = self.type_name_position(expected)
            && self.locals.find(text).is_none()
            && self.world.types.is_known(text)
            // A class only declared ahead has no record to name yet.
            && !self.world.types.find(text).is_some_and(|symbol| symbol.forward);
        if type_first {
            return Ty::Typename(base_name(text).to_string());
        }
        if let Some(found) = self.lookup_member(text) {
            return found;
        }
        // A class derived from a primitive is written against that primitive's
        // implicit `value`: the instance the method was called on.
        if text == "value"
            && let Some(class) = self.class.clone()
            && self.derives_primitive(&class)
        {
            // The instance itself, so its own methods resolve as well as the
            // primitive's.
            return Ty::Named(class);
        }
        if let Some(global) = self.world.globals.get(text) {
            return global.ty.clone();
        }
        if self.world.enum_values.contains_key(text) {
            return Ty::Named("int".into());
        }
        // A method named as a value is the callback the game builds out of it.
        if self.is_method(text) {
            let wanted = expected.is_some_and(|ty| self.kind(ty) == Kind::Func);
            if wanted && !self.method_callback(text) {
                return Ty::Unknown;
            }
            return Ty::Named("func".into());
        }
        // A template parameter is a type the use site supplies, so a body that
        // names it has nothing to look up.
        if self.is_template_param(text) {
            return Ty::Unknown;
        }
        // The engine registers some constants and enum members before any script.
        // Nothing is known about their type, so nothing is said about its members
        // either.
        if engine_names::GLOBALS.contains(&text) {
            return Ty::Unknown;
        }
        if self.is_input(text) || self.is_widget_type_id(text) {
            return Ty::Named("int".into());
        }
        if expected.is_some_and(|ty| self.kind(ty) == Kind::Func) {
            // A class whose ancestry reaches one no script declares may hold
            // the method there.
            let hidden = self.class.as_deref().is_some_and(|class| self.world.reaches_engine_base(class));
            if !hidden {
                self.unknown_callback(text);
            }
            return Ty::Unknown;
        }
        self.error(format!("Can't find variable '{text}'"));
        Ty::Unknown
    }

    fn is_typed(&self, ty: &Ty) -> bool {
        !matches!(self.kind(ty), Kind::Void | Kind::Unknown)
    }

    // An input the engine registers as a number under its name: one of its
    // own, which all start with `UA` and a capital, or one a loaded mod lists.
    fn is_input(&self, name: &str) -> bool {
        let engine = name.strip_prefix("UA").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()));
        engine || self.world.input_names.contains(name)
    }

    // The engine gives every widget class a number named after it.
    fn is_widget_type_id(&self, name: &str) -> bool {
        name.strip_suffix("TypeID")
            .is_some_and(|class| self.world.chain(class).iter().any(|info| info.name == "Widget"))
    }

    // A literal subscript at or past the declared size of a local static
    // array: its name, the size and the subscript.
    pub(super) fn past_the_end<'e>(&self, expr: &'e Expr) -> Option<(&'e str, i64, i64)> {
        let ExprKind::Index { object, index } = &expr.kind else {
            return None;
        };
        let name = object.leaf_text()?;
        let size = self.locals.size(name)?;
        let at = super::literal_int(index.as_deref()?)?;
        (at >= size).then_some((name, size, at))
    }

    // `{...}` builds an `array`, so it needs one to land in: a class built on
    // anything else is named, and a place with no type at all is not.
    fn list_destination(&mut self, expected: Option<&Ty>) {
        let Some(dest) = expected else {
            return;
        };
        match self.kind(dest) {
            Kind::Void => self.error("Array initialization of non-array type".into()),
            Kind::Class(class) => {
                let array = matches!(self.container(dest), Some(Ty::Container(name, _)) if name == "array");
                if !array && !self.world.reaches_engine_base(&class) {
                    self.error(format!("Array initialization of non-array type '{}'", dest.text()));
                }
            }
            _ => {}
        }
    }

    // The `array`, `set` or `map` a type is, names through an alias, or derives
    // from, with the arguments as written there.
    pub(super) fn container(&self, ty: &Ty) -> Option<Ty> {
        let mut current = ty.expanded(self.world);
        for _ in 0..16 {
            if matches!(&current, Ty::Container(name, _) if matches!(name.as_str(), "array" | "set" | "map")) {
                return Some(current);
            }
            current = self.world.base_type(current.base()?)?.expanded(self.world);
        }
        None
    }

    // What a subscript on this type yields, following a class that derives a
    // container to the container's own arguments.
    fn container_element(&self, ty: &Ty) -> Ty {
        let expanded = ty.expanded(self.world);
        let element = expanded.element();
        if !element.is_unknown() {
            return element;
        }
        let Some(name) = ty.base() else {
            return Ty::Unknown;
        };
        // The nearest ancestor that is a container supplies its arguments.
        for info in self.world.chain(name) {
            if matches!(info.name.as_str(), "array" | "set" | "map") {
                return Ty::Unknown;
            }
            if let Some(base) = &info.base {
                let base_ty = Ty::named(base);
                let inherited = base_ty.expanded(self.world).element();
                if !inherited.is_unknown() {
                    return inherited;
                }
            }
        }
        Ty::Unknown
    }

    // Whether a class derives from a primitive, following the base chain and
    // any alias.
    fn derives_primitive(&self, class: &str) -> bool {
        let mut current = self.world.class_of(class).map(str::to_string);
        for _ in 0..16 {
            let Some(name) = current else {
                return false;
            };
            if super::is_primitive(&name) {
                return true;
            }
            current = self.world.base_of(&name);
        }
        false
    }

    // A member the engine fills in rather than a script declaration.
    fn pre_populated(&self, class: &str, name: &str) -> bool {
        engine_names::PREPOPULATED.iter().any(|(owner, members)| {
            base_name(class) == *owner && members.contains(&name)
        })
    }

    pub(super) fn is_method(&self, name: &str) -> bool {
        self.world.functions.contains_key(name)
            || self
                .class
                .as_ref()
                .is_some_and(|class| !self.world.methods_of(class, name).is_empty())
    }

    // A type name wins over a variable while the expected type is none or
    // `typename`; with any other expected type the variable lookup runs first.
    fn type_name_position(&self, expected: Option<&Ty>) -> bool {
        // A type nothing is known about, such as a template parameter, may
        // well be `typename`.
        // A type name is a value where the root class is wanted too.
        expected.is_none_or(|ty| {
            matches!(self.kind(ty), Kind::Typename | Kind::Void | Kind::Unknown) || ty.base() == Some("Class")
        })
    }

    pub(super) fn receiver(&mut self, text: &str) -> Ty {
        let Some(class) = self.class.clone() else {
            self.error("'this' or 'super' outside class is not valid".into());
            self.error(format!("Can't find variable '{text}'"));
            return Ty::Unknown;
        };
        if text == "super" {
            return self
                .world
                .base_of(&class)
                .map_or(Ty::Unknown, Ty::Named);
        }
        self.own_type(&class)
    }

    // `obj.name`, `Type.name` and `Enum.NAME` share this shape.
    fn member(&mut self, object: &Expr, name: &str) -> Ty {
        // A member the engine registers on an engine enum: the qualifier is a
        // name no script declares at all, so there is nothing to look it up in.
        if engine_names::GLOBALS.contains(&name)
            && object
                .leaf_text()
                .is_some_and(|text| !self.world.types.is_known(base_name(text)))
        {
            return Ty::Named("int".into());
        }
        // `Enum.NAME`: the enum's own value, which the game reads as an int.
        if let Some(text) = object.leaf_text()
            && self.is_enum(text)
        {
            let values = self.world.enum_values(base_name(text));
            // The engine adds members to its own enums, so a value the enum's
            // script body does not list may still be one it registers.
            if values.iter().any(|value| value == name)
                || engine_names::GLOBALS.contains(&name)
            {
                return Ty::Named("int".into());
            }
            self.error(format!("Can't find variable '{name}'"));
            return Ty::Unknown;
        }
        // A static method has no instance, so `this` and `super` only name the
        // class and its base there.
        if self.static_method
            && let Some(word @ ("this" | "super")) = object.leaf_text()
            && let Some(own) = self.class.clone()
        {
            let class = match word {
                "super" => self.world.base_of(&own).unwrap_or(own),
                _ => own,
            };
            return self.static_field(&class, name);
        }
        let object_ty = self.typed_in(object, None);
        if is_full_chain(object) {
            self.error(TOO_COMPLEX.into());
        }
        // `ClassName.field` reaches a static member without an instance, and a
        // built-in like `vector` or `string` carries its own constants too.
        if let Some(class) = object
            .leaf_text()
            .filter(|text| self.world.class_of(base_name(text)).is_some())
        {
            return self.static_field(class, name);
        }
        // A receiver whose type did not resolve, such as a call returning a
        // generic parameter, says nothing about its members.
        if object_ty.is_unknown() {
            return Ty::Unknown;
        }
        let Some(class) = object_ty.base() else {
            self.error(format!("Can't find variable '{name}'"));
            return Ty::Unknown;
        };
        let Some(member) = self.world.member_for(&object_ty, class, name) else {
            return self.missing_member(class, name);
        };
        let declaring = self.world.owner_of(class, name).unwrap_or(class);
        // `this.name` reaches a member the way its bare name does.
        let reach = match object.leaf_text() {
            Some("this") => Reach::Bare,
            _ => Reach::Field,
        };
        if let Some(word) = self.access(declaring, &member.modifiers, reach) {
            self.out_of_reach(format!("Variable '{name}' is {word}"), name);
        }
        member.ty
    }

    fn is_enum(&self, name: &str) -> bool {
        self.world.info(base_name(name)).is_some_and(|info| info.is_enum)
    }

    // `ClassName.field` where the left side is a type: an instance member is
    // the error the game prints, a static one is read.
    fn static_field(&mut self, class: &str, name: &str) -> Ty {
        let class = base_name(class).to_string();
        let Some(member) = self.world.member_of(&class, name).cloned() else {
            return self.missing_member(&class, name);
        };
        if !member.has("static") && !member.has("const") {
            self.error(format!("Variable '{name}' is not static"));
        }
        member.ty
    }

    // `name` is no declared field of `class`. `obj.Method` without the call is
    // the callback the game builds, and some members the engine fills in.
    fn missing_member(&mut self, class: &str, name: &str) -> Ty {
        if !self.world.methods_of(class, name).is_empty() {
            return Ty::Named("func".into());
        }
        if self.pre_populated(class, name) {
            return Ty::Named("int".into());
        }
        self.error(format!("Can't find variable '{name}'"));
        Ty::Unknown
    }

    // The variable a bare name reaches: a local, a member of the class chain,
    // then a global.
    pub(super) fn variable(&self, name: &str) -> Option<Member> {
        if let Some(local) = self.locals.find(name) {
            return Some(local.clone());
        }
        let member = self
            .class
            .as_deref()
            .and_then(|class| self.world.member_of(class, name));
        member.or_else(|| self.world.globals.get(name)).cloned()
    }

    // A variable that exists but may not be used here: the game says why, then
    // reports the name as one it could not find.
    fn out_of_reach(&mut self, reason: String, name: &str) {
        self.error(reason);
        self.error(format!("Can't find variable '{name}'"));
    }

    // An unqualified name inside a method: the class chain first.
    fn lookup_member(&mut self, name: &str) -> Option<Ty> {
        if let Some(local) = self.locals.find(name) {
            return Some(local.ty.clone());
        }
        let class = self.class.clone()?;
        let member = self.world.member_for(&self.own_type(&class), &class, name)?;
        if self.static_method && !member.has("static") && !member.has("const") {
            let reason = format!(
                "Trying to access non-static member '{name}' from static method '{}'",
                self.function_name
            );
            self.out_of_reach(reason, name);
        } else if let Some(word) = self.access(self.world.owner_of(&class, name).unwrap_or(&class), &member.modifiers, Reach::Bare) {
            self.out_of_reach(format!("Variable '{name}' is {word}"), name);
        }
        Some(member.ty)
    }

    fn new_object(&mut self, ty: &Ty, args: &[Expr]) {
        let Some(name) = ty.base() else {
            self.walk_args(args);
            return;
        };
        // `new Alias(...)` reaches the class an alias names, so the alias is a
        // type the game accepts here.
        let known = self.world.is_class(name)
            || super::is_primitive(name)
            || self.world.types.is_known(name)
            || self.is_template_param(name);
        if !known {
            self.error(format!("Bad type '{name}'"));
            self.error(format!("Can't find class {name}"));
            self.walk_args(args);
            return;
        }
        // A constructor is reached like a method, so its access is checked too.
        if let Some(owner) = self.world.class_of(name).map(str::to_string)
            && let Some(word) = self.constructor_access(&owner)
        {
            self.error(format!("Method '{name}' is {word}"));
        }
        self.constructor_arguments(name, args);
    }

    // The arguments of `new Class(...)` go to the constructor the class wrote,
    // or to the nearest one its bases wrote. A class with none takes none.
    pub(super) fn constructor_arguments(&mut self, name: &str, args: &[Expr]) {
        let Some(class) = self.world.class_of(name).map(str::to_string) else {
            self.walk_args(args);
            return;
        };
        let constructors: Vec<Method> = self
            .world
            .chain(&class)
            .iter()
            .map(|info| {
                // The merge renames a wrapped class and its constructor alike.
                let own = info
                    .methods
                    .iter()
                    .filter(|method| !method.destructor && plain_class(&method.name) == plain_class(&info.name));
                own.cloned().collect::<Vec<_>>()
            })
            .find(|found| !found.is_empty())
            .unwrap_or_default();
        match constructors.as_slice() {
            [constructor] => self.check_arguments(plain_class(&constructor.name), args, constructor, None),
            [] if !args.is_empty() && self.world.is_class(&class) && !self.world.reaches_engine_base(&class) => {
                self.walk_args(args);
                self.error(format!("Too many parameters for '{name}' method"));
            }
            _ => self.walk_args(args),
        }
    }

    // The access row this class's constructor would print, if any.
    fn constructor_access(&self, class: &str) -> Option<&'static str> {
        let info = self.world.info(class)?;
        let constructor = info
            .methods
            .iter()
            .find(|method| !method.destructor && method.name == info.name)?;
        self.access(&info.name, &constructor.modifiers, Reach::Method(&info.name))
    }
}

// The AST keeps a leaf's text but not its token kind, so a literal is told apart
// by shape: a number, or text no identifier could hold.
fn literal_type(text: &str) -> Option<Ty> {
    let number = text.strip_prefix('-').unwrap_or(text);
    if number.starts_with("0x") || number.starts_with("0X") {
        return Some(Ty::Named("int".into()));
    }
    if number.starts_with(|c: char| c.is_ascii_digit()) {
        let float = number.contains(['.', 'e', 'E']);
        let name = if float { "float" } else { "int" };
        return Some(Ty::Named(name.into()));
    }
    if !super::is_name(text) {
        return Some(Ty::Named("string".into()));
    }
    None
}

pub(super) const TOO_COMPLEX: &str = "Formula too complex";

// The game follows this many `.name` steps in one chain and complains about
// each one after them.
pub(super) const MEMBER_STEPS: usize = 16;

// Whether `expr` ends in as many `.name` steps as a chain may hold. A call in
// the chain is passed over: neither it nor its method's name is a step.
pub(super) fn is_full_chain(mut expr: &Expr) -> bool {
    let mut steps = 0;
    while steps < MEMBER_STEPS {
        match &expr.kind {
            ExprKind::Member { object, .. } => {
                steps += 1;
                expr = object;
            }
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Member { object, .. } => expr = object,
                _ => break,
            },
            _ => break,
        }
    }
    steps == MEMBER_STEPS
}
