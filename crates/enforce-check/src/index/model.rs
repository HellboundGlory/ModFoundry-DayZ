use std::collections::HashMap;
use std::rc::Rc;

use crate::input::Module;
use crate::input::SourceFile;
use crate::parser::{ClassDecl, EnumDecl, Expr, ExprKind, FunctionDecl, Item, Stmt, TypeRef, TypedefDecl, VarDecl};

use super::resolve::base_name;

// A static array dimension, reduced to what the size rules read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dim {
    Number(i64),
    Hex,
    Name(String),
    Expression,
}

impl Dim {
    pub(crate) fn of(expr: &Expr) -> Dim {
        match &expr.kind {
            ExprKind::Leaf { text } => {
                if text.starts_with("0x") || text.starts_with("0X") {
                    Dim::Hex
                } else if let Ok(value) = text.parse::<i64>() {
                    Dim::Number(value)
                } else {
                    Dim::Name(text.clone())
                }
            }
            _ => Dim::Expression,
        }
    }
}

// A declaration the parser could not name, such as a stray `[Attr];` inside a
// class body, carries punctuation instead of an identifier.
pub fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Clone, Debug)]
pub struct VarModel {
    pub name: String,
    pub line: u32,
    pub modifiers: Vec<String>,
    pub type_name: Option<String>,
    pub dims: Vec<Dim>,
    pub has_init: bool,
    pub init: Option<Expr>,
}

impl VarModel {
    pub fn of(decl: &VarDecl) -> VarModel {
        VarModel {
            name: decl.name.clone(),
            line: decl.line,
            modifiers: decl.modifiers.clone(),
            type_name: decl.type_.as_ref().map(TypeRef::text),
            dims: decl.dims.iter().map(Dim::of).collect(),
            has_init: decl.init.is_some(),
            init: decl.init.clone(),
        }
    }

    pub fn has_modifier(&self, name: &str) -> bool {
        self.modifiers.iter().any(|modifier| modifier == name)
    }

    pub fn has_any_modifier(&self) -> bool {
        !self.modifiers.is_empty()
    }

    pub fn base_type(&self) -> Option<&str> {
        self.type_name.as_deref().map(base_name)
    }

    // An instance member: neither `static` nor `const`.
    pub fn is_instance(&self) -> bool {
        !self.has_modifier("static") && !self.has_modifier("const")
    }
}

#[derive(Clone, Debug)]
pub struct FnModel {
    pub name: String,
    pub line: u32,
    pub modifiers: Vec<String>,
    pub return_type: Option<String>,
    pub params: Vec<VarModel>,
    pub body: bool,
    pub destructor: bool,
}

impl FnModel {
    pub fn of(decl: &FunctionDecl) -> FnModel {
        FnModel {
            name: decl.name.clone(),
            line: decl.line,
            modifiers: decl.modifiers.clone(),
            return_type: decl.return_type.as_ref().map(TypeRef::text),
            params: decl.params.iter().map(VarModel::of).collect(),
            body: decl.body.is_some(),
            destructor: decl.destructor,
        }
    }

    pub fn has_modifier(&self, name: &str) -> bool {
        self.modifiers.iter().any(|modifier| modifier == name)
    }

    pub fn is_proto(&self) -> bool {
        self.has_modifier("proto")
    }
}

#[derive(Clone, Debug)]
pub struct ClassModel {
    pub name: String,
    pub file: String,
    // The place of the file among its module's files, from 0.
    pub position: usize,
    pub line: u32,
    // The line of the closing brace.
    pub end_line: u32,
    pub module: Module,
    pub sealed: bool,
    pub modded: bool,
    pub forward: bool,
    pub base: Option<String>,
    pub template: Vec<String>,
    pub members: Vec<VarModel>,
    pub functions: Vec<FnModel>,
    // Set on an original once a `modded` wrapper has taken its plain name.
    pub replaced: bool,
    // Set on a `modded` class once the merge wrapped its target, with whether
    // that target was an engine class.
    pub wrapped: Option<(String, bool)>,
    // The members a `modded` redeclaration replaced, with the type it wrote.
    pub overrides: Vec<Override>,
}

// The members a `modded` redeclaration replaced: the type it wrote, the type it
// replaced, and the place the replaced member was declared, which is where the
// game reports a mismatch.
#[derive(Clone, Debug)]
pub struct Override {
    pub name: String,
    pub type_name: Option<String>,
    pub original_type: Option<String>,
    pub file: String,
    pub line: u32,
    pub module: Module,
}

impl ClassModel {
    // An engine class declares `proto native` in its own body: the record exists
    // before any script file, so a `modded` class cannot wrap it.
    pub fn is_engine(&self) -> bool {
        self.functions
            .iter()
            .any(|function| function.is_proto() && function.has_modifier("native"))
    }

    pub fn base_name(&self) -> Option<&str> {
        self.base.as_deref().map(base_name)
    }

    pub fn member_names(&self) -> Vec<String> {
        self.members.iter().map(|member| member.name.clone()).collect()
    }
}

#[derive(Clone, Debug)]
pub struct EnumMember {
    pub name: String,
    pub line: u32,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct EnumModel {
    pub name: String,
    pub file: String,
    pub position: usize,
    pub line: u32,
    pub module: Module,
    pub sealed: bool,
    pub modded: bool,
    pub base: Option<String>,
    pub members: Vec<EnumMember>,
    pub replaced: bool,
    // Set on a `modded` enum once the merge wrapped its target, with the plain
    // name of that target.
    pub wrapped: Option<String>,
}

impl EnumModel {
    pub fn base_name(&self) -> Option<&str> {
        self.base.as_deref().map(base_name)
    }
}

#[derive(Clone, Debug)]
pub struct TypedefModel {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub module: Module,
    pub target: Option<String>,
}

impl TypedefModel {
    pub fn target_name(&self) -> Option<&str> {
        self.target.as_deref().map(base_name)
    }
}

// A file-level variable: the declaration's own record has no file of its own.
#[derive(Clone, Debug)]
pub struct Global {
    pub var: VarModel,
    pub file: String,
    pub module: Module,
}

// A file-level function, which has no owning class to carry its file.
#[derive(Clone, Debug)]
pub struct GlobalFn {
    pub function: FnModel,
    pub file: String,
    pub module: Module,
}

// The class holding this plain name now: a `modded` class holds it only once it
// has wrapped what it replaced, and a forward declaration never does.
pub fn holder<'a>(classes: &'a [ClassModel], name: &str) -> Option<&'a ClassModel> {
    classes.iter().find(|candidate| candidate.name == name && holds(candidate))
}

fn holds(candidate: &ClassModel) -> bool {
    !candidate.forward && (!candidate.modded || candidate.wrapped.is_some())
}

// Where `holder` finds each name, for a class list that has stopped changing:
// the rules look a class up once per type they read.
pub fn holders(classes: &[ClassModel]) -> HashMap<String, usize> {
    let mut places = HashMap::new();
    for (at, class) in classes.iter().enumerate() {
        if holds(class) {
            places.entry(class.name.clone()).or_insert(at);
        }
    }
    places
}

// What a class leaves behind for the modules after it: the functions an override
// or constructor is matched against, its template parameters and its base.
// Shared because every module after this one reads it and none of them writes
// it: a module's own records stay put while the later modules are indexed.
#[derive(Clone, Debug, Default)]
pub struct ClassRecord {
    pub functions: Rc<Vec<FnModel>>,
    pub template: Rc<Vec<String>>,
    pub base: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FileModel {
    pub classes: Vec<ClassModel>,
    pub enums: Vec<EnumModel>,
    pub typedefs: Vec<TypedefModel>,
    pub globals: Vec<Global>,
    pub functions: Vec<GlobalFn>,
}

impl FileModel {
    pub fn of(position: usize, file: &SourceFile, ast: &crate::parser::FileAst) -> FileModel {
        let mut model = FileModel::default();
        let module = Module::from(file.layer);
        for item in &ast.items {
            match item {
                Item::Class(decl) => model.classes.push(class(decl, position, file, module)),
                Item::Enum(decl) => model.enums.push(enumeration(decl, position, file, module)),
                Item::Typedef(decl) => model.typedefs.push(typedef(decl, file, module)),
                Item::Var(decl) if is_name(&decl.name) => model.globals.push(Global {
                    var: VarModel::of(decl),
                    file: file.path.clone(),
                    module,
                }),
                Item::Function(decl) if is_name(&decl.name) => model.functions.push(GlobalFn {
                    function: FnModel::of(decl),
                    file: file.path.clone(),
                    module,
                }),
                _ => {}
            }
        }
        model
    }
}

fn class(decl: &ClassDecl, position: usize, file: &SourceFile, module: Module) -> ClassModel {
    let mut members = Vec::new();
    let mut functions = Vec::new();
    for item in &decl.members {
        match item {
            Item::Var(var) if is_name(&var.name) => members.push(VarModel::of(var)),
            Item::Function(function) if is_name(&function.name) => functions.push(FnModel::of(function)),
            _ => {}
        }
    }
    ClassModel {
        name: decl.name.clone(),
        file: file.path.clone(),
        position,
        line: decl.line,
        end_line: decl.end_line,
        module,
        sealed: decl.modifiers.iter().any(|modifier| modifier == "sealed"),
        modded: decl.modifiers.iter().any(|modifier| modifier == "modded"),
        forward: decl.forward,
        base: decl.base.as_ref().map(TypeRef::text),
        template: decl.template.iter().map(|param| param.name.clone()).collect(),
        members,
        functions,
        replaced: false,
        wrapped: None,
        overrides: Vec::new(),
    }
}

fn enumeration(decl: &EnumDecl, position: usize, file: &SourceFile, module: Module) -> EnumModel {
    EnumModel {
        name: decl.name.clone(),
        file: file.path.clone(),
        position,
        line: decl.line,
        module,
        sealed: decl.modifiers.iter().any(|modifier| modifier == "sealed"),
        modded: decl.modifiers.iter().any(|modifier| modifier == "modded"),
        base: decl.base.as_ref().map(TypeRef::text),
        members: decl
            .members
            .iter()
            .filter_map(member)
            .filter(|member| is_name(&member.name))
            .collect(),
        replaced: false,
        wrapped: None,
    }
}

fn member(item: &Item) -> Option<EnumMember> {
    let Item::Stmt(Stmt::Expr { line, expr }) = item else {
        return None;
    };
    match &expr.kind {
        ExprKind::Binary { op, left, right } if op == "=" => Some(EnumMember {
            name: left.leaf_text().unwrap_or_default().to_string(),
            line: *line,
            value: right.as_deref().cloned(),
        }),
        _ => Some(EnumMember {
            name: expr.leaf_text().unwrap_or_default().to_string(),
            line: *line,
            value: None,
        }),
    }
}

fn typedef(decl: &TypedefDecl, file: &SourceFile, module: Module) -> TypedefModel {
    TypedefModel {
        name: decl.name.clone(),
        file: file.path.clone(),
        line: decl.line,
        module,
        target: decl.type_.as_ref().map(TypeRef::text),
    }
}
