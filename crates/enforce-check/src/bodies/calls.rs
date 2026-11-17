use crate::parser::{Expr, ExprKind};

use super::conversions::{Dest, Verdict};
use super::expr::TOO_COMPLEX;
use super::scope::{base_name, is_value_type, Method, Ty};
use super::{engine_names, is_primitive, plain_class, BodyChecker, Reach};

// How a call names its method, which decides the access it is held to: by a
// bare name, on a receiver, or through `super`, which reaches every protected
// member of its ancestors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    Bare,
    Receiver,
    Super,
}

// A call's resolved candidate set: the arguments, the methods found for the
// name, and the class whose chain they came from.
struct Call<'a> {
    name: &'a str,
    args: &'a [Expr],
    methods: Vec<&'a Method>,
    class: Option<&'a str>,
}

// How many calls may sit in one another's receiver or arguments. Twenty in a
// row and two hundred inside each other are each one call too many; the
// number itself is taken from the member steps.
const CALLS_OPEN: u32 = 16;

impl BodyChecker<'_> {
    // `Name(...)`, `obj.Name(...)`, `Type.Name(...)`, `super.Name(...)`.
    pub(super) fn call(&mut self, callee: &Expr, args: &[Expr]) -> Ty {
        // The game gives up on the call that is one too deep and reads
        // nothing inside it.
        if self.calls_open == CALLS_OPEN {
            self.error(TOO_COMPLEX.into());
            return Ty::Unknown;
        }
        self.calls_open += 1;
        let ty = self.call_at_depth(callee, args);
        self.calls_open -= 1;
        ty
    }

    fn call_at_depth(&mut self, callee: &Expr, args: &[Expr]) -> Ty {
        if let ExprKind::Member { object, name } = &callee.kind {
            // `T.Cast(x)` is the cast helper, not a method of the class.
            if name == "Cast" {
                return self.cast(object, args);
            }
            return self.member_call(object, name, args);
        }
        let Some(name) = callee.leaf_text() else {
            self.typed_in(callee, None);
            return Ty::Unknown;
        };
        if args.len() == 1 && (is_primitive(name) || self.is_template_param(name)) {
            self.typed_in(&args[0], None);
            return Ty::named(name);
        }
        self.bare_call(name, args)
    }

    // A call with no receiver word in front of it.
    fn bare_call(&mut self, name: &str, args: &[Expr]) -> Ty {
        if name == "new" {
            return Ty::Unknown;
        }
        // An alias to a container is built by naming it, which the game allows
        // for the built-in containers the engine supplies.
        if let Some(class) = self.world.class_of(name)
            && matches!(
                self.world.info(class).map(|info| info.name.as_str()),
                Some("array" | "set" | "map")
            )
        {
            self.walk_args(args);
            return Ty::named(class);
        }
        // `ClassName(args)` written without `new` builds that class: the game
        // reaches its constructor through the class name, and a class with no
        // constructor of its own inherits one.
        if let Some(class) = self.world.class_of(name).map(str::to_string)
            && self.world.types.is_known(&class)
            // `Class(args)` builds it through its own constructor, or the base's
            // when the class writes none of its own.
            && self.world.builds_with(&class, name)
        {
            self.constructor_arguments(&class, args);
            return Ty::Named(class);
        }
        // Inside a method the current class is the receiver; with no class the
        // lookup falls back to the module's global functions.
        let class = self.class.clone();
        if let Some(class) = &class {
            let methods = self.world.methods_of(class, name);
            if !methods.is_empty() {
                return self.resolve_call(Call { name, args, methods, class: Some(class) }, None, Via::Bare);
            }
        }
        let globals: Vec<&Method> = self
            .world
            .functions
            .get(name)
            .map(|found| found.iter().collect())
            .unwrap_or_default();
        if globals.is_empty() {
            // A class whose ancestry reaches a class no script declares may still
            // carry the method on an engine class.
            if class.as_ref().is_some_and(|class| self.world.reaches_engine_base(class)) {
                self.walk_args(args);
                return Ty::Unknown;
            }
            self.undefined_function(class.as_deref(), name);
            return Ty::Unknown;
        }
        self.resolve_call(Call { name, args, methods: globals, class: None }, None, Via::Bare)
    }

    // `receiver.Name(...)`: the receiver is `super`, a type name or a value.
    fn member_call(&mut self, object: &Expr, name: &str, args: &[Expr]) -> Ty {
        if object.leaf_text() == Some("super") {
            return self.super_call(name, args);
        }
        // A type name on the left: a static method, or the instance-method slip.
        // A local can share the name of a class that is only declared ahead,
        // and is then what the name means.
        if let Some(text) = object.leaf_text()
            && self.world.class_of(base_name(text)).is_some()
            && (self.locals.find(text).is_none() || self.world.types.blocks_variable_name(text))
        {
            return self.static_call(base_name(text), name, args);
        }
        // In a static method `this` only names the class.
        if self.static_method
            && object.leaf_text() == Some("this")
            && let Some(class) = self.class.clone()
        {
            return self.static_call(&class, name, args);
        }
        let object_ty = self.typed_in(object, None);
        // A template parameter stands for a class the body does not name, so
        // its members cannot be looked up.
        if object_ty.is_unknown() || object_ty.base().is_some_and(|name| self.is_template_param(name)) {
            self.walk_args(args);
            return Ty::Unknown;
        }
        let Some(class) = object_ty.base().map(str::to_string) else {
            self.error(format!("Undefined function '{name}'"));
            return Ty::Unknown;
        };
        let methods = self.world.methods_of(&class, name);
        // `this.Name()` reaches a method the way its bare name does.
        let via = match object.leaf_text() {
            Some("this") => Via::Bare,
            _ => Via::Receiver,
        };
        self.resolve_call(Call { name, args, methods, class: Some(&class) }, Some(&object_ty), via)
    }

    fn super_call(&mut self, name: &str, args: &[Expr]) -> Ty {
        self.receiver("super");
        // `super` reaches the nearest ancestor that declares the name, which
        // may be further up than the immediate base.
        let Some(class) = self.class.clone() else {
            return Ty::Unknown;
        };
        let base = self
            .world
            .chain(&class)
            .into_iter()
            .skip(1)
            .find(|info| !info.methods(name).is_empty())
            .map(|info| info.name.clone())
            .or_else(|| self.world.base_of(&class));
        let Some(base) = base else {
            return Ty::Unknown;
        };
        let methods = self.world.methods_of(&base, name);
        // A subclass may reach every protected member of its ancestors, so a
        // `super` call reports no access row; the missing-method row still
        // applies unless the ancestry runs into a class no script declares.
        if methods.is_empty() && self.world.reaches_engine_base(&base) {
            self.walk_args(args);
            return Ty::Unknown;
        }
        // With no instance, `super` only names the base class.
        if self.static_method && !methods.is_empty() && !methods.iter().any(|method| method.is_static()) {
            self.error(format!("Trying to call non-static function '{name}' as static"));
            self.walk_args(args);
            return methods[0].ret.clone();
        }
        self.resolve_call(Call { name, args, methods, class: Some(&base) }, None, Via::Super)
    }

    // `Type.Name(...)`: a static method, or the instance-method slip.
    fn static_call(&mut self, class: &str, name: &str, args: &[Expr]) -> Ty {
        let methods = self.world.methods_of(class, name);
        if methods.is_empty() && self.is_engine_method(class, name) {
            self.walk_args(args);
            return Ty::Named(class.to_string());
        }
        if methods.is_empty() {
            self.undefined_function(Some(class), name);
            return Ty::Unknown;
        }
        if !methods.iter().any(|method| method.is_static()) {
            self.error(format!("Trying to call non-static function '{name}' as static"));
            return methods[0].ret.clone();
        }
        let static_methods: Vec<&Method> = methods
            .into_iter()
            .filter(|method| method.is_static())
            .collect();
        self.resolve_call(Call { name, args, methods: static_methods, class: Some(class) }, None, Via::Receiver)
    }

    // Calls reaching a resolved candidate set.
    fn resolve_call(&mut self, call: Call<'_>, receiver: Option<&Ty>, via: Via) -> Ty {
        let Call {
            name,
            args,
            methods,
            class,
        } = call;
        // A constructor written as a call is never allowed to be called directly.
        if let Some(class) = class
            && name == class
            && methods.iter().any(|method| !method.destructor)
        {
            self.error(format!(
                "Cannot call constructor or destructor '{class}' method directly!"
            ));
            return Ty::Named(class.to_string());
        }
        if methods.is_empty() {
            // The engine may hold the method on a class the scripts never
            // declare, so nothing is reported for such a receiver.
            let engine = class.is_some_and(|class| {
                self.is_engine_method(class, name) || self.world.reaches_engine_base(class)
            });
            if engine {
                self.walk_args(args);
                return Ty::Unknown;
            }
            self.undefined_function(class, name);
            return Ty::Unknown;
        }
        let (method, typed) = match methods.len() {
            1 => (methods[0].clone(), None),
            _ => {
                let levelled = match class {
                    Some(class) => self.world.methods_with_level(class, name),
                    // A global function has one level, since there is no chain.
                    None => methods.iter().map(|method| (*method, 0)).collect(),
                };
                match self.pick_overload(name, args, levelled) {
                    Some((method, types)) => (method, Some(types)),
                    None => return Ty::Unknown,
                }
            }
        };
        if let Some(word) = self.call_access(class, &method, via) {
            self.error(format!("Method '{name}' is {word}"));
        }
        self.check_arguments(name, args, &method, typed);
        match class {
            Some(class) => {
                let receiver = receiver.cloned().unwrap_or_else(|| self.own_type(class));
                self.world.return_for(&receiver, class, &method)
            }
            None => method.ret.clone(),
        }
    }

    // Score each overload; the lowest total wins, a tie is ambiguous unless
    // the tied candidates sit at different levels of the chain, where the one the
    // class declares itself is the pick.
    fn pick_overload(
        &mut self,
        name: &str,
        args: &[Expr],
        methods: Vec<(&Method, usize)>,
    ) -> Option<(Method, Vec<Ty>)> {
        // Every argument has a type once; the candidates score against those so a
        // nested call is not walked once per candidate.
        let arg_types: Vec<Ty> = args.iter().map(|arg| self.typed_in(arg, None)).collect();
        let mut scored: Vec<(i64, usize, Method)> = Vec::new();
        for (method, level) in &methods {
            if args.len() > method.params.len() || args.len() < method.required() {
                continue;
            }
            let mut total = 0i64;
            let mut usable = true;
            for (index, arg_ty) in arg_types.iter().enumerate() {
                match self.score(arg_ty, &method.params[index].ty, method) {
                    Some(score) => total += score,
                    None => {
                        usable = false;
                        break;
                    }
                }
            }
            if usable {
                scored.push((total, *level, (*method).clone()));
            }
        }
        let Some(best) = scored.iter().map(|(score, _, _)| *score).min() else {
            self.error(format!("Can't find matching overload for function '{name}'"));
            return None;
        };
        let nearest = scored
            .iter()
            .filter(|(score, _, _)| *score == best)
            .map(|(_, level, _)| *level)
            .min()
            .unwrap_or(0);
        let winners: Vec<Method> = scored
            .iter()
            .filter(|(score, level, _)| *score == best && *level == nearest)
            .map(|(_, _, method)| method.clone())
            .collect();
        // An argument of unknown type cannot rank the tied candidates, and the
        // game's answer for it is not known; a tie it alone makes stays silent.
        if winners.len() > 1 && arg_types.iter().any(Ty::is_unknown) {
            return None;
        }
        if winners.len() > 1 {
            self.error(format!("Ambiguous function call '{name}', candidates:"));
            for winner in &winners {
                let signature = format!("\t{}", winner.signature());
                self.error(signature);
            }
            self.error(format!("Can't find matching overload for function '{name}'"));
            return None;
        }
        Some((winners[0].clone(), arg_types))
    }

    // 0 exact, 1 between two numeric word types, 2 enum from int, 3 null to a
    // class, then the inheritance distance; None drops the candidate.
    fn score(&self, arg: &Ty, param: &Ty, method: &Method) -> Option<i64> {
        if arg == param {
            return Some(0);
        }
        if param.base() == Some("void") && method.is_native() {
            return Some(10);
        }
        if *arg == Ty::Null && param.base().is_some_and(|name| self.world.is_class(name)) {
            return Some(3);
        }
        // An argument whose own type did not resolve neither prefers nor drops
        // a candidate, so it scores above every real match and below the `void`
        // catch-all; a tie it alone would produce is then not a tie at all.
        if arg.is_unknown() {
            return Some(9);
        }
        let arg_name = arg.base()?;
        let param_name = param.base()?;
        // A parameter typed by a template parameter takes whatever the
        // instantiation supplies.
        if self.is_template_param(param_name) {
            return Some(9);
        }
        if self.container_arguments_differ(arg, param) {
            return None;
        }
        // One container with other arguments converts, but is no exact match:
        // `array<int>` is not the pick where an `array<bool>` overload exists.
        match (arg.expanded(self.world), param.expanded(self.world)) {
            (left, right) if left == right => return Some(0),
            (Ty::Container(left, _), Ty::Container(right, _)) if left == right => return Some(1),
            _ => {}
        }
        if is_word_type(arg_name) && is_word_type(param_name) {
            return Some(1);
        }
        if self.world.info(param_name).is_some_and(|info| info.is_enum)
            && arg_name == "int"
        {
            return Some(2);
        }
        // An enum member is an int where one is declared.
        if param_name == "int" && self.world.info(arg_name).is_some_and(|info| info.is_enum) {
            return Some(2);
        }
        self.distance(arg_name, param_name)
            // A class built on a primitive is that value: `class CF_Byte : int`
            // accepts an `int` where a `CF_Byte` is declared, and the reverse.
            .or_else(|| self.primitive_kin(arg_name, param_name))
    }

    // Two instances of one container whose arguments cannot convert, such as an
    // `array<string>` argument for an `array<ExpansionString>` parameter.
    fn container_arguments_differ(&self, arg: &Ty, param: &Ty) -> bool {
        let (arg, param) = (arg.expanded(self.world), param.expanded(self.world));
        let (Ty::Container(arg_name, arg_args), Ty::Container(param_name, param_args)) = (&arg, &param) else {
            return false;
        };
        arg_name == param_name
            && arg_args.len() == param_args.len()
            && arg_args.iter().zip(param_args).any(|(left, right)| {
                match (left.base(), right.base()) {
                    (Some(left), Some(right)) => {
                        !self.is_template_param(left)
                            && !self.is_template_param(right)
                            && self.distance(left, right).is_none()
                            && !(is_word_type(left) && is_word_type(right))
                    }
                    _ => false,
                }
            })
    }

    // The distance between two types that meet at the same built-in value type.
    fn primitive_kin(&self, arg_name: &str, param_name: &str) -> Option<i64> {
        let root = |name: &str| {
            self.world
                .chain(name)
                .into_iter()
                .map(|info| info.name.as_str())
                .find(|name| is_value_type(name))
                .map(str::to_string)
        };
        let arg_root = root(arg_name)?;
        let param_root = root(param_name)?;
        (arg_root == param_root).then_some(1)
    }

    // Inheritance steps from `from` up to `to`.
    fn distance(&self, from: &str, to: &str) -> Option<i64> {
        // The chain follows aliases and merge renames, so the walk is the same
        // one member lookup uses.
        let to = self.world.class_of(to).unwrap_or(to);
        // A class a `modded` wrapper replaced and the plain name are one type.
        if plain_class(from) == plain_class(to) {
            return Some(0);
        }
        self.world
            .chain(from)
            .into_iter()
            .position(|info| base_name(&info.name) == base_name(to))
            .map(|steps| steps as i64)
    }

    // `typed` holds the argument types when the overload pick already computed
    // them, so a nested call is walked once.
    pub(super) fn check_arguments(
        &mut self,
        name: &str,
        args: &[Expr],
        method: &Method,
        typed: Option<Vec<Ty>>,
    ) {
        // An argument is read against the parameter it lands in; the overload
        // pick read them with no type wanted.
        let types = typed.unwrap_or_else(|| {
            args.iter()
                .enumerate()
                .map(|(index, arg)| self.typed_in(arg, method.params.get(index).map(|param| &param.ty)))
                .collect()
        });
        for (index, arg_ty) in types.iter().enumerate() {
            let Some(param) = method.params.get(index) else {
                self.error(format!("Too many parameters for '{name}' method"));
                return;
            };
            // An `out` or `inout` parameter writes its argument back.
            if param.modifiers.iter().any(|word| word == "out" || word == "inout") && self.read_only(&args[index], false) {
                continue;
            }
            self.store(&args[index], arg_ty, &param.ty, Dest::Argument { index, function: name });
        }
        if args.len() < method.required() {
            self.error(format!("Not enough parameters in function '{name}'"));
        } else if args.len() > method.params.len() {
            self.error(format!("Too many parameters for '{name}' method"));
        }
    }

    // A method the engine adds to a class it also fills in.
    fn is_engine_method(&self, class: &str, name: &str) -> bool {
        engine_names::ENGINE_METHODS.iter().any(|(owner, methods)| {
            base_name(class) == *owner && methods.contains(&name)
        })
    }

    // Access is judged on the overload the call resolved to, in the class that
    // declares it.
    fn call_access(&self, class: Option<&str>, method: &Method, via: Via) -> Option<&'static str> {
        let class = class?;
        let reach = match via {
            Via::Super => return None,
            Via::Bare => Reach::Bare,
            Via::Receiver => Reach::Method(class),
        };
        let owner = self.world.method_owner(class, method)?;
        self.access(owner, &method.modifiers, reach)
    }

    // `T.Cast(x)`: only a class type carries a `SafeCastType`, and calling it on
    // an instance is the shape the parser already rejected.
    fn cast(&mut self, object: &Expr, args: &[Expr]) -> Ty {
        let Some(text) = object.leaf_text() else {
            self.error("Syntax error".into());
            return Ty::Unknown;
        };
        // A local or member name is an instance, which is the shape the parser
        // rejects: the game prints its syntax error rather than a cast row.
        let name = base_name(text).to_string();
        if self.locals.find(&name).is_some() || self.world.globals.contains_key(&name) {
            self.error("Syntax error".into());
            return Ty::Unknown;
        }
        let ty = Ty::named(text).substituted(&self.bindings);
        let name = ty.base().unwrap_or(&name).to_string();
        // Only a class type carries a `SafeCastType`; an alias to one does too,
        // but a built-in name like `int` never does.
        let is_class = !is_primitive(&name) && self.world.class_of(&name).is_some();
        if !is_class {
            self.error(format!("Cast not supported on type '{name}'"));
            return ty;
        }
        let Some(value) = args.first() else {
            return ty;
        };
        let value_ty = self.typed_in(value, None);
        self.walk_args(&args[1..]);
        match self.convert(&ty, &value_ty) {
            // The value's class sits below the target already. A cast to the
            // value's own class says nothing, which vanilla relies on.
            Verdict::Downcast => self.warning("No need to use 'Cast' for up-casting".into()),
            Verdict::Unrelated => {
                self.error(format!("Types '{}' and '{}' are not related", ty.text(), value_ty.text()));
            }
            _ => {}
        }
        ty
    }

    // `Sleep(x)` and `Wait(x)`: only the argument has a type to check.
    pub(super) fn command(&mut self, expr: &Expr) {
        if let ExprKind::Call { args, .. } = &expr.kind {
            self.walk_args(args);
        }
    }

    // `thread Foo();` needs a script function, not a native one.
    pub(super) fn thread(&mut self, expr: &Expr) {
        let ExprKind::Call { callee, args } = &expr.kind else {
            self.typed_in(expr, None);
            return;
        };
        let Some(name) = callee.leaf_text() else {
            self.call(callee, args);
            return;
        };
        let class = self.class.clone();
        let own = class
            .as_deref()
            .map(|class| self.world.methods_of(class, name))
            .filter(|found| !found.is_empty());
        let methods = own.or_else(|| self.world.functions.get(name).map(|found| found.iter().collect()));
        let Some(methods) = methods else {
            self.undefined_function(class.as_deref(), name);
            return;
        };
        if methods.iter().all(|method| method.is_native()) {
            self.error(format!("Cannot create thread out of non-script function '{name}'"));
            return;
        }
        // Several overloads leave no single signature to hold the arguments to.
        match methods.as_slice() {
            [method] => self.check_arguments(name, args, method, None),
            _ => self.walk_args(args),
        }
    }

    // Types each argument so the names inside it are looked up, with no call
    // to match them against.
    pub(super) fn walk_args(&mut self, args: &[Expr]) {
        for arg in args {
            self.typed_in(arg, None);
        }
    }

    fn undefined_function(&mut self, class: Option<&str>, name: &str) {
        let printed = match class {
            Some(class) => format!("{}.{name}", self.class_label(class)),
            None => name.to_string(),
        };
        self.error(format!("Undefined function '{printed}'"));
    }
}

fn is_word_type(name: &str) -> bool {
    matches!(name, "int" | "float" | "bool")
}
