mod circles;
mod dims;
mod engine;
mod functions;
mod merge;
mod model;
mod resolve;
mod rules;
mod templates;

use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity};
use crate::input::SourceFile;

pub use model::{ClassModel, EnumModel, FnModel, Global, GlobalFn, TypedefModel, VarModel};
pub use resolve::{Known, Kind, Scope};
pub use templates::{lines as template_lines, Place as TemplatePlace};
pub(crate) use dims::dimension_lines;
pub(crate) use model::{is_name, Dim};
pub(crate) use resolve::{base_name, is_primitive, split_arguments};

use merge::Merged;
use model::FileModel;
use resolve::Symbol;

// One module's declarations, in the shape the rules and later work read.
pub struct Index {
    pub classes: Vec<ClassModel>,
    pub enums: Vec<EnumModel>,
    pub typedefs: Vec<TypedefModel>,
    pub globals: Vec<Global>,
    // Each `modded` declaration and the declaration it wrapped, in load order.
    pub modded_chains: Vec<Chain>,
    // The functions declared at module level.
    pub functions: Vec<GlobalFn>,
    pub types: Scope,
    // `const int` names an array size may read, with the value when it is literal.
    pub constants: HashMap<String, Option<i64>>,
    pub diagnostics: Vec<Diagnostic>,
    // What the modules compiled after this one may see.
    pub known: Known,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    pub name: String,
    pub file: String,
    pub line: u32,
    // The plain name the wrapped declaration had before the merge renamed it.
    pub wrapped: Option<String>,
}

// Parse every file, merge `modded`, then declare typedefs, enums, classes, their
// members and the globals.
// A file's path and its last line that holds text; an empty file has line 1.
fn module_end(file: &SourceFile) -> (String, u32) {
    let lines = file.text.trim_end().lines().count() as u32;
    (file.path.clone(), lines.max(1))
}

// `strict`: the target counts a soft rule as an error.
pub fn build(files: &[(&SourceFile, &crate::parser::FileAst)], earlier: &Known, strict: bool) -> Index {
    let mut models = Vec::with_capacity(files.len());
    for (position, (file, ast)) in files.iter().enumerate() {
        models.push(FileModel::of(position, file, ast));
    }
    let mut classes = Vec::new();
    let mut enums = Vec::new();
    let mut typedefs = Vec::new();
    let mut globals = Vec::new();
    let mut file_functions = Vec::new();
    for model in &models {
        classes.extend(model.classes.iter().cloned());
        enums.extend(model.enums.iter().cloned());
        typedefs.extend(model.typedefs.iter().cloned());
        globals.extend(model.globals.iter().cloned());
        file_functions.extend(model.functions.iter().cloned());
    }
    let merged = Merged::of(classes, enums);
    let chains = chains(&merged.classes, &merged.enums);
    let mut constants = constants(&globals, &merged.classes);
    constants.extend(earlier.constants.clone());
    let types = types(&merged, &typedefs, earlier);
    let declarations = rules::Declarations {
        types,
        holders: model::holders(&merged.classes),
        classes: merged.classes,
        enums: merged.enums,
        typedefs,
        globals,
        functions: file_functions,
        constants: constants.clone(),
        known: earlier.clone(),
        end: files.last().map(|(file, _)| module_end(file)),
        strict,
    };
    let mut diagnostics = merged.diagnostics;
    // An error from the merge is a parse error, so pass 1 never runs.
    if diagnostics.is_empty() {
        diagnostics.extend(pass_one(&declarations));
    }
    let known = Known::of(
        &declarations.classes,
        &declarations.holders,
        &declarations.functions,
        &declarations.types,
        &declarations.constants,
        earlier,
    );
    Index {
        classes: declarations.classes,
        enums: declarations.enums,
        typedefs: declarations.typedefs,
        globals: declarations.globals,
        modded_chains: chains,
        functions: declarations.functions,
        types: declarations.types,
        constants,
        diagnostics,
        known,
    }
}

// The engine-modded check ends the module on its own, then the circles, then the
// global variables, whose failure skips the class rows, then the records.
fn pass_one(declarations: &rules::Declarations) -> Vec<Diagnostic> {
    let engine = rules::engine_rules(declarations);
    if !engine.diagnostics.is_empty() {
        return engine.diagnostics;
    }
    let loops = circles::circles(
        &declarations.classes,
        &declarations.enums,
        &declarations.typedefs,
    );
    if !loops.is_empty() {
        return loops;
    }
    let mut globals = rules::global_pass(declarations);
    if globals.diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Error) {
        return globals.diagnostics;
    }
    globals.diagnostics.extend(rules::run(declarations).diagnostics);
    globals.diagnostics.extend(functions::rules(declarations));
    globals.diagnostics.extend(templates::rules(declarations));
    // The `proto` body check belongs to the function-body pass, which runs only
    // when the module's declarations reported no hard error. A destructor error
    // is reported and the module carries on, so it does not close the pass.
    if !globals
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error && !diagnostic.reports_only)
    {
        globals.diagnostics.extend(engine::rules(declarations));
    }
    globals.diagnostics
}

// Typedefs and enums are recorded before any class. A class body on a plain
// alias's name takes it over, but an enum record keeps its name, so a class of
// that name is a duplicate.
fn types(merged: &Merged, typedefs: &[TypedefModel], earlier: &Known) -> Scope {
    let mut types = earlier.types.clone();
    for typedef in typedefs {
        types.insert(
            &typedef.name,
            Symbol::alias(typedef.target.as_deref().unwrap_or_default()),
        );
    }
    for enumeration in &merged.enums {
        types.insert(&enumeration.name, Symbol::enumeration(enumeration.sealed));
    }
    for class in &merged.classes {
        types.insert_class(
            &class.name,
            Symbol::class(class.sealed, class.forward, class.is_engine()),
        );
    }
    types
}

// A `const int` name is what an array size accepts. The value is kept when it is
// a literal, for the size rule; a name whose value is not a literal is still
// accepted, which is how the game resolves constants.
fn constants(globals: &[Global], classes: &[ClassModel]) -> HashMap<String, Option<i64>> {
    let mut constants = HashMap::new();
    let members = classes.iter().flat_map(|class| class.members.iter());
    for var in globals.iter().map(|global| &global.var).chain(members) {
        if !var.has_modifier("const") || var.base_type() != Some("int") {
            continue;
        }
        let value = match &var.init {
            Some(crate::parser::Expr {
                kind: crate::parser::ExprKind::Leaf { text },
                ..
            }) => text.parse::<i64>().ok(),
            _ => None,
        };
        constants.insert(var.name.clone(), value);
    }
    constants
}

// After the merge, each `modded` declaration names the declaration it wrapped,
// by the plain name that declaration had before it was renamed.
fn chains(classes: &[ClassModel], enums: &[EnumModel]) -> Vec<Chain> {
    let class_chains = classes
        .iter()
        .filter(|class| class.modded)
        .map(|class| Chain {
            name: class.wrapped.as_ref().map_or_else(|| class.name.clone(), |(name, _)| name.clone()),
            file: class.file.clone(),
            line: class.line,
            wrapped: class.wrapped.as_ref().map(|(name, _)| name.clone()),
        });
    let enum_chains = enums
        .iter()
        .filter(|enumeration| enumeration.modded)
        .map(|enumeration| Chain {
            name: enumeration.name.clone(),
            file: enumeration.file.clone(),
            line: enumeration.line,
            wrapped: enumeration.wrapped.clone(),
        });
    class_chains.chain(enum_chains).collect()
}
