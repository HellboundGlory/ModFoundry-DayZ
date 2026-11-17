use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity};
use crate::input::Module;

use super::model::{ClassModel, EnumModel, TypedefModel};

// The three "Circle inheritance" messages, raised while the declarations are
// ordered. An edge is a base class, or a `static`/`const` member initialiser of
// the form `Other.Member` (a class-level dependency, because initialisers need
// ordering). A node referencing its own name is not an edge.
pub fn circles(
    classes: &[ClassModel],
    enums: &[EnumModel],
    typedefs: &[TypedefModel],
) -> Vec<Diagnostic> {
    let mut diagnostics = class_circles(classes);
    diagnostics.extend(enum_circles(enums));
    diagnostics.extend(typedef_circles(typedefs));
    diagnostics
}

fn class_circles(classes: &[ClassModel]) -> Vec<Diagnostic> {
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for class in classes {
        let mut out = Vec::new();
        if let Some(base) = class.base_name()
            && base != class.name
        {
            out.push(base);
        }
        for member in &class.members {
            if !(member.has_modifier("static") || member.has_modifier("const")) {
                continue;
            }
            if let Some(name) = initialiser_class(member)
                && name != class.name
            {
                out.push(name);
            }
        }
        edges.insert(class.name.as_str(), out);
    }
    let mut found: Vec<&str> = cycles(&edges);
    found.sort_by_key(|name| {
        classes
            .iter()
            .position(|class| class.name == *name)
            .unwrap_or_default()
    });
    found
        .into_iter()
        .filter_map(|name| {
            let class = classes.iter().find(|class| class.name == name && !class.replaced)?;
            Some(diagnostic(
                &class.file,
                class.line,
                class.module,
                format!("Circle inheritance in class '{name}'"),
            ))
        })
        .collect()
}

fn enum_circles(enums: &[EnumModel]) -> Vec<Diagnostic> {
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for enumeration in enums.iter().filter(|enumeration| !enumeration.replaced) {
        let mut out = Vec::new();
        if let Some(base) = enumeration.base_name()
            && base != enumeration.name
        {
            out.push(base);
        }
        edges.insert(enumeration.name.as_str(), out);
    }
    cycles(&edges)
        .into_iter()
        .filter_map(|name| {
            let enumeration = enums
                .iter()
                .find(|candidate| candidate.name == name && !candidate.replaced)?;
            Some(diagnostic(
                &enumeration.file,
                enumeration.line,
                enumeration.module,
                format!("Circle inheritance in enum '{name}'"),
            ))
        })
        .collect()
}

fn typedef_circles(typedefs: &[TypedefModel]) -> Vec<Diagnostic> {
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for typedef in typedefs {
        let mut out = Vec::new();
        if let Some(target) = typedef.target_name()
            && target != typedef.name
        {
            out.push(target);
        }
        // A name among the target's template arguments is an edge as well,
        // the typedef's own included.
        if let Some((_, arguments)) = typedef.target.as_deref().and_then(|target| target.split_once('<')) {
            let words = arguments.split(|c: char| !c.is_alphanumeric() && c != '_');
            out.extend(words.filter(|word| !word.is_empty()));
        }
        edges.insert(typedef.name.as_str(), out);
    }
    cycles(&edges)
        .into_iter()
        .filter_map(|name| {
            let typedef = typedefs.iter().find(|candidate| candidate.name == name)?;
            Some(diagnostic(
                &typedef.file,
                typedef.line,
                typedef.module,
                format!("Circle inheritance in typedef '{name}'"),
            ))
        })
        .collect()
}

// `Other.Member` as the initialiser, reduced to `Other`.
fn initialiser_class(member: &super::model::VarModel) -> Option<&str> {
    let Some(crate::parser::Expr {
        kind: crate::parser::ExprKind::Member { object, .. },
        ..
    }) = &member.init
    else {
        return None;
    };
    object.leaf_text()
}

// The game reports each class it revisits while that class is still on the
// stack, once. A colouring walk finds the nodes on a cycle in one pass each, so a
// diamond of dependencies costs no more than its edges.
fn cycles<'a>(edges: &HashMap<&'a str, Vec<&'a str>>) -> Vec<&'a str> {
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        Open,
        Done,
    }
    let mut colour: HashMap<&str, Colour> = HashMap::new();
    let mut found = Vec::new();
    let mut names: Vec<&str> = edges.keys().copied().collect();
    names.sort();
    for name in names {
        if colour.contains_key(name) {
            continue;
        }
        // An explicit stack, because a deep chain of bases must not recurse.
        let mut stack: Vec<(&str, usize)> = vec![(name, 0)];
        colour.insert(name, Colour::Open);
        while let Some((node, at)) = stack.pop() {
            let targets = edges.get(node).map(Vec::as_slice).unwrap_or_default();
            match targets.get(at) {
                Some(target) => {
                    stack.push((node, at + 1));
                    if !edges.contains_key(target) {
                        continue;
                    }
                    match colour.get(target) {
                        Some(Colour::Open) => {
                            if !found.contains(target) {
                                found.push(target);
                            }
                        }
                        Some(Colour::Done) => {}
                        None => {
                            colour.insert(target, Colour::Open);
                            stack.push((target, 0));
                        }
                    }
                }
                None => {
                    colour.insert(node, Colour::Done);
                }
            }
        }
    }
    found
}

fn diagnostic(file: &str, line: u32, module: Module, message: String) -> Diagnostic {
    Diagnostic {
        file: file.to_string(),
        line,
        module,
        severity: Severity::Error,
        message,
        fixme: false,
        game_crash: false,
        reports_only: false,
        related: None,
    }
}
