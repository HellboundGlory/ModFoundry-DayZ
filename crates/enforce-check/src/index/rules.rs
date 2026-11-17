use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity};
use crate::input::Module;
use crate::parser::{Expr, ExprKind};

use super::model::{
    ClassModel, EnumMember, EnumModel, FnModel, Global, GlobalFn, Override,
    TypedefModel, VarModel,
};
use super::dims::dimension_lines;
use super::resolve::{base_name, Known, Kind, Scope};

// One line a rule shared with the body pass prints: the text, and whether the
// retail targets only warn about it.
pub struct Line {
    pub message: String,
    pub soft: bool,
}

// `Soft` prints `FIX-ME:` and only warns on the retail targets; the target
// settles the severity in `check`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Level {
    Error,
    // An error the game prints and then carries on from: the module still loads.
    ErrorKeepsLoading,
    Warning,
    Soft,
    // A soft rule that fails the module on no target.
    SoftKeepsLoading,
}

// One diagnostic at a level a later target adjustment settles.
pub(super) fn push(
    out: &mut Vec<Diagnostic>,
    file: &str,
    line: u32,
    module: Module,
    level: Level,
    message: String,
) {
    out.push(diagnostic(file, line, module, level, message, None));
}

fn diagnostic(
    file: &str,
    line: u32,
    module: Module,
    level: Level,
    message: String,
    related: Option<(String, u32)>,
) -> Diagnostic {
    Diagnostic {
        file: file.to_string(),
        line,
        module,
        severity: match level {
            Level::Error | Level::ErrorKeepsLoading => Severity::Error,
            Level::Warning | Level::Soft | Level::SoftKeepsLoading => Severity::Warning,
        },
        message,
        fixme: matches!(level, Level::Soft | Level::SoftKeepsLoading),
        game_crash: false,
        reports_only: matches!(level, Level::ErrorKeepsLoading | Level::SoftKeepsLoading),
        related,
    }
}

#[derive(Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    fn error(&mut self, file: &str, line: u32, module: Module, message: String) {
        self.push(file, line, module, Level::Error, message, None);
    }

    fn warning(&mut self, file: &str, line: u32, module: Module, message: String) {
        self.push(file, line, module, Level::Warning, message, None);
    }

    fn soft(&mut self, file: &str, line: u32, module: Module, message: String) {
        self.push(file, line, module, Level::Soft, message, None);
    }

    fn push(
        &mut self,
        file: &str,
        line: u32,
        module: Module,
        level: Level,
        message: String,
        related: Option<(String, u32)>,
    ) {
        self.diagnostics.push(diagnostic(file, line, module, level, message, related));
    }
}

// One module's declarations after the merge, ready for the rules.
pub struct Declarations {
    pub types: Scope,
    pub classes: Vec<ClassModel>,
    // The place in `classes` of the class holding each plain name.
    pub holders: HashMap<String, usize>,
    pub enums: Vec<EnumModel>,
    pub typedefs: Vec<TypedefModel>,
    pub globals: Vec<Global>,
    pub functions: Vec<GlobalFn>,
    // `const int` names an array size may read, with the value when it is literal.
    pub constants: HashMap<String, Option<i64>>,
    // What the earlier modules declared.
    pub known: Known,
    // The module's last file and its last line that holds text.
    pub end: Option<(String, u32)>,
    // Whether the target counts a soft rule as an error.
    pub strict: bool,
}

impl Declarations {
    pub fn holder(&self, name: &str) -> Option<&ClassModel> {
        self.holders.get(name).map(|at| &self.classes[*at])
    }
}

// The engine-modded check runs first and ends pass 1 by itself.
pub fn engine_rules(declarations: &Declarations) -> Report {
    let mut report = Report::default();
    for class in declarations.classes.iter().filter(|class| class.modded) {
        if let Some((name, engine)) = class.wrapped.as_ref()
            && *engine
        {
            report.error(
                &class.file,
                class.line,
                class.module,
                format!("Engine class '{name}' cannot be modded."),
            );
        }
    }
    report
}

// The globals are read before the classes are declared, and a global error skips
// the class, member and function rows of the module.
pub fn global_pass(declarations: &Declarations) -> Report {
    let mut report = Report::default();
    for var in &declarations.globals {
        global_rules(declarations, var, &mut report);
    }
    report
}

// The record rows: the typedefs and enums, then the classes with their bases and
// members, then the file-level functions.
pub fn run(declarations: &Declarations) -> Report {
    let mut report = Report::default();
    for typedef in &declarations.typedefs {
        typedef_rules(declarations, typedef, &mut report);
    }
    for enumeration in &declarations.enums {
        enum_rules(declarations, enumeration, &mut report);
        enum_member_rules(declarations, enumeration, &mut report);
    }
    for declared in &declarations.functions {
        parameter_rules(
            declarations,
            &declared.file,
            declared.module,
            &declared.function,
            &[],
            &mut report,
        );
    }
    for class in &declarations.classes {
        class_rules(declarations, class, &mut report);
    }
    for class in declarations.classes.iter().filter(|class| class.replaced) {
        override_rules(class, &mut report);
    }
    report
}

// A typedef's target must be visible, and its name must be free.
fn typedef_rules(declarations: &Declarations, typedef: &TypedefModel, report: &mut Report) {
    let Some(target) = typedef.target_name() else {
        return;
    };
    if !declarations.types.is_known(target) {
        report.error(
            &typedef.file,
            typedef.line,
            typedef.module,
            format!("Unknown type '{target}'"),
        );
    }
    if declarations.known.types.has(&typedef.name) {
        report.error(
            &typedef.file,
            typedef.line,
            typedef.module,
            format!("Typedef '{}' already exists", typedef.name),
        );
    }
}

fn enum_rules(declarations: &Declarations, enumeration: &EnumModel, report: &mut Report) {
    if let Some(base) = enumeration.base_name()
        && declarations.types.kind_of(base) != Some(Kind::Enum)
    {
        report.error(
            &enumeration.file,
            enumeration.line,
            enumeration.module,
            format!("Can't inherit class '{base}'"),
        );
    }
    if enumeration.modded {
        return;
    }
    // An enum name from an earlier module prints the class row when that record
    // was an enum, and the soft non-enum row otherwise.
    if let Some(symbol) = declarations.known.types.find(&enumeration.name) {
        match symbol.kind {
            Kind::Enum => report.error(
                &enumeration.file,
                enumeration.line,
                enumeration.module,
                format!("Multiple declaration of class '{}'", enumeration.name),
            ),
            _ => report.soft(
                &enumeration.file,
                enumeration.line,
                enumeration.module,
                format!("Multiple declaration of '{}'", enumeration.name),
            ),
        }
    }
}

// A modded enum's target kept its members under the renamed original, which
// became this enum's base.
fn enum_member_rules(declarations: &Declarations, enumeration: &EnumModel, report: &mut Report) {
    let inherited = inherited_enum_members(declarations, enumeration);
    let mut seen: Vec<&str> = Vec::new();
    for member in &enumeration.members {
        member_value(member, enumeration, report);
        let repeated = seen.contains(&member.name.as_str())
            || inherited.iter().any(|name| *name == member.name);
        if repeated {
            report.error(
                &enumeration.file,
                member.line,
                enumeration.module,
                format!("Multiple declaration of variable '{}'", member.name),
            );
        }
        seen.push(&member.name);
    }
}

fn inherited_enum_members<'a>(declarations: &'a Declarations, enumeration: &'a EnumModel) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut seen = vec![enumeration.name.clone()];
    let mut base = enumeration.base_name().map(str::to_string);
    while let Some(name) = base {
        if seen.contains(&name) {
            break;
        }
        seen.push(name.clone());
        let Some(parent) = declarations.enums.iter().find(|candidate| candidate.name == name) else {
            break;
        };
        out.extend(parent.members.iter().map(|member| member.name.as_str()));
        base = parent.base_name().map(str::to_string);
    }
    out
}

// A single-token right side `strtol` cannot read. Hex tokens are read with radix
// 16 and accepted; a bare identifier is not, and expressions are skipped.
// The game names the member on the left of the assignment.
fn member_value(member: &EnumMember, enumeration: &EnumModel, report: &mut Report) {
    let Some(Expr {
        kind: ExprKind::Leaf { text },
        ..
    }) = &member.value
    else {
        return;
    };
    let readable = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).is_ok(),
        None => text.parse::<i64>().is_ok(),
    };
    if !readable {
        report.error(
            &enumeration.file,
            member.line,
            enumeration.module,
            format!("Can't convert enum value '{text}' to number"),
        );
    }
}
// A plain class's base, then every class's members. A modded class's members
// were left by the merge with the redeclarations removed, so what is left is new.
fn class_rules(declarations: &Declarations, class: &ClassModel, report: &mut Report) {
    if class.forward {
        return;
    }
    if !class.modded {
        plain_class_rules(declarations, class, report);
    }
    member_rows(declarations, class, report);
}

fn member_rows(declarations: &Declarations, class: &ClassModel, report: &mut Report) {
    for (at, member) in class.members.iter().enumerate() {
        member_rules(declarations, class, &class.members[..at], member, report);
    }
    for function in &class.functions {
        parameter_rules(
            declarations,
            &class.file,
            class.module,
            function,
            &class.template,
            report,
        );
    }
}

// Parameter rows. The position is the function's declaration line, because the
// game sets the position once per member and the parameter code does not reset
// it.
fn parameter_rules(
    declarations: &Declarations,
    file: &str,
    module: Module,
    function: &FnModel,
    templates: &[String],
    report: &mut Report,
) {
    for parameter in &function.params {
        let line = function.line;
        if declares_a_type(declarations, &parameter.name) {
            report.error(
                file,
                line,
                module,
                format!("Variable name '{}' already used as type name", parameter.name),
            );
        }
        let Some(type_name) = parameter.base_type() else {
            continue;
        };
        if templates.iter().any(|name| name == type_name) {
            continue;
        }
        // `ref`/`autoptr` on a script function's parameter is a soft error; a
        // `proto` function's parameters are the engine's own signatures.
        if (parameter.has_modifier("ref") || parameter.has_modifier("autoptr"))
            && !function.is_proto()
        {
            report.soft(
                file,
                line,
                module,
                "Method argument can't be strong reference".to_string(),
            );
        }
        if declarations.types.is_known(type_name) {
            if parameter.has_modifier("notnull")
                && declarations.types.is_primitive(type_name)
                && type_name != "string"
            {
                report.error(
                    file,
                    line,
                    module,
                    format!("'notnull' must not be used with '{type_name}' type"),
                );
                report.error(file, line, module, "Error in parameters".to_string());
            }
            continue;
        }
        // The unknown-type cascade adds a third line for a parameter.
        if parameter.has_any_modifier() {
            report.error(file, line, module, format!("Bad type '{type_name}'"));
        }
        report.error(file, line, module, format!("Unknown type '{type_name}'"));
        report.error(file, line, module, "Error in parameters".to_string());
    }
}

// A file-level variable whose name is a type the module can see.
fn global_rules(declarations: &Declarations, global: &Global, report: &mut Report) {
    let var = &global.var;
    if declares_a_type(declarations, &var.name) {
        report.error(
            &global.file,
            var.line,
            global.module,
            format!("Variable name '{}' already used as type name", var.name),
        );
    }
    let Some(type_name) = var.base_type() else {
        return;
    };
    if !declarations.types.is_known(type_name) {
        if var.has_any_modifier() {
            report.error(&global.file, var.line, global.module, format!("Bad type '{type_name}'"));
        }
        report.error(
            &global.file,
            var.line,
            global.module,
            format!("Unknown type '{type_name}'"),
        );
    }
}

fn plain_class_rules(declarations: &Declarations, class: &ClassModel, report: &mut Report) {
    // An enum of this name in this module is a completed class-kind record, so
    // the class is the duplicate; a plain alias is not.
    let enum_first = declarations
        .enums
        .iter()
        .any(|enumeration| !enumeration.replaced && enumeration.name == class.name);
    let alias = declarations
        .typedefs
        .iter()
        .any(|typedef| typedef.name == class.name);
    if enum_first && !alias {
        report.error(
            &class.file,
            class.line,
            class.module,
            format!("Multiple declaration of class '{}'", class.name),
        );
        return;
    }
    // A class the modules before this one completed is the pass-1 row, one line.
    // A forward declaration there holds only the name, so it is no duplicate.
    if declarations
        .known
        .types
        .find(&class.name)
        .is_some_and(|symbol| !symbol.forward)
    {
        report.error(
            &class.file,
            class.line,
            class.module,
            format!("Multiple declaration of class '{}'", class.name),
        );
    }
    let Some(base) = class.base_name() else {
        return;
    };
    match declarations.types.kind_of(base) {
        // A class body on a built-in's name extends its pseudo-class, which the
        // engine writes as `class ResourceName : string`.
        Some(Kind::Class | Kind::Primitive) => {
            if declarations.types.find(base).is_some_and(|symbol| symbol.sealed) {
                report.error(
                    &class.file,
                    class.line,
                    class.module,
                    format!("'{}': cannot derive from sealed type '{base}'", class.name),
                );
            }
        }
        Some(_) => report.error(
            &class.file,
            class.line,
            class.module,
            format!("Can't inherit class '{}'", class.name),
        ),
        None => report.error(
            &class.file,
            class.line,
            class.module,
            format!("Unknown type '{base}'"),
        ),
    }
}

fn member_rules(
    declarations: &Declarations,
    class: &ClassModel,
    earlier: &[VarModel],
    member: &VarModel,
    report: &mut Report,
) {
    let file = class.file.as_str();
    let line = member.line;
    let module = class.module;
    if member.has_modifier("private") && member.has_modifier("protected") {
        report.error(file, line, module, "Private conflicts with protected".to_string());
    }
    if member.has_modifier("local") {
        report.error(
            file,
            line,
            module,
            format!("Local variable '{}' outside function", member.name),
        );
    }
    if ["out", "inout", "notnull"].iter().any(|name| member.has_modifier(name)) {
        report.error(
            file,
            line,
            module,
            "inout/out/notnull can be used only on function parameters".to_string(),
        );
    }
    // A body on a non-class type's name is an extension body, and only a static
    // or const member may be declared there.
    if member.is_instance() && declarations.types.kind_of(&class.name) != Some(Kind::Class) {
        report.error(
            file,
            line,
            module,
            format!(
                "Variable '{}' in pseudo-class '{}' must be static/const",
                member.name, class.name
            ),
        );
    }
    if member.base_type() == Some("auto") && !member.is_instance() {
        report.error(file, line, module, "static/const variables can't be auto".to_string());
    }
    if declares_a_type(declarations, &member.name) {
        report.error(
            file,
            line,
            module,
            format!("Variable name '{}' already used as type name", member.name),
        );
    }
    if redeclares(declarations, class, earlier, member) {
        // The warning belongs to an existing plain instance member only; the
        // duplicate itself is any existing name, whatever this declaration's
        // modifiers are.
        if conflicts_with_instance(declarations, class, earlier, &member.name) {
            report.warning(file, line, module, format!("Possible variable name conflict '{}'", member.name));
        }
        report.error(
            file,
            line,
            module,
            format!("Multiple declaration of variable '{}'", member.name),
        );
        report.error(file, line, module, format!("Can't create variable '{}'", member.name));
    }
    let Some(type_name) = member.base_type() else {
        return;
    };
    if class.template.iter().any(|name| name == type_name) {
        return;
    }
    type_rules(declarations, class, member, type_name, report);
    dimension_rules(declarations, class, member, type_name, report);
}

// Unknown-type cascade: a declaration with no modifier prints the plain message,
// one with any modifier prints `Bad type` first.
fn type_rules(
    declarations: &Declarations,
    class: &ClassModel,
    member: &VarModel,
    type_name: &str,
    report: &mut Report,
) {
    let file = class.file.as_str();
    let line = member.line;
    let module = class.module;
    if declarations.types.is_known(type_name) {
        // A class record that is only forward-declared is a soft unknown.
        if declarations.types.find(type_name).is_some_and(|symbol| symbol.forward) {
            report.soft(
                file,
                line,
                module,
                format!("Unknown type '{type_name}'"),
            );
            return;
        }
        let auto = member.has_modifier("ref") || member.has_modifier("autoptr");
        if auto && declarations.types.is_primitive(type_name) {
            report.error(
                file,
                line,
                module,
                format!("Auto-pointer '{type_name}' must be class-type"),
            );
        }
        if member.has_modifier("notnull")
            && declarations.types.is_primitive(type_name)
            && type_name != "string"
        {
            report.error(
                file,
                line,
                module,
                format!("'notnull' must not be used with '{type_name}' type"),
            );
        }
        return;
    }
    if member.has_any_modifier() {
        report.error(file, line, module, format!("Bad type '{type_name}'"));
    }
    report.error(file, line, module, format!("Unknown type '{type_name}'"));
}

fn dimension_rules(
    declarations: &Declarations,
    class: &ClassModel,
    member: &VarModel,
    type_name: &str,
    report: &mut Report,
) {
    let lines = dimension_lines(&declarations.constants, &declarations.types, &member.name, &member.dims, type_name);
    for line in lines {
        match line.soft {
            true => report.soft(&class.file, member.line, class.module, line.message),
            false => report.error(&class.file, member.line, class.module, line.message),
        }
    }
}

// A variable, parameter or global whose name is a type the module can see.
fn declares_a_type(declarations: &Declarations, name: &str) -> bool {
    declarations.types.blocks_variable_name(name)
}

// The existing member of this name: `Some(true)` when it is a plain instance
// member, `Some(false)` when it is not, `None` when nothing has it. A duplicate
// declaration never creates a record, so only the first declaration of a name in
// the class survives; further up, any ancestor's instance member counts.
fn inherited_member(
    declarations: &Declarations,
    class: &ClassModel,
    earlier: &[VarModel],
    name: &str,
) -> Option<bool> {
    if let Some(member) = earlier.iter().find(|other| other.name == name) {
        return Some(member.is_instance());
    }
    let mut seen = vec![class.name.clone()];
    let mut base = class.base_name().map(str::to_string);
    while let Some(current) = base {
        if seen.contains(&current) {
            break;
        }
        seen.push(current.clone());
        match declarations.holder(&current) {
            Some(parent) => {
                if let Some(member) = parent.members.iter().find(|other| other.name == name) {
                    return Some(member.is_instance());
                }
                base = parent.base_name().map(str::to_string);
            }
            None => {
                // `Known` keeps names only, so an earlier-layer member counts as
                // an instance member.
                if declarations.known.inherited(&current).iter().any(|other| other == name) {
                    return Some(true);
                }
                base = declarations
                    .known
                    .bases
                    .get(&current)
                    .map(|next| base_name(next).to_string());
            }
        }
    }
    None
}

// A member name the class or a base class already has. A modded class's
// own redeclarations were removed by the merge, so a name it still declares is
// checked here like any subclass's, including against an earlier layer's base.
fn redeclares(
    declarations: &Declarations,
    class: &ClassModel,
    earlier: &[VarModel],
    member: &VarModel,
) -> bool {
    inherited_member(declarations, class, earlier, &member.name).is_some()
}

// The warning is limited to an existing plain instance member.
fn conflicts_with_instance(
    declarations: &Declarations,
    class: &ClassModel,
    earlier: &[VarModel],
    name: &str,
) -> bool {
    inherited_member(declarations, class, earlier, name) == Some(true)
}

// A modded redeclaration may change only the value. The transplanted initialiser
// is checked against the type of the member it replaced, and the error is
// reported where that member was declared.
fn override_rules(class: &ClassModel, report: &mut Report) {
    for Override { name, type_name, original_type, file, line, module } in &class.overrides {
        let original = original_type.as_deref().unwrap_or_default();
        let different = type_name.as_deref().is_some_and(|written| written != original);
        if different {
            report.error(file, *line, *module, format!("Incompatible parameter '{name}'"));
        }
    }
}
