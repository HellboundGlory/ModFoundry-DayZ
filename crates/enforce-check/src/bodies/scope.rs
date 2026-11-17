use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::index::{self, split_arguments, Index, Scope};
use crate::input::SourceFile;
use crate::parser::{FileAst, Item, TypeRef, VarDecl};

use super::templates::TemplateSource;

// A type name without its modifiers, template arguments or array suffix.
pub fn base_name(text: &str) -> &str {
    index::base_name(strip_modifiers(text))
}

fn strip_modifiers(text: &str) -> &str {
    let mut name = text.trim();
    loop {
        let Some((word, rest)) = name.split_once(char::is_whitespace) else {
            return name;
        };
        if matches!(word, "ref" | "autoptr" | "const" | "notnull" | "owned") {
            name = rest.trim();
        } else {
            return name;
        }
    }
}

// The built-in type names a body can declare a variable of. The containers and
// the two roots are declared by `1_Core`, so a module without vanilla still
// knows them.
pub fn is_primitive(text: &str) -> bool {
    let name = base_name(text);
    index::is_primitive(name) || matches!(name, "array" | "set" | "map" | "Class" | "Managed")
}

// The value types a class can be built on (`class Byte : int`).
pub fn is_value_type(name: &str) -> bool {
    matches!(base_name(name), "int" | "float" | "bool" | "string" | "vector" | "typename")
}

// A type as far as a body needs it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Ty {
    #[default]
    Unknown,
    Void,
    Null,
    Array(Box<Ty>),
    Container(String, Vec<Ty>),
    Named(String),
    // A type used as a value: `typename` carrying the name it names.
    Typename(String),
}

impl Ty {
    pub fn named(text: &str) -> Ty {
        match split_type(strip_modifiers(text)) {
            ("", None, false) => Ty::Unknown,
            ("void", None, false) => Ty::Void,
            (name, Some(inner), array) => {
                let arguments = split_arguments(inner).into_iter().map(Ty::named).collect();
                Ty::Container(name.to_string(), arguments).arrayed(array)
            }
            (name, None, array) => Ty::Named(name.to_string()).arrayed(array),
        }
    }

    fn arrayed(self, array: bool) -> Ty {
        match array {
            true => Ty::Array(Box::new(self)),
            false => self,
        }
    }

    pub fn of_ref(type_: &TypeRef) -> Ty {
        let ty = match type_.template.is_empty() {
            true => Ty::named(&type_.name),
            false => Ty::Container(
                type_.name.clone(),
                type_.template.iter().map(Ty::of_ref).collect(),
            ),
        };
        ty.arrayed(type_.array)
    }

    // A declared variable's type, with one array level per declared dimension.
    pub fn of_var(var: &index::VarModel) -> Ty {
        Ty::named(var.type_name.as_deref().unwrap_or_default()).with_dims(var.dims.len())
    }

    pub fn of_decl(decl: &VarDecl) -> Ty {
        let Some(type_) = &decl.type_ else {
            return Ty::Unknown;
        };
        Ty::of_ref(type_).with_dims(decl.dims.len())
    }

    fn with_dims(self, dims: usize) -> Ty {
        (0..dims).fold(self, |ty, _| Ty::Array(Box::new(ty)))
    }

    pub fn base(&self) -> Option<&str> {
        match self {
            Ty::Named(name) | Ty::Container(name, _) => Some(name),
            Ty::Typename(_) => Some("typename"),
            Ty::Array(inner) => inner.base(),
            Ty::Void => Some("void"),
            _ => None,
        }
    }

    pub fn element(&self) -> Ty {
        match self {
            Ty::Array(inner) => (**inner).clone(),
            Ty::Container(_, arguments) => arguments.last().cloned().unwrap_or(Ty::Unknown),
            // One character of a string is itself a string.
            Ty::Named(name) if name == "string" => Ty::Named(name.clone()),
            _ => Ty::Unknown,
        }
    }

    // A container's template arguments, a map's key first.
    pub fn arguments(&self) -> Vec<Ty> {
        match self {
            Ty::Container(_, arguments) => arguments.clone(),
            Ty::Array(inner) => vec![(**inner).clone()],
            _ => Vec::new(),
        }
    }

    // An alias to a container carries the container's own parameters, which the
    // alias's name alone does not show.
    pub fn expanded(&self, world: &World) -> Ty {
        let Some(name) = self.base() else {
            return self.clone();
        };
        // An alias among a container's own arguments stands for its type too.
        if let Ty::Container(container, arguments) = self {
            let arguments = arguments.iter().map(|argument| argument.expanded(world)).collect();
            return Ty::Container(container.clone(), arguments);
        }
        let Some(target) = world.types.find(name).and_then(|symbol| symbol.target.as_deref()) else {
            return self.clone();
        };
        match self {
            Ty::Array(inner) => Ty::Array(Box::new(inner.expanded(world))),
            _ => Ty::named(target),
        }
    }

    // Template parameters replaced by what they stand for, inside containers
    // and arrays too. A parameter nothing binds stays as it is.
    pub fn substituted(&self, bindings: &[(String, Ty)]) -> Ty {
        match self {
            Ty::Named(name) => match bindings.iter().find(|(parameter, _)| parameter == name) {
                Some((_, ty)) => ty.clone(),
                None => self.clone(),
            },
            Ty::Container(name, arguments) => Ty::Container(
                name.clone(),
                arguments.iter().map(|argument| argument.substituted(bindings)).collect(),
            ),
            Ty::Array(inner) => Ty::Array(Box::new(inner.substituted(bindings))),
            _ => self.clone(),
        }
    }

    // Whether one of these names stands anywhere `substituted` would replace it.
    fn mentions(&self, names: &HashSet<String>) -> bool {
        match self {
            Ty::Named(name) => names.contains(name),
            Ty::Container(_, arguments) => arguments.iter().any(|argument| argument.mentions(names)),
            Ty::Array(inner) => inner.mentions(names),
            _ => false,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    // The type as the game words it in a message.
    pub fn text(&self) -> String {
        match self {
            Ty::Unknown | Ty::Null => "NULL".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Typename(_) => "typename".to_string(),
            Ty::Named(name) => name.clone(),
            Ty::Array(inner) => inner.text(),
            Ty::Container(name, arguments) => {
                let inner: Vec<String> = arguments.iter().map(Ty::text).collect();
                format!("{name}<{}>", inner.join(","))
            }
        }
    }
}

// `array<int>` or `Man[]` read back into a name, the text inside its `<...>` and
// the array flag.
fn split_type(text: &str) -> (&str, Option<&str>, bool) {
    let array = text.contains('[');
    let text = text.split('[').next().unwrap_or(text).trim();
    let Some(open) = text.find('<') else {
        return (text, None, array);
    };
    let name = text[..open].trim();
    let inner = &text[open..];
    let mut depth = 0i32;
    for (index, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return (name, Some(&inner[1..index]), array);
                }
            }
            _ => {}
        }
    }
    (name, None, array)
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub ty: Ty,
    pub modifiers: Vec<String>,
}

impl Member {
    pub fn has(&self, modifier: &str) -> bool {
        self.modifiers.iter().any(|word| word == modifier)
    }
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Ty,
    pub modifiers: Vec<String>,
    pub default: bool,
}

#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Ty,
    pub modifiers: Vec<String>,
    pub body: bool,
    pub destructor: bool,
}

impl Param {
    fn type_word(&self) -> &str {
        self.ty.base().unwrap_or("void")
    }
}

impl Method {
    pub fn has(&self, modifier: &str) -> bool {
        self.modifiers.iter().any(|word| word == modifier)
    }

    pub fn is_static(&self) -> bool {
        self.has("static")
    }

    pub fn is_native(&self) -> bool {
        self.has("native") || self.has("proto")
    }

    pub fn required(&self) -> usize {
        self.params.iter().filter(|param| !param.default).count()
    }

    // The candidate line the game prints after `Ambiguous ... candidates:`.
    pub fn signature(&self) -> String {
        let types: Vec<&str> = self.params.iter().map(Param::type_word).collect();
        format!("{}({})", self.name, types.join(","))
    }

    // Whether both would print the same candidate line.
    pub fn same_signature(&self, other: &Method) -> bool {
        self.name == other.name
            && self.params.len() == other.params.len()
            && self.params.iter().zip(&other.params).all(|(left, right)| left.type_word() == right.type_word())
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypeInfo {
    pub name: String,
    pub base: Option<String>,
    pub members: Vec<Member>,
    pub methods: Vec<Method>,
    pub values: Vec<String>,
    pub is_enum: bool,
    pub template: Vec<String>,
    // Declared in 1_Core or 2_GameLib.
    pub early: bool,
}

impl TypeInfo {
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.name == name)
    }

    pub fn methods(&self, name: &str) -> Vec<&Method> {
        self.methods
            .iter()
            .filter(|method| method.name == name)
            .collect()
    }
}

// Everything one module's bodies can see: its own declarations plus those of the
// modules before it, and the type names that resolve.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub types: Scope,
    // Every class and enum, the place of each by name, and for each place the
    // class and its ancestors, nearest first. `add` keeps the three in step.
    infos: Vec<TypeInfo>,
    places: HashMap<String, usize>,
    chains: Vec<Vec<usize>>,
    pub globals: HashMap<String, Member>,
    pub functions: HashMap<String, Vec<Method>>,
    // Enum value name -> the enum it belongs to.
    pub enum_values: HashMap<String, String>,
    // The class name the merge gave each declaration, keyed by file and line.
    // The game prints whichever name the merge settled on.
    pub merged_names: HashMap<(String, u32), String>,
    // Every template parameter name any declared class uses.
    pub template_names: HashSet<String>,
    // Names the engine declares as `typedef int[] Name;`: handles it hands out,
    // which behave as numbers whatever class shares the name.
    pub handles: HashSet<String>,
    // `const int` names an array size may read, with the value when literal.
    pub constants: HashMap<String, Option<i64>>,
    // The variables declared at module level. `globals` also answers for the
    // constant members of every class.
    pub module_variables: HashSet<String>,
    // The input names the loaded mods register.
    pub input_names: HashSet<String>,
    // Every template class as written, under the name the merge gave it: its
    // body is compiled where an instance of it is first named.
    pub templates: HashMap<String, Rc<TemplateSource>>,
}

impl World {
    // Each module's declarations join the ones before it, so a body in a later
    // layer resolves a name the earlier modules declared.
    pub fn add(&mut self, index: &Index) {
        self.types = index.types.clone();
        self.constants = index.constants.clone();
        for class in &index.classes {
            self.merged_names
                .insert((class.file.clone(), class.line), class.name.clone());
            let at = self.place(&class.name);
            let info = &mut self.infos[at];
            info.name = class.name.clone();
            info.base = class.base.clone();
            info.template = class.template.clone();
            info.early = matches!(class.module, crate::input::Module::Core | crate::input::Module::GameLib);
            info.members = class
                .members
                .iter()
                .map(|member| Member {
                    name: member.name.clone(),
                    ty: Ty::of_var(member),
                    modifiers: member.modifiers.clone(),
                })
                .collect();
            info.methods = class.functions.iter().map(method).collect();
        }
        for typedef in &index.typedefs {
            let target: String = typedef.target.iter().flat_map(|target| target.chars()).filter(|c| !c.is_whitespace()).collect();
            if target == "int[]" {
                self.handles.insert(typedef.name.clone());
            }
        }
        for enumeration in &index.enums {
            let at = self.place(&enumeration.name);
            let info = &mut self.infos[at];
            info.name = enumeration.name.clone();
            info.base = enumeration.base.clone();
            info.is_enum = true;
            info.values = enumeration
                .members
                .iter()
                .map(|member| member.name.clone())
                .collect();
            for value in &info.values {
                self.enum_values
                    .insert(value.clone(), enumeration.name.clone());
            }
        }
        for global in &index.globals {
            self.module_variables.insert(global.var.name.clone());
            self.globals.insert(
                global.var.name.clone(),
                Member {
                    name: global.var.name.clone(),
                    ty: Ty::of_var(&global.var),
                    modifiers: global.var.modifiers.clone(),
                },
            );
        }
        for name in index.constants.keys() {
            self.globals.entry(name.clone()).or_insert(Member {
                name: name.clone(),
                ty: Ty::Named("int".to_string()),
                modifiers: vec!["const".to_string()],
            });
        }
        for declared in &index.functions {
            let function = method(&declared.function);
            self.functions.entry(function.name.clone()).or_default().push(function);
        }
        self.template_names = self.infos.iter().flat_map(|info| info.template.iter().cloned()).collect();
        self.chains = self.infos.iter().map(|info| self.ancestry(&info.name)).collect();
    }

    // Where a class or enum is kept, making room for a new one.
    fn place(&mut self, name: &str) -> usize {
        if let Some(at) = self.places.get(name) {
            return *at;
        }
        let at = self.infos.len();
        self.infos.push(TypeInfo::default());
        self.places.insert(name.to_string(), at);
        at
    }

    // The class and its ancestors, nearest first, as places in `infos`.
    fn ancestry(&self, name: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = vec![name];
        let mut current = Some(name);
        while let Some(name) = current {
            let Some(at) = self.places.get(name) else {
                break;
            };
            out.push(*at);
            // A script class with no written base still derives from `Class`. A
            // base given through an alias is followed to the class it names.
            current = match self.infos[*at].base.as_deref().map(base_name) {
                Some(base) => Some(self.class_of(base).unwrap_or(base)),
                None if name != "Class" => Some("Class"),
                None => None,
            };
            if current.is_some_and(|next| seen.contains(&next)) {
                break;
            }
            seen.extend(current);
        }
        out
    }

    // Keeps what a module's template classes wrote. Runs after `add`, which
    // records the names the merge settled on.
    pub fn add_templates(&mut self, files: &[(&SourceFile, &FileAst)]) {
        for (file, ast) in files {
            for item in &ast.items {
                let Item::Class(decl) = item else {
                    continue;
                };
                if decl.template.is_empty() || decl.forward {
                    continue;
                }
                let name = self.merged_name(&file.path, decl.line).unwrap_or(&decl.name).to_string();
                let source = TemplateSource {
                    file: file.path.clone(),
                    decl: decl.clone(),
                };
                self.templates.insert(name, Rc::new(source));
            }
        }
    }

    // The name the merge gave the class declared at this file and line.
    pub fn merged_name(&self, file: &str, line: u32) -> Option<&str> {
        self.merged_names
            .get(&(file.to_string(), line))
            .map(String::as_str)
    }

    pub fn info(&self, name: &str) -> Option<&TypeInfo> {
        self.places.get(name).map(|at| &self.infos[*at])
    }

    // The plain name of a class's base, as the chain and lookups use it.
    pub fn base_of(&self, name: &str) -> Option<String> {
        self.info(name)
            .and_then(|info| info.base.as_deref())
            .map(|base| base_name(base).to_string())
    }

    // The base as written, template arguments included.
    pub fn base_type(&self, name: &str) -> Option<Ty> {
        self.info(name)
            .and_then(|info| info.base.as_deref())
            .map(Ty::named)
    }

    // A real class, not one of the built-in pseudo-classes the engine declares
    // as `class bool` / `class int` / `class float`.
    pub fn is_class(&self, name: &str) -> bool {
        !is_value_type(name)
            && !matches!(base_name(name), "void" | "auto" | "func")
            && self.info(name).is_some_and(|info| !info.is_enum)
    }

    // The chain ends at a base no script file declares: the engine's own class.
    // Nothing is known about what it and its ancestors declare, so a missing
    // member on the one below it proves nothing.
    pub fn reaches_engine_base(&self, name: &str) -> bool {
        let Some(class) = self.class_of(name) else {
            return true;
        };
        let mut current = Some(class.to_string());
        let mut seen = Vec::new();
        while let Some(name) = current {
            if seen.contains(&name) {
                return false;
            }
            seen.push(name.clone());
            let Some(info) = self.info(&name) else {
                return true;
            };
            current = info.base.as_deref().map(|base| base_name(base).to_string());
        }
        false
    }

    // An alias names a type the module chain already recorded; the members and
    // methods of that type are the alias's own.
    pub fn resolve(&self, name: &str) -> Option<&str> {
        self.types
            .find(name)
            .and_then(|symbol| symbol.target.as_deref())
            .map(base_name)
    }

    // The class a name reaches through its aliases, however many there are.
    pub fn class_of(&self, name: &str) -> Option<&str> {
        let mut current = base_name(name);
        for _ in 0..16 {
            if let Some(info) = self.info(current) {
                return Some(info.name.as_str());
            }
            let next = self.resolve(current)?;
            if next == current {
                return None;
            }
            current = next;
        }
        None
    }

    pub fn member_of(&self, class: &str, name: &str) -> Option<&Member> {
        self.chain(class)
            .into_iter()
            .find_map(|info| info.member(name))
    }

    // The member a receiver of this type reaches, with a field declared as one
    // of its class's template parameters taking the argument the receiver's own
    // type supplied. `receiver` is the type as written at the use site, so its
    // arguments line up with the declaring class's parameters.
    pub fn member_for(&self, receiver: &Ty, class: &str, name: &str) -> Option<Member> {
        let mut member = self.member_of(class, name)?.clone();
        if !member.ty.mentions(&self.template_names) {
            return Some(member);
        }
        let owner = self
            .owner_of(class, name)
            .unwrap_or_else(|| self.class_of(class).unwrap_or(class));
        member.ty = self.bound_type(receiver, class, owner, &member.ty);
        Some(member)
    }

    // Every method of this name the class and its ancestors declare, with the
    // level each came from: 0 is the class itself, 1 its base, and so on. An
    // ancestor's overload joins the set unless its signature is already there,
    // and the level lets a caller prefer the most-derived candidates.
    pub fn methods_of(&self, class: &str, name: &str) -> Vec<&Method> {
        self.methods_with_level(class, name)
            .into_iter()
            .map(|(method, _)| method)
            .collect()
    }

    pub fn methods_with_level(&self, class: &str, name: &str) -> Vec<(&Method, usize)> {
        let mut found: Vec<(&Method, usize)> = Vec::new();
        for (level, info) in self.chain(class).into_iter().enumerate() {
            for method in info.methods.iter().filter(|method| method.name == name) {
                if !found.iter().any(|(known, _)| known.same_signature(method)) {
                    found.push((method, level));
                }
            }
        }
        found
    }

    // Whether `Class(args)` builds this class: the class writes that constructor
    // itself, or writes none and inherits its base's. A constructor carries the
    // name of the class that wrote it, and a class that writes other
    // constructors has chosen its own, so an inherited one does not apply.
    pub fn builds_with(&self, class: &str, call: &str) -> bool {
        let Some(name) = self.class_of(class) else {
            return false;
        };
        let mut chain = self.chain(name).into_iter();
        let Some(own) = chain.next() else {
            return false;
        };
        let own_constructors: Vec<&str> = own
            .methods
            .iter()
            .filter(|method| !method.destructor && method.name == own.name)
            .map(|method| method.name.as_str())
            .collect();
        if own_constructors.contains(&call) {
            return true;
        }
        // A class that writes no constructor of its own takes its base's.
        own_constructors.is_empty()
            && chain.any(|info| {
                info.methods
                    .iter()
                    .any(|method| !method.destructor && method.name == info.name)
            })
    }

    // A method whose declared return type is one of its class's template
    // parameters hands back the argument the receiver's own type supplied.
    pub fn return_for(&self, receiver: &Ty, class: &str, method: &Method) -> Ty {
        if !method.ret.mentions(&self.template_names) {
            return method.ret.clone();
        }
        let Some(owner) = self.method_owner(class, method) else {
            return method.ret.clone();
        };
        self.bound_type(receiver, class, owner, &method.ret)
    }

    // The class in the chain that holds this very method.
    pub fn method_owner(&self, class: &str, method: &Method) -> Option<&str> {
        self.chain(class)
            .into_iter()
            .find(|info| info.methods.iter().any(|own| own.same_signature(method)))
            .map(|info| info.name.as_str())
    }

    // What the template parameters of `owner` stand for when the receiver's
    // class reaches `owner` through its bases: each class hands the arguments it
    // wrote for its base on, written in terms of its own parameters.
    fn bindings_at(&self, receiver: &Ty, class: &str, owner: &str) -> Vec<(String, Ty)> {
        let chain = self.chain(class);
        let Some(first) = chain.first() else {
            return Vec::new();
        };
        let mut bindings: Vec<(String, Ty)> = first
            .template
            .iter()
            .cloned()
            .zip(receiver.expanded(self).arguments())
            .collect();
        for (at, info) in chain.iter().enumerate() {
            if info.name == owner {
                return bindings;
            }
            let Some(next) = chain.get(at + 1) else {
                return Vec::new();
            };
            let written = info.base.as_deref().and_then(|base| split_type(strip_modifiers(base)).1);
            let handed: Vec<Ty> = written
                .map(split_arguments)
                .unwrap_or_default()
                .into_iter()
                .map(|argument| Ty::named(argument).substituted(&bindings))
                .collect();
            bindings = next.template.iter().cloned().zip(handed).collect();
        }
        Vec::new()
    }

    // A declared type with the owner's template parameters replaced, at any
    // depth: `LNode<T>` becomes `LNode<LX>`.
    fn bound_type(&self, receiver: &Ty, class: &str, owner: &str, ty: &Ty) -> Ty {
        let mut bindings = self.bindings_at(receiver, class, owner);
        // A parameter the receiver gave no argument for is a type nothing knows.
        if let Some(info) = self.info(owner) {
            for parameter in &info.template {
                if !bindings.iter().any(|(name, _)| name == parameter) {
                    bindings.push((parameter.clone(), Ty::Unknown));
                }
            }
        }
        ty.substituted(&bindings)
    }

    // Whether the class or one of its ancestors is built on a template instance.
    pub fn has_template_ancestor(&self, class: &str) -> bool {
        self.chain(class).iter().any(|info| {
            info.base
                .as_deref()
                .is_some_and(|base| matches!(Ty::named(base).expanded(self), Ty::Container(..)))
        })
    }

    // The class in the chain that declares this name.
    pub fn owner_of(&self, class: &str, name: &str) -> Option<&str> {
        self.chain(class)
            .into_iter()
            .find(|info| {
                info.member(name).is_some() || !info.methods(name).is_empty()
            })
            .map(|info| info.name.as_str())
    }

    pub fn enum_values(&self, name: &str) -> Vec<String> {
        self.chain(name)
            .into_iter()
            .flat_map(|info| info.values.iter().cloned())
            .collect()
    }

    // The class a name reaches through its aliases, and its ancestors, nearest
    // first.
    pub fn chain(&self, name: &str) -> Vec<&TypeInfo> {
        let Some(at) = self.class_of(name).and_then(|class| self.places.get(class)) else {
            return Vec::new();
        };
        self.chains[*at].iter().map(|at| &self.infos[*at]).collect()
    }
}

fn method(function: &index::FnModel) -> Method {
    Method {
        name: function.name.clone(),
        params: function
            .params
            .iter()
            .map(|param| Param {
                name: param.name.clone(),
                ty: Ty::of_var(param),
                modifiers: param.modifiers.clone(),
                default: param.has_init,
            })
            .collect(),
        ret: Ty::named(function.return_type.as_deref().unwrap_or_default()),
        modifiers: function.modifiers.clone(),
        body: function.body,
        destructor: function.destructor,
    }
}

// The names declared by one function. Locals share one namespace for the whole
// function, so a block does not hide the names of the block before it.
#[derive(Clone, Debug, Default)]
pub struct Locals {
    entries: Vec<Member>,
    // The first dimension of each static array declared with a literal size.
    sizes: HashMap<String, i64>,
    // How many of the entries are the function's parameters; they come first.
    parameters: usize,
}

impl Locals {
    pub fn declare(&mut self, member: Member) {
        self.entries.push(member);
    }

    pub fn declare_size(&mut self, name: &str, size: i64) {
        self.sizes.insert(name.to_string(), size);
    }

    pub fn size(&self, name: &str) -> Option<i64> {
        self.sizes.get(name).copied()
    }

    pub fn find(&self, name: &str) -> Option<&Member> {
        self.entries.iter().find(|member| member.name == name)
    }

    // Every name declared so far is a parameter.
    pub fn end_parameters(&mut self) {
        self.parameters = self.entries.len();
    }

    pub fn declared_in_body(&self, name: &str) -> bool {
        self.entries.iter().position(|member| member.name == name).is_some_and(|at| at >= self.parameters)
    }
}
