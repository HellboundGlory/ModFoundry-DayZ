use crate::diag::Diagnostic;
use crate::input::Module;

use super::model::{ClassModel, FnModel, GlobalFn, VarModel};
use super::resolve::{alias_target, base_name, split_arguments, type_key, type_key_text, FnSignature, Kind, Scope};
use super::rules::{push, Declarations, Level};

// A class the functions are checked against: its own records, or what an earlier
// module left behind. Its template parameters are bound to the arguments the
// chain handed it, which is what a signature written in terms of them means.
pub(super) struct Ancestor<'a> {
    pub name: String,
    pub functions: Vec<&'a FnModel>,
    pub bindings: Vec<(String, String)>,
}

// The root every script class derives from, with its own functions, so a class
// with no written base still overrides what it declares.
pub(super) const ROOT: &str = "Class";

impl<'a> Ancestor<'a> {
    // The ancestor an override is searched in: the nearest one that declares the
    // name with exactly these parameter types.
    fn prototype(&self, function: &FnModel, scope: &Scope) -> Option<&'a FnModel> {
        self.functions.iter().copied().find(|other| {
            !other.destructor
                && other.name == function.name
                && exactly(scope, other, function, &self.bindings)
        })
    }

    // The ancestor's constructor: the nearest one named like its own class.
    fn constructor(&self) -> Option<&'a FnModel> {
        self.functions
            .iter()
            .copied()
            .find(|other| other.name == self.name && !other.destructor)
    }
}

// Every class's members in source order, then the file-level functions, which
// the game declares after all the classes of the module.
pub(super) fn rules(declarations: &Declarations) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for class in declarations.classes.iter().filter(|class| !class.forward) {
        class_functions(declarations, class, &mut out);
    }
    for (at, declared) in declarations.functions.iter().enumerate() {
        global_rules(declarations, at, declared, &mut out);
    }
    out
}

fn class_functions(declarations: &Declarations, class: &ClassModel, out: &mut Vec<Diagnostic>) {
    let scope = &declarations.types;
    // A chain that is broken is reported as its own row; the rules that need the
    // base would only repeat it.
    let ancestors = ancestors(declarations, class);
    let mut saw_constructor = false;
    for (at, function) in class.functions.iter().enumerate() {
        let earlier = &class.functions[..at];
        destructor_rules(class, function, earlier, out);
        parameter_rows(&class.file, class.module, function, out);
        if function.destructor {
            continue;
        }
        // A destructor keeps its own record, so `~Object` is no twin of the
        // constructor `Object()`; a `proto` one does count, which is how a
        // native method refuses to be overridden, and a `proto` that repeats
        // an earlier method is a twin itself.
        if earlier.iter().any(|other| {
            !other.destructor && other.name == function.name && covers(scope, other, function, &[])
        }) {
            push(
                out,
                &class.file,
                function.line,
                class.module,
                Level::Error,
                format!("Multiple declaration of function '{}'", function.name),
            );
        }
        if function.is_proto() {
            continue;
        }
        if function.name == class.name {
            // Only the first function named like the class is its constructor;
            // every later one carries the mangled name and is an overload.
            if saw_constructor {
                continue;
            }
            saw_constructor = true;
            if let Some(ancestors) = &ancestors {
                constructor_rules(declarations, class, function, ancestors, out);
            }
        } else if let Some(ancestors) = &ancestors {
            method_rules(declarations, class, function, earlier, ancestors, out);
        }
    }
}

fn destructor_rules(
    class: &ClassModel,
    function: &FnModel,
    earlier: &[FnModel],
    out: &mut Vec<Diagnostic>,
) {
    if !function.destructor {
        return;
    }
    // A class wrapped by `modded` carries the renamed name, and its destructor
    // was renamed with it.
    let plain = class.name.split('#').next().unwrap_or(&class.name);
    if function.name != class.name && function.name != plain {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::ErrorKeepsLoading,
            format!("Invalid destructor name '~{}'", function.name),
        );
    }
    if earlier.iter().any(|other| other.destructor) {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::ErrorKeepsLoading,
            format!("Multiple declaration destructor '{}'", function.name),
        );
    }
}

fn parameter_rows(file: &str, module: Module, function: &FnModel, out: &mut Vec<Diagnostic>) {
    let limit = if function.has_modifier("external") { 15 } else { 16 };
    if function.params.len() > limit {
        push(
            out,
            file,
            function.line,
            module,
            Level::Error,
            format!("Maximum arguments count 16 exceeded in method '{}'", function.name),
        );
    }
    let mut defaulted = false;
    for parameter in &function.params {
        if parameter.has_init {
            defaulted = true;
        } else if defaulted {
            push(
                out,
                file,
                function.line,
                module,
                Level::Error,
                format!("Misplaced non-default parameter {}", parameter.name),
            );
        }
    }
}

fn global_rules(
    declarations: &Declarations,
    at: usize,
    declared: &GlobalFn,
    out: &mut Vec<Diagnostic>,
) {
    let function = &declared.function;
    if function.has_modifier("private") || function.has_modifier("protected") {
        push(
            out,
            &declared.file,
            function.line,
            declared.module,
            Level::Error,
            format!("Function '{}' cannot be private/protected", function.name),
        );
    }
    parameter_rows(&declared.file, declared.module, function, out);
    let types = FnSignature::of(&declarations.types, function).types;
    // A twin in this module's own earlier files is a hard error; one from an
    // earlier module prints the same text as a soft error. A `proto` waits for
    // no engine record, so it takes part like any other declaration.
    let repeated = declarations.functions[..at].iter().any(|other| {
        other.function.name == function.name
            && FnSignature::of(&declarations.types, &other.function).covers(&types)
    });
    if repeated {
        push(
            out,
            &declared.file,
            function.line,
            declared.module,
            Level::Error,
            format!("Multiple declaration of function '{}'", function.name),
        );
    } else if declarations.known.repeats_function(&function.name, &types) {
        push(
            out,
            &declared.file,
            function.line,
            declared.module,
            Level::Soft,
            format!("Multiple declaration of function '{}'", function.name),
        );
    }
}

fn method_rules(
    declarations: &Declarations,
    class: &ClassModel,
    function: &FnModel,
    earlier: &[FnModel],
    ancestors: &[Ancestor<'_>],
    out: &mut Vec<Diagnostic>,
) {
    let scope = &declarations.types;
    let prototype = ancestors.iter().find_map(|parent| parent.prototype(function, scope));
    match (prototype, function.has_modifier("override")) {
        (Some(prototype), true) => {
            override_rules(declarations, class, function, prototype, ancestors, out)
        }
        (Some(_), false) => push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Soft,
            format!("Overriding function '{}' but not marked as 'override'", function.name),
        ),
        (None, true) => push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Error,
            format!(
                "Function '{}' is marked as override, but there is no function with this name in the base class",
                function.name
            ),
        ),
        (None, false) => event_rules(class, function, earlier, out),
    }
}

fn override_rules(
    declarations: &Declarations,
    class: &ClassModel,
    function: &FnModel,
    prototype: &FnModel,
    ancestors: &[Ancestor<'_>],
    out: &mut Vec<Diagnostic>,
) {
    let scope = &declarations.types;
    if prototype.has_modifier("sealed") {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Error,
            format!(
                "'{}.{}': cannot override inherited member '{}' because it is sealed",
                class.name, function.name, function.name
            ),
        );
        return;
    }
    if prototype.has_modifier("static") != function.has_modifier("static") {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Soft,
            format!(
                "Function '{}' override signature conflict: differing 'static' usage",
                function.name
            ),
        );
    }
    let bindings = prototype_bindings(ancestors, prototype);
    if return_category(scope, function) != category_of(scope, &prototype.return_type, &bindings) {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Error,
            format!("Overloaded function '{}' not compatible", function.name),
        );
        return;
    }
    for (own, base) in function.params.iter().zip(&prototype.params) {
        // The exact-type search sent a differing specifier here, so only the
        // specifier can differ now.
        if specifiers(own) != specifiers(base) && same_param(scope, own, base) {
            push(
                out,
                &class.file,
                function.line,
                class.module,
                Level::Error,
                format!(
                    "Overloaded function '{}', argument '{}' is not compatible with prototype argument type or spec '{}'",
                    function.name,
                    own.name,
                    specifier_text(base)
                ),
            );
            continue;
        }
        if own.name != base.name {
            push(
                out,
                &class.file,
                function.line,
                class.module,
                Level::Soft,
                format!(
                    "Overloaded method '{}', argument name '{}' differs from prototype '{}'",
                    function.name, own.name, base.name
                ),
            );
        }
    }
}

fn specifier_text(parameter: &VarModel) -> String {
    let mut text = parameter.modifiers.join(" ");
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(parameter.type_name.as_deref().unwrap_or_default());
    text
}

// `event` on a function of the name in this class forbids a second signature.
// No observed row covers an ancestor's event, so only this class's
// own earlier declarations are read.
fn event_rules(
    class: &ClassModel,
    function: &FnModel,
    earlier: &[FnModel],
    out: &mut Vec<Diagnostic>,
) {
    let event = earlier
        .iter()
        .any(|other| other.name == function.name && other.has_modifier("event"));
    if event {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Soft,
            format!("Overloading event '{}' is not allowed", function.name),
        );
    }
}

// The class's first constructor against the nearest base constructor: at least
// as many parameters, and the leading ones the same type and name. A parameter
// type that derives from the base's is a soft error.
fn constructor_rules(
    declarations: &Declarations,
    class: &ClassModel,
    function: &FnModel,
    ancestors: &[Ancestor<'_>],
    out: &mut Vec<Diagnostic>,
) {
    let Some(prototype) = ancestors.iter().find_map(Ancestor::constructor) else {
        return;
    };
    let scope = &declarations.types;
    if function.params.len() < prototype.params.len() {
        push(
            out,
            &class.file,
            function.line,
            class.module,
            Level::Error,
            format!("Overloaded function '{}' not compatible", function.name),
        );
        return;
    }
    let bindings = prototype_bindings(ancestors, prototype);
    for (own, base) in function.params.iter().zip(&prototype.params) {
        let base = bound(scope, base, &bindings);
        if !same_param(scope, own, &base) {
            // A type that derives from the base's is the narrower mistake.
            let level = if derives_from(declarations, own, &base) { Level::Soft } else { Level::Error };
            push(
                out,
                &class.file,
                function.line,
                class.module,
                level,
                format!(
                    "Constructor '{}', argument '{}' is not compatible with prototype argument type or spec '{}'",
                    class.name,
                    own.name,
                    base.type_name.as_deref().unwrap_or_default()
                ),
            );
        } else if own.name != base.name {
            push(
                out,
                &class.file,
                function.line,
                class.module,
                Level::Soft,
                format!(
                    "Constructor '{}', argument '{}' name differs from prototype '{}'",
                    class.name, own.name, base.name
                ),
            );
        }
    }
}

// The arguments written on a base type, resolved through any alias first.
fn split_base_arguments(scope: &Scope, written: &str) -> Vec<String> {
    let resolved = alias_target(scope, written);
    resolved
        .strip_suffix('>')
        .and_then(|text| text.find('<').map(|open| &text[open + 1..]))
        .map(written_arguments)
        .unwrap_or_default()
        .into_iter()
        .map(str::to_string)
        .collect()
}

// One level of `A, B<C, D>, E` split at the commas that belong to it. Nothing
// after the last comma is no argument.
fn written_arguments(inner: &str) -> Vec<&str> {
    let mut arguments = split_arguments(inner);
    if arguments.last().is_some_and(|last| last.is_empty()) {
        arguments.pop();
    }
    arguments
}

// A written type with every bound template parameter replaced, wherever it
// appears: a parameter may be a template whose arguments are those parameters.
fn substitute_bindings(text: &str, bindings: &[(String, String)]) -> String {
    let Some(open) = text.find('<') else {
        return match bindings.iter().find(|(name, _)| name == base_name(text)) {
            Some((_, value)) => value.clone(),
            None => text.trim().to_string(),
        };
    };
    let head = &text[..open];
    let inner = &text.strip_suffix('>').unwrap_or(text)[open + 1..];
    let rewritten: Vec<String> = written_arguments(inner)
        .iter()
        .map(|part| substitute_bindings(part, bindings))
        .collect();
    format!("{head}<{}>", rewritten.join(","))
}

// One parameter with its bindings applied.
fn bound(scope: &Scope, parameter: &VarModel, bindings: &[(String, String)]) -> VarModel {
    let Some(name) = parameter.type_name.as_deref() else {
        return parameter.clone();
    };
    // A binding applied to a written type reads it as the class that names it.
    VarModel {
        type_name: Some(alias_target(scope, &substitute_bindings(name, bindings))),
        ..parameter.clone()
    }
}

// The bindings in force where a prototype was declared, so a signature written
// in terms of the prototype class's own parameters reads as real types.
fn prototype_bindings<'a>(ancestors: &'a [Ancestor<'a>], prototype: &FnModel) -> Vec<(String, String)> {
    ancestors
        .iter()
        .find(|ancestor| ancestor.functions.iter().any(|other| std::ptr::eq(*other, prototype)))
        .map(|ancestor| ancestor.bindings.clone())
        .unwrap_or_default()
}

// The ancestors, nearest first. A class with no written base derives from the
// root `Class`; a body on a built-in's name extends that pseudo-class instead.
// `None` when the chain is broken: a base nothing declares, or a cycle. The
// `Unknown type` or `Circle inheritance` row is already the report for that.
pub(super) fn ancestors<'a>(
    declarations: &'a Declarations,
    class: &'a ClassModel,
) -> Option<Vec<Ancestor<'a>>> {
    let mut out = Vec::new();
    let mut seen = vec![class.name.clone()];
    let pseudo = declarations.types.is_primitive(&class.name);
    // The written base keeps its template arguments; only the lookup uses the
    // bare name. The root is added for a class that wrote none, and reaching a
    // name nothing declares only breaks the chain when it was written.
    let mut base = match class.base.as_deref() {
        Some(base) => Some((base.trim().to_string(), true)),
        None if pseudo => None,
        None => Some((ROOT.to_string(), false)),
    };
    // The bindings the class before this one handed down, by that class's own
    // parameter names; the next class's parameters are bound through them.
    let mut bindings: Vec<(String, String)> = class
        .template
        .iter()
        .map(|parameter| (parameter.clone(), parameter.clone()))
        .collect();
    while let Some((written, rung)) = base {
        let resolved = alias_target(&declarations.types, &written);
        let name = base_name(&resolved).to_string();
        if seen.contains(&name) {
            return None;
        }
        seen.push(name.clone());
        let (functions, template, next): (Vec<&FnModel>, Vec<String>, Option<(String, bool)>) =
            match declarations.holder(&name) {
                Some(parent) => {
                    let pseudo = declarations.types.is_primitive(&parent.name);
                    // The next hop keeps the arguments that class wrote.
                    let next = match parent.base.as_deref() {
                        Some(base) => Some((base.trim().to_string(), true)),
                        None if parent.name != ROOT && !pseudo => Some((ROOT.to_string(), false)),
                        None => None,
                    };
                    (parent.functions.iter().collect(), parent.template.clone(), next)
                }
                None => {
                    let Some(record) = declarations.known.classes.get(&name) else {
                        // The root is added rather than written, so a module
                        // without it written down is no broken chain.
                        return if rung { None } else { Some(out) };
                    };
                    let next = match record.base.as_deref() {
                        Some(base) => Some((base.to_string(), true)),
                        None if name != ROOT => Some((ROOT.to_string(), false)),
                        None => None,
                    };
                    (record.functions.iter().collect(), (*record.template).clone(), next)
                }
            };
        // The arguments this class's base handed it, written in terms of the
        // bindings in force, bind this class's own parameters.
        let handed = split_base_arguments(&declarations.types, &resolved);
        let own: Vec<(String, String)> = template
            .iter()
            .enumerate()
            .map(|(at, parameter)| {
                let value = match handed.get(at) {
                    Some(argument) => substitute_bindings(argument, &bindings),
                    None => parameter.clone(),
                };
                (parameter.clone(), value)
            })
            .collect();
        out.push(Ancestor {
            name: name.clone(),
            functions,
            bindings: own.clone(),
        });
        bindings = own;
        base = next;
    }
    Some(out)
}

// The exact-signature search `override` is checked against: the same count and
// the same parameter types, with the prototype's own template parameters read as
// the arguments the chain handed it.
fn exactly(scope: &Scope, candidate: &FnModel, function: &FnModel, bindings: &[(String, String)]) -> bool {
    candidate.params.len() == function.params.len()
        && candidate.params.iter().zip(&function.params).all(|(left, right)| {
            bound_param(scope, left, bindings) == keyed(scope, right)
        })
}

// The search a duplicate uses: the stored function may hold extra trailing
// defaulted parameters, the new one may not.
fn covers(scope: &Scope, candidate: &FnModel, function: &FnModel, bindings: &[(String, String)]) -> bool {
    let count = function.params.len();
    candidate.params.len() >= count
        && candidate.params.iter().filter(|parameter| !parameter.has_init).count() <= count
        && candidate.params.iter().zip(&function.params).all(|(left, right)| {
            bound_param(scope, left, bindings) == keyed(scope, right)
        })
}

// One parameter's type and rank, with a template parameter read as its binding.
fn bound_param(scope: &Scope, parameter: &VarModel, bindings: &[(String, String)]) -> (String, usize) {
    let written = parameter.type_name.as_deref().unwrap_or_default();
    let resolved = substitute_bindings(written, bindings);
    (type_key_text(scope, &resolved), parameter.dims.len())
}

fn keyed(scope: &Scope, parameter: &VarModel) -> (String, usize) {
    (type_key(scope, parameter), parameter.dims.len())
}

fn same_param(scope: &Scope, left: &VarModel, right: &VarModel) -> bool {
    type_key(scope, left) == type_key(scope, right) && left.dims.len() == right.dims.len()
}

// Whether the first parameter's type derives from the second's, nearest first.
fn derives_from(declarations: &Declarations, own: &VarModel, base: &VarModel) -> bool {
    let Some(own) = own.type_name.as_deref() else {
        return false;
    };
    let target = base_name(base.type_name.as_deref().unwrap_or_default());
    let mut current = base_name(own).to_string();
    for _ in 0..16 {
        if current == target {
            return true;
        }
        let next = match declarations.holder(&current) {
            Some(class) => class.base_name().map(str::to_string),
            None => declarations.known.bases.get(&current).map(|base| base_name(base).to_string()),
        };
        match next {
            Some(next) => current = next,
            None => return false,
        }
    }
    false
}

fn specifiers(parameter: &VarModel) -> Vec<&str> {
    ["out", "inout", "notnull", "ref", "autoptr"]
        .into_iter()
        .filter(|name| parameter.has_modifier(name))
        .collect()
}

// Return types agree when their category does. `int` and `bool` share one: the
// vanilla scripts override `bool IsEat()` with an `int` one and compile.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Int,
    Float,
    String,
    Vector,
    Class,
    Void,
}

fn return_category(scope: &Scope, function: &FnModel) -> Category {
    category_of(scope, &function.return_type, &[])
}

// The category of a written type, with a template parameter replaced by the
// argument it was given.
fn category_of(
    scope: &Scope,
    type_name: &Option<String>,
    bindings: &[(String, String)],
) -> Category {
    let Some(type_name) = type_name.as_deref() else {
        return Category::Void;
    };
    let name = substitute_bindings(type_name, bindings);
    let name = alias_target(scope, &name);
    if scope.kind_of(&name) == Some(Kind::Enum) {
        return Category::Int;
    }
    match base_name(&name) {
        "void" => Category::Void,
        "float" => Category::Float,
        "string" => Category::String,
        "vector" => Category::Vector,
        "int" | "bool" => Category::Int,
        _ => Category::Class,
    }
}

