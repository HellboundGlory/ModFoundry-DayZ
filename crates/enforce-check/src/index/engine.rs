use crate::diag::Diagnostic;
use crate::input::Module;

use super::model::FnModel;
use super::resolve::base_name;
use super::rules::{push, Declarations, Level};

// The rules a `proto` declaration goes through. The engine's own function table
// is not modelled: a mod's `proto` names have no engine record, and the link
// check that would report them did not fire on the server.
pub(super) fn rules(declarations: &Declarations) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for class in declarations.classes.iter().filter(|class| !class.forward) {
        for function in class.functions.iter().filter(|function| function.is_proto()) {
            proto_rules(function, &class.file, class.module, &mut out);
        }
    }
    for declared in declarations.functions.iter().filter(|declared| declared.function.is_proto()) {
        proto_rules(&declared.function, &declared.file, declared.module, &mut out);
    }
    out
}

fn proto_rules(function: &FnModel, file: &str, module: Module, out: &mut Vec<Diagnostic>) {
    // A `proto` declaration is never given a name the body pass can look up, so
    // one written with a body fails that lookup before any flag is read.
    if function.body {
        push(
            out,
            file,
            function.line,
            module,
            Level::Error,
            "Cannot find function '(null)'".to_string(),
        );
        return;
    }
    if !function.has_modifier("native") {
        return;
    }
    let offending = function.params.iter().any(|parameter| {
        (parameter.has_modifier("out") || parameter.has_modifier("inout"))
            && parameter.dims.is_empty()
            && parameter
                .type_name
                .as_deref()
                .map(base_name)
                .is_some_and(|name| name == "int")
    });
    if offending {
        push(
            out,
            file,
            function.line,
            module,
            Level::Error,
            format!("Native functions don't support 'out' arguments '{}'", function.name),
        );
    }
}
