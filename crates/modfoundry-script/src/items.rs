//! Declaration item types extracted from one Enforce source file.

use modfoundry_core::Span;

/// A type reference: base name, generic arguments, and `ref` strength.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeRef {
    pub name: String,
    pub args: Vec<TypeRef>,
    pub strong: bool,
}

impl TypeRef {
    /// A plain named type: no generic arguments, not `ref`.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: Vec::new(),
            strong: false,
        }
    }

    /// Render back to source form, e.g. `ref array<ref Foo>`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.strong {
            s.push_str("ref ");
        }
        s.push_str(&self.name);
        if !self.args.is_empty() {
            s.push('<');
            for (i, a) in self.args.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&a.display());
            }
            s.push('>');
        }
        s
    }
}

/// A formal parameter of a method or free function.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamItem {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeRef,
    pub modifiers: Vec<String>,
    pub default: Option<String>,
    pub array: Vec<String>,
}

/// A class field or top-level global.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldItem {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeRef,
    pub modifiers: Vec<String>,
    pub doc: Option<String>,
    pub attributes: Vec<String>,
    pub value: Option<String>,
    pub array: Vec<String>,
}

/// A class method or top-level free function.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodItem {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub ret: TypeRef,
    pub params: Vec<ParamItem>,
    pub modifiers: Vec<String>,
    pub doc: Option<String>,
    pub body: Option<Span>,
    pub attributes: Vec<String>,
    pub decl_start: usize,
    pub destructor: bool,
}

impl MethodItem {
    /// Render `modifiers ret name(params)` on one line.
    pub fn signature(&self) -> String {
        let mut s = String::new();
        for m in &self.modifiers {
            s.push_str(m);
            s.push(' ');
        }
        s.push_str(&self.ret.display());
        s.push(' ');
        s.push_str(&self.name);
        s.push('(');
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            for m in &p.modifiers {
                s.push_str(m);
                s.push(' ');
            }
            s.push_str(&p.ty.display());
            s.push(' ');
            s.push_str(&p.name);
            if let Some(d) = &p.default {
                s.push_str(" = ");
                s.push_str(d);
            }
        }
        s.push(')');
        s
    }
}

/// A class declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassItem {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub modded: bool,
    pub base: Option<TypeRef>,
    pub type_params: Vec<String>,
    pub fields: Vec<FieldItem>,
    pub methods: Vec<MethodItem>,
    pub doc: Option<String>,
    pub attributes: Vec<String>,
    pub decl_start: usize,
}

/// One enumerator.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumMember {
    pub name: String,
    pub name_span: Span,
    /// The member's `= ...` expression as written, if it has one.
    pub value: Option<String>,
}

/// An enum declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub base: Option<String>,
    pub members: Vec<EnumMember>,
    pub doc: Option<String>,
}

/// A `typedef target alias;` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedefItem {
    pub name: String,
    pub name_span: Span,
    pub target: TypeRef,
    pub doc: Option<String>,
}

/// Every declaration found in one source file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileItems {
    pub classes: Vec<ClassItem>,
    pub enums: Vec<EnumItem>,
    pub typedefs: Vec<TypedefItem>,
    pub functions: Vec<MethodItem>,
    pub globals: Vec<FieldItem>,
}
