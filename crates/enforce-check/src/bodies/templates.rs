use std::collections::HashSet;

use crate::diag::Severity;
use crate::parser::{ClassDecl, FileAst, Item, TypeRef, VarDecl};

use super::scope::{is_primitive, Locals, Ty};
use super::{statements, BodyChecker};

// A template class as written, kept for the modules that use it.
#[derive(Clone, Debug)]
pub struct TemplateSource {
    pub file: String,
    pub decl: ClassDecl,
}

// The instances compiled so far, under the name the game gives each. An
// instance is compiled once, in the module that names it first.
#[derive(Debug, Default)]
pub struct Instances {
    done: HashSet<String>,
    // The aliases being followed to their target right now.
    expanding: Vec<String>,
    // The aliases whose target was already compiled.
    expanded: HashSet<String>,
}

// Deeper than any script nests one template in another; an instance compiled
// this far inside others is a type that grows with every instance, which
// crashes the game.
const DEPTH_LIMIT: u32 = 16;

// Past this many instances no new one is compiled. The largest load measured
// has 1,234.
const INSTANCE_LIMIT: usize = 4096;

// What the checker holds about the body it is reading. An instance is compiled
// in the middle of whatever named it, so this is set aside and put back.
#[derive(Default)]
pub(super) struct Frame {
    file: String,
    class: Option<String>,
    template_params: Vec<String>,
    bindings: Vec<(String, Ty)>,
    instance: Option<String>,
    report_line: Option<u32>,
    calls_open: u32,
    instructions: usize,
    variables: usize,
    bare_ifs: usize,
    bare_ifs_line: u32,
    static_method: bool,
    function_name: String,
    returns_value: bool,
    return_type: Ty,
    enclosing: Vec<statements::Enclosing>,
    locals: Locals,
    statement_line: u32,
}

impl BodyChecker<'_> {
    // A written type, with the parameters of the instance being compiled
    // replaced by its arguments.
    pub(super) fn ty_of_ref(&self, type_: &TypeRef) -> Ty {
        Ty::of_ref(type_).substituted(&self.bindings)
    }

    pub(super) fn ty_of_decl(&self, decl: &VarDecl) -> Ty {
        Ty::of_decl(decl).substituted(&self.bindings)
    }

    // The class being read as a type: an instance carries its arguments, so a
    // member typed by a parameter resolves to what the instance was given.
    pub(super) fn own_type(&self, class: &str) -> Ty {
        match self.instance.is_some() && self.class.as_deref() == Some(class) {
            true => Ty::Container(class.to_string(), self.bindings.iter().map(|(_, ty)| ty.clone()).collect()),
            false => Ty::Named(class.to_string()),
        }
    }

    // The name the game prints for a class: an instance's carries its arguments.
    pub(super) fn class_label(&self, class: &str) -> String {
        match (&self.instance, self.class.as_deref() == Some(class)) {
            (Some(instance), true) => instance.clone(),
            _ => class.to_string(),
        }
    }

    // The instances a file's typedefs name, which the game compiles first.
    pub(super) fn aliased_instances(&mut self, ast: &FileAst) {
        for item in &ast.items {
            if let Item::Typedef(decl) = item
                && let Some(type_) = &decl.type_
            {
                self.instantiate(&Ty::of_ref(type_));
            }
        }
    }

    // The instances a file's declarations name: bases, members, parameters and
    // return types. The game compiles these before any function body.
    pub(super) fn declared_instances(&mut self, ast: &FileAst) {
        for item in &ast.items {
            match item {
                Item::Class(decl) if decl.template.is_empty() => {
                    if let Some(base) = &decl.base {
                        self.instantiate(&Ty::of_ref(base));
                    }
                    for member in &decl.members {
                        self.declared_types(member);
                    }
                }
                Item::Class(_) => {}
                _ => self.declared_types(item),
            }
        }
    }

    fn declared_types(&mut self, item: &Item) {
        match item {
            Item::Var(decl) => {
                let ty = self.ty_of_decl(decl);
                self.instantiate(&ty);
            }
            Item::Function(decl) => {
                if let Some(type_) = &decl.return_type {
                    let ty = self.ty_of_ref(type_);
                    self.instantiate(&ty);
                }
                for param in &decl.params {
                    let ty = self.ty_of_decl(param);
                    self.instantiate(&ty);
                }
            }
            _ => {}
        }
    }

    // Compiles the template class a type names with the arguments it gives,
    // the first time that pair is met. False when that compile failed; a later
    // name for the same instance says nothing more.
    pub(super) fn instantiate(&mut self, ty: &Ty) -> bool {
        match ty {
            Ty::Array(inner) => self.instantiate(inner),
            Ty::Container(name, arguments) => {
                for argument in arguments {
                    self.instantiate(argument);
                }
                self.instance(name, arguments)
            }
            Ty::Named(name) => match ty.expanded(self.world) {
                expanded @ Ty::Container(..)
                    if self.world.resolve(name).is_some() && !self.instances.expanding.contains(name)
                        && self.instances.expanded.insert(name.clone()) =>
                {
                    self.instances.expanding.push(name.clone());
                    let compiled = self.instantiate(&expanded);
                    self.instances.expanding.pop();
                    compiled
                }
                _ => true,
            },
            _ => true,
        }
    }

    fn instance(&mut self, name: &str, arguments: &[Ty]) -> bool {
        let world = self.world;
        let Some(class) = world.class_of(name) else {
            return true;
        };
        let Some(source) = world.templates.get(class) else {
            return true;
        };
        let parameters: Vec<String> = source.decl.template.iter().map(|parameter| parameter.name.clone()).collect();
        if parameters.len() != arguments.len() || !arguments.iter().all(|argument| self.is_concrete(argument)) {
            return true;
        }
        // An alias among the arguments stands for its type: both name one
        // instance, printed under the type.
        let arguments: Vec<Ty> = arguments.iter().map(|argument| argument.expanded(world)).collect();
        let key = Ty::Container(class.to_string(), arguments.clone()).text();
        if self.crashed || self.instances.done.len() >= INSTANCE_LIMIT || self.instances.done.contains(&key) {
            return true;
        }
        // Only a template that names itself with a larger argument each time
        // gets this deep, and the game compiles those until it falls over.
        if self.depth >= DEPTH_LIMIT {
            self.crash(format!("Template class '{class}' names itself with a larger argument in every instance"));
            return true;
        }
        self.instances.done.insert(key.clone());
        let mut frame = Frame {
            file: source.file.clone(),
            class: Some(class.to_string()),
            bindings: parameters.iter().cloned().zip(arguments).collect(),
            template_params: parameters,
            instance: Some(key.clone()),
            // Every line an instance prints sits on the class's closing line.
            report_line: Some(source.decl.end_line),
            statement_line: source.decl.end_line,
            ..Frame::default()
        };
        self.swap(&mut frame);
        self.depth += 1;
        let printed = self.diagnostics.len();
        self.instance_members(&source.decl);
        let strict = self.strict;
        let failed = self.diagnostics[printed..]
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error && !diagnostic.reports_only && (strict || !diagnostic.fixme));
        if failed {
            self.error(format!("...while compiling template class '{key}'"));
        }
        self.depth -= 1;
        self.swap(&mut frame);
        !failed
    }

    fn instance_members(&mut self, decl: &ClassDecl) {
        if let Some(base) = &decl.base {
            let ty = self.ty_of_ref(base);
            self.instantiate(&ty);
        }
        for member in &decl.members {
            self.declared_types(member);
        }
        for member in &decl.members {
            match member {
                Item::Function(function) => self.function(function),
                Item::Var(variable) => self.initialiser(variable),
                _ => {}
            }
        }
    }

    // A type with nothing left open: every name in it is a real type, none a
    // template parameter still waiting for its argument.
    fn is_concrete(&self, ty: &Ty) -> bool {
        let known = |name: &str| {
            !self.world.template_names.contains(name) && (is_primitive(name) || self.world.types.is_known(name))
        };
        match ty {
            Ty::Named(name) => known(name),
            Ty::Container(name, arguments) => known(name) && arguments.iter().all(|argument| self.is_concrete(argument)),
            Ty::Array(inner) => self.is_concrete(inner),
            _ => false,
        }
    }

    fn swap(&mut self, frame: &mut Frame) {
        use std::mem::swap;
        swap(&mut self.file, &mut frame.file);
        swap(&mut self.class, &mut frame.class);
        swap(&mut self.template_params, &mut frame.template_params);
        swap(&mut self.bindings, &mut frame.bindings);
        swap(&mut self.instance, &mut frame.instance);
        swap(&mut self.report_line, &mut frame.report_line);
        swap(&mut self.calls_open, &mut frame.calls_open);
        swap(&mut self.instructions, &mut frame.instructions);
        swap(&mut self.variables, &mut frame.variables);
        swap(&mut self.bare_ifs, &mut frame.bare_ifs);
        swap(&mut self.bare_ifs_line, &mut frame.bare_ifs_line);
        swap(&mut self.static_method, &mut frame.static_method);
        swap(&mut self.function_name, &mut frame.function_name);
        swap(&mut self.returns_value, &mut frame.returns_value);
        swap(&mut self.return_type, &mut frame.return_type);
        swap(&mut self.enclosing, &mut frame.enclosing);
        swap(&mut self.locals, &mut frame.locals);
        swap(&mut self.statement_line, &mut frame.statement_line);
    }
}
