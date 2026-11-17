use std::collections::HashMap;
use std::rc::Rc;

use super::model::{ClassModel, ClassRecord, FnModel, GlobalFn, VarModel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Class,
    Enum,
    Typedef,
    Primitive,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub kind: Kind,
    pub target: Option<String>,
    pub sealed: bool,
    pub forward: bool,
    pub engine: bool,
}

// The built-in type names. `int`, `float`, `bool`, `vector`, `typename` and
// `func` are the engine's own classes in `1_Core/proto/EnConvert.c`, `string` in
// `EnString.c`; `void` is the return type of every `proto` in `1_Core`; `auto` is
// the inferred local type the vanilla scripts use. No other built-in name has a
// source.
const PRIMITIVES: [&str; 9] = [
    "void", "int", "float", "bool", "string", "vector", "typename", "auto", "func",
];

// A name without its template arguments or an array suffix.
pub fn base_name(text: &str) -> &str {
    let name = text.split('<').next().unwrap_or(text);
    name.split('[').next().unwrap_or(name).trim()
}

pub fn is_primitive(text: &str) -> bool {
    PRIMITIVES.contains(&base_name(text))
}

// The template arguments of a type, split at the commas that are not inside a
// nested `<...>`.
pub fn split_arguments(text: &str) -> Vec<&str> {
    let mut arguments = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                arguments.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    arguments.push(text[start..].trim());
    arguments
}

// The types a module sees: its own, plus everything the earlier modules declared.
// A name from a later layer is absent, which is what `Unknown type` reports.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    symbols: HashMap<String, Symbol>,
}

impl Symbol {
    pub fn class(sealed: bool, forward: bool, engine: bool) -> Symbol {
        Symbol { kind: Kind::Class, target: None, sealed, forward, engine }
    }

    pub fn enumeration(sealed: bool) -> Symbol {
        Symbol { kind: Kind::Enum, target: None, sealed, forward: false, engine: false }
    }

    pub fn alias(target: &str) -> Symbol {
        Symbol {
            kind: Kind::Typedef,
            target: Some(target.to_string()),
            sealed: false,
            forward: false,
            engine: false,
        }
    }
}

impl Scope {
    pub fn is_engine(&self, text: &str) -> bool {
        self.find(text).is_some_and(|symbol| symbol.engine)
    }

    // A record already present stays: typedefs and enums are recorded before any
    // class, so an alias hides a class of the same name.
    pub fn insert(&mut self, name: &str, symbol: Symbol) {
        self.symbols.entry(name.to_string()).or_insert(symbol);
    }

    // A class body on a name that is already an alias takes the name over: the
    // engine writes `typedef int[] Foo;` beside `class Foo`. An enum record keeps
    // it, so a class of that name is a duplicate. A forward declaration only
    // registers the name, so a body takes it over and a forward declaration never
    // takes one.
    pub fn insert_class(&mut self, name: &str, symbol: Symbol) {
        if symbol.forward {
            self.symbols.entry(name.to_string()).or_insert(symbol);
            return;
        }
        match self.symbols.get(name) {
            Some(existing) if existing.kind != Kind::Typedef && !existing.forward => {}
            _ => {
                self.symbols.insert(name.to_string(), symbol);
            }
        }
    }

    pub fn find(&self, text: &str) -> Option<&Symbol> {
        self.symbols.get(base_name(text))
    }

    pub fn has(&self, text: &str) -> bool {
        self.symbols.contains_key(base_name(text))
    }

    pub fn kind_of(&self, text: &str) -> Option<Kind> {
        let name = base_name(text);
        if is_primitive(name) {
            return Some(Kind::Primitive);
        }
        let mut current = name.to_string();
        for _ in 0..8 {
            let symbol = self.symbols.get(&current)?;
            match symbol.kind {
                Kind::Typedef => {
                    let target = symbol.target.as_deref().map(base_name)?;
                    if target == current {
                        return Some(Kind::Typedef);
                    }
                    if is_primitive(target) {
                        return Some(Kind::Primitive);
                    }
                    current = target.to_string();
                }
                kind => return Some(kind),
            }
        }
        Some(Kind::Typedef)
    }

    pub fn is_known(&self, text: &str) -> bool {
        self.kind_of(text).is_some()
    }

    // A name a variable may not take. A class that is only forward-declared
    // does not count: the game accepts a variable of that name.
    pub fn blocks_variable_name(&self, text: &str) -> bool {
        self.is_known(text) && !self.find(text).is_some_and(|symbol| symbol.forward)
    }

    pub fn is_primitive(&self, text: &str) -> bool {
        self.kind_of(text) == Some(Kind::Primitive)
    }
}

// The part of a global function an earlier module leaves behind: the parameter
// types a match is searched for, and how many of them lack a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnSignature {
    pub types: Vec<String>,
    pub required: usize,
}

impl FnSignature {
    pub fn of(scope: &Scope, function: &FnModel) -> Self {
        Self {
            types: function.params.iter().map(|parameter| type_key(scope, parameter)).collect(),
            required: function.params.iter().filter(|parameter| !parameter.has_init).count(),
        }
    }

    // Whether a declaration with these parameter types collides with this one:
    // the stored signature may hold extra trailing defaulted parameters, the new
    // one may not.
    pub fn covers(&self, types: &[String]) -> bool {
        self.types.len() >= types.len() && self.required <= types.len() && self.types[..types.len()] == *types
    }
}

// A parameter's type as the game compares it: an enum is built on `int`, so an
// enum type and an `int` are the same type there; a typedef stands for its
// target, while template arguments stay part of the type.
pub fn type_key(scope: &Scope, parameter: &VarModel) -> String {
    parameter
        .type_name
        .as_deref()
        .map(|name| type_key_text(scope, name))
        .unwrap_or_default()
}

// The name a typedef chain ends at, or the written name when it is not an alias.
// A chain that loops or is longer than any real one stops where it is.
pub fn alias_target(scope: &Scope, name: &str) -> String {
    let mut current = name.trim().to_string();
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..32 {
        let base = base_name(&current).to_string();
        if seen.contains(&base) {
            break;
        }
        let Some(target) = scope
            .find(&base)
            .filter(|symbol| symbol.kind == Kind::Typedef)
            .and_then(|symbol| symbol.target.clone())
        else {
            break;
        };
        seen.push(base);
        // An alias written with arguments keeps them when its target has none.
        current = match (current.find('<'), target.find('<')) {
            (Some(open), None) => format!("{}{}", target.trim(), &current[open..]),
            _ => target.trim().to_string(),
        };
    }
    current
}

// The key for a written type, for a type that already went through template
// substitution.
pub fn type_key_text(scope: &Scope, text: &str) -> String {
    let resolved = alias_target(scope, text);
    match scope.kind_of(&resolved) {
        Some(Kind::Enum) => "int".to_string(),
        _ => resolved,
    }
}

// What the modules compiled before this one declared. A base or a member from an
// earlier layer resolves here; one from a later layer does not.
#[derive(Clone, Debug, Default)]
pub struct Known {
    pub types: Scope,
    // Own member names of each class.
    pub members: HashMap<String, Vec<String>>,
    pub bases: HashMap<String, String>,
    // `const int` names an array size may read, from the earlier modules.
    pub constants: HashMap<String, Option<i64>>,
    // The global functions of the earlier modules, by name.
    pub functions: HashMap<String, Vec<FnSignature>>,
    // The classes of the earlier modules, by plain name.
    pub classes: HashMap<String, ClassRecord>,
}

impl Known {
    pub fn of(
        classes: &[ClassModel],
        holders: &HashMap<String, usize>,
        functions: &[GlobalFn],
        types: &Scope,
        constants: &HashMap<String, Option<i64>>,
        earlier: &Known,
    ) -> Known {
        let mut known = Known {
            types: types.clone(),
            members: earlier.members.clone(),
            bases: earlier.bases.clone(),
            constants: constants.clone(),
            functions: earlier.functions.clone(),
            classes: earlier.classes.clone(),
        };
        for class in classes.iter().filter(|class| !class.forward) {
            // A modded class is the plain name, so what a later module sees is
            // the members of the whole chain, not the wrapper's own.
            known.members.insert(class.name.clone(), chain_members(classes, holders, class));
            if let Some(base) = chain_base(classes, holders, class) {
                known.bases.insert(class.name.clone(), base);
            }
            let record = known.classes.entry(class.name.clone()).or_default();
            record.functions = Rc::new(class.functions.clone());
            record.template = Rc::new(class.template.clone());
            // The base written in the module, which for a wrapper is the renamed
            // original: the walk continues through it, so its own functions are
            // still reachable from a later module.
            record.base = class.base.as_ref().map(|base| base.trim().to_string());
        }
        for declared in functions.iter().filter(|declared| !declared.function.is_proto()) {
            let function = &declared.function;
            known.functions.entry(function.name.clone()).or_default().push(FnSignature::of(types, function));
        }
        known
    }

    // Whether one of the earlier modules declares this global function with the
    // parameters this declaration would collide with.
    pub fn repeats_function(&self, name: &str, types: &[String]) -> bool {
        self.functions
            .get(name)
            .is_some_and(|signatures| signatures.iter().any(|signature| signature.covers(types)))
    }

    // The member names of a class and its ancestors, as far as the earlier
    // modules declare them. A cycle in the records ends the walk.
    pub fn inherited(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = vec![class.to_string()];
        let mut current = Some(class.to_string());
        while let Some(name) = current {
            let Some(names) = self.members.get(&name) else {
                break;
            };
            out.extend(names.iter().cloned());
            current = self.bases.get(&name).map(|base| base_name(base).to_string());
            if current.as_ref().is_some_and(|next| seen.contains(next)) {
                break;
            }
            if let Some(next) = current.as_ref() {
                seen.push(next.clone());
            }
        }
        out
    }
}

// The wrapper's own members first, then the class it wrapped. The merge points
// a modded class's base at the renamed original, so following that chain reaches
// the whole chain's members.
fn chain_members(classes: &[ClassModel], holders: &HashMap<String, usize>, class: &ClassModel) -> Vec<String> {
    let mut names = class.member_names();
    let mut seen = vec![class.name.clone()];
    let mut current = class.base_name().map(str::to_string);
    while let Some(name) = current {
        if seen.contains(&name) {
            break;
        }
        seen.push(name.clone());
        let Some(parent) = holders.get(&name).map(|at| &classes[*at]) else {
            break;
        };
        names.extend(parent.member_names());
        current = parent.base_name().map(str::to_string);
    }
    names
}

// The base a later module sees: the base of the plain class at the bottom of
// the chain, not the renamed original the merge pointed the wrapper at.
fn chain_base(classes: &[ClassModel], holders: &HashMap<String, usize>, class: &ClassModel) -> Option<String> {
    let mut seen = vec![class.name.clone()];
    let mut current = class;
    loop {
        let base = current.base.clone()?;
        match holders.get(base_name(&base)).map(|at| &classes[*at]) {
            Some(parent) => {
                if seen.contains(&parent.name) {
                    return None;
                }
                seen.push(parent.name.clone());
                current = parent;
            }
            None => return Some(base_name(&base).to_string()),
        }
    }
}
