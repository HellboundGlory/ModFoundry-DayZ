use std::cmp::Ordering;

use crate::diag::Diagnostic;
use crate::input::Module;

use super::model::{ClassModel, FnModel};
use super::resolve::base_name;
use super::rules::{push, Declarations, Level, Line};

// Where a type is written decides how often the game resolves it, and so how
// many lines a wrong argument count prints.
#[derive(Clone, Copy)]
pub enum Place<'a> {
    // A member, a global or a return type.
    Variable,
    Parameter,
    Local,
    // The base of the class named.
    Base(&'a str),
}

// A template class is checked where it is written: every type that names one
// must pass it the parameter count the class declares.
pub(super) fn rules(declarations: &Declarations) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // The template classes a `modded` line wrote parameters on: the name is no
    // type anywhere in the module. A strict target loses the name without
    // them too.
    let lost: Vec<&str> = declarations
        .classes
        .iter()
        .filter(|class| (declarations.strict || !class.template.is_empty()) && wrapped_template(declarations, class).is_some())
        .filter_map(|class| class.wrapped.as_ref().map(|(name, _)| name.as_str()))
        .collect();
    // What follows from a lost name is printed after every class is read.
    let mut follow = Vec::new();
    for class in &declarations.classes {
        match &class.wrapped {
            Some(_) => wrapper(declarations, class, &mut out),
            None => {
                let place = Place::Base(&class.name);
                report(declarations, class.base.as_deref(), &class.file, class.line, class.module, place, &mut out);
            }
        }
        for member in &class.members {
            report(declarations, member.type_name.as_deref(), &class.file, member.line, class.module, Place::Variable, &mut out);
            let Some(name) = member.type_name.as_deref().map(base_name).filter(|name| lost.contains(name)) else {
                continue;
            };
            if member.has_any_modifier() {
                push(&mut follow, &class.file, member.line, class.module, Level::Error, format!("Bad type '{name}'"));
            }
            push(&mut follow, &class.file, member.line, class.module, Level::Error, format!("Unknown type '{name}'"));
        }
        let mut check = |type_name: Option<&str>, line: u32, place: Place| {
            report(declarations, type_name, &class.file, line, class.module, place, &mut out);
        };
        for function in &class.functions {
            signature(function, &mut check);
            lost_parameters(function, &lost, &class.file, class.module, &mut follow);
        }
    }
    for global in &declarations.globals {
        let var = &global.var;
        report(declarations, var.type_name.as_deref(), &global.file, var.line, global.module, Place::Variable, &mut out);
    }
    for global in &declarations.functions {
        let mut check = |type_name: Option<&str>, line: u32, place: Place| {
            report(declarations, type_name, &global.file, line, global.module, place, &mut out);
        };
        signature(&global.function, &mut check);
        lost_parameters(&global.function, &lost, &global.file, global.module, &mut follow);
    }
    out.extend(follow);
    out
}

// A `modded` class takes the class it wraps as its base, which a template
// class cannot be.
fn wrapper(declarations: &Declarations, class: &ClassModel, out: &mut Vec<Diagnostic>) {
    let Some((name, expected)) = wrapped_template(declarations, class) else {
        return;
    };
    let mut print = |file: &str, line: u32, found: Line| push_line(out, file, line, class.module, found, false);
    if !class.template.is_empty() {
        for found in lines(name, 0, expected, Place::Variable) {
            print(&class.file, class.line, found);
        }
        return;
    }
    // The second and third line sit where the module's last file ends. A
    // strict target stops at the first line, which is an error there.
    let here = (class.file.clone(), class.line);
    let end = declarations.end.as_ref().unwrap_or(&here);
    let printed = if declarations.strict { 1 } else { usize::MAX };
    for (at, found) in lines(name, 0, expected, Place::Base(&class.name)).into_iter().enumerate().take(printed) {
        let (file, line) = if at == 1 || at == 2 { end } else { &here };
        print(file, *line, found);
    }
}

// A parameter typed with a name a `modded` line lost.
fn lost_parameters(function: &FnModel, lost: &[&str], file: &str, module: Module, out: &mut Vec<Diagnostic>) {
    for param in &function.params {
        let Some(name) = param.type_name.as_deref().map(base_name).filter(|name| lost.contains(name)) else {
            continue;
        };
        push(out, file, param.line, module, Level::Error, format!("Unknown type '{name}'"));
        push(out, file, param.line, module, Level::Error, "Error in parameters".to_string());
    }
}

// The template class a `modded` class wraps, under the name the merge gave
// it, with its parameter count.
fn wrapped_template<'a>(declarations: &Declarations, class: &'a ClassModel) -> Option<(&'a str, usize)> {
    class.wrapped.as_ref()?;
    let name = base_name(class.base.as_deref()?);
    let expected = parameters_of(declarations, name);
    (expected > 0).then_some((name, expected))
}

fn push_line(out: &mut Vec<Diagnostic>, file: &str, line: u32, module: Module, found: Line, keeps_loading: bool) {
    let level = match (found.soft, keeps_loading) {
        (true, true) => Level::SoftKeepsLoading,
        (true, false) => Level::Soft,
        (false, _) => Level::Error,
    };
    push(out, file, line, module, level, found.message);
}

// How many parameters a class declares. The declaration may sit in an earlier
// module, which leaves only its parameter names behind.
fn parameters_of(declarations: &Declarations, name: &str) -> usize {
    match declarations.holder(name) {
        Some(declared) => declared.template.len(),
        None => declarations.known.classes.get(name).map_or(0, |record| record.template.len()),
    }
}

fn signature(function: &FnModel, check: &mut impl FnMut(Option<&str>, u32, Place)) {
    check(function.return_type.as_deref(), function.line, Place::Variable);
    for param in &function.params {
        check(param.type_name.as_deref(), param.line, Place::Parameter);
    }
}

fn report(
    declarations: &Declarations,
    type_name: Option<&str>,
    file: &str,
    line: u32,
    module: Module,
    place: Place,
    out: &mut Vec<Diagnostic>,
) {
    let Some(type_name) = type_name else {
        return;
    };
    let name = base_name(type_name);
    let expected = parameters_of(declarations, name);
    // A strict target stops at the first line about a base, which is an
    // error there whichever way the count is wrong.
    let stops = declarations.strict && matches!(place, Place::Base(_));
    let printed = lines(name, template_arguments(type_name), expected, place);
    // Anywhere else, arguments past the declared ones are dropped, and no
    // target fails the module over the lines about them.
    let dropped = !stops && printed.iter().all(|found| found.soft);
    let count = if stops { 1 } else { printed.len() };
    for found in printed.into_iter().take(count) {
        push_line(out, file, line, module, found, dropped);
    }
}

// What the game prints for a type that gives `written` arguments to a class
// declaring `expected` parameters. A class that declares none takes any.
pub fn lines(name: &str, written: usize, expected: usize, place: Place) -> Vec<Line> {
    let soft = |message: String| Line { message, soft: true };
    let hard = |message: &str| Line {
        message: message.to_string(),
        soft: false,
    };
    if expected == 0 {
        return Vec::new();
    }
    let many = || soft(format!("Too many template parameters for '{name}' type"));
    let few = || soft(format!("Not enough parameters for template class '{name}'"));
    let wrong = || hard("Wrong number of template parameters");
    match (written.cmp(&expected), place) {
        (Ordering::Equal, _) => Vec::new(),
        // The extra arguments are dropped and the module compiles.
        (Ordering::Greater, Place::Base(_)) => vec![many(), many(), many()],
        (Ordering::Greater, _) => vec![many(), many()],
        (Ordering::Less, Place::Variable) => vec![few(), wrong()],
        (Ordering::Less, Place::Parameter) => vec![few(), wrong(), hard("Error in parameters")],
        (Ordering::Less, Place::Local) => vec![few(), wrong(), few(), wrong(), hard("Error in parameters")],
        (Ordering::Less, Place::Base(class)) => vec![
            few(),
            few(),
            hard("Wrong number of parameters"),
            few(),
            wrong(),
            hard(&format!("Can't inherit class '{class}'")),
        ],
    }
}

// The written argument count of a type, counted at the outer level only: a
// nested `array<int>` is one argument, and a bare name gives none.
fn template_arguments(type_name: &str) -> usize {
    let Some(open) = type_name.find('<') else {
        return 0;
    };
    let mut depth = 0usize;
    let mut count = 1usize;
    for character in type_name[open..].chars() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 1 => count += 1,
            _ => {}
        }
    }
    count
}
