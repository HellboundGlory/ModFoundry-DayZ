#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAst {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Class(ClassDecl),
    Enum(EnumDecl),
    Typedef(TypedefDecl),
    Function(FunctionDecl),
    Var(VarDecl),
    Block(Block),
    Stmt(Stmt),
}

impl Item {
    pub fn line(&self) -> u32 {
        match self {
            Item::Class(node) => node.line,
            Item::Enum(node) => node.line,
            Item::Typedef(node) => node.line,
            Item::Function(node) => node.line,
            Item::Var(node) => node.line,
            Item::Block(node) => node.line,
            Item::Stmt(node) => node.line(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub line: u32,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeRef {
    pub line: u32,
    pub modifiers: Vec<String>,
    pub name: String,
    pub template: Vec<TypeRef>,
    pub array: bool,
}

impl TypeRef {
    pub fn text(&self) -> String {
        let mut text = self.name.clone();
        if !self.template.is_empty() {
            let inner: Vec<String> = self.template.iter().map(TypeRef::text).collect();
            text = format!("{text}<{}>", inner.join(","));
        }
        if self.array {
            text.push_str("[]");
        }
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarDecl {
    pub line: u32,
    pub attributes: Vec<Expr>,
    pub modifiers: Vec<String>,
    pub type_: Option<TypeRef>,
    pub name: String,
    pub dims: Vec<Expr>,
    pub init: Option<Expr>,
    pub ctor_args: Option<Vec<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassDecl {
    pub line: u32,
    // The line of the closing brace.
    pub end_line: u32,
    pub attributes: Vec<Expr>,
    pub modifiers: Vec<String>,
    pub name: String,
    pub template: Vec<VarDecl>,
    pub base: Option<TypeRef>,
    pub forward: bool,
    pub members: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDecl {
    pub line: u32,
    pub attributes: Vec<Expr>,
    pub modifiers: Vec<String>,
    pub name: String,
    pub base: Option<TypeRef>,
    pub members: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedefDecl {
    pub line: u32,
    pub name: String,
    pub type_: Option<TypeRef>,
    pub params: Vec<VarDecl>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDecl {
    pub line: u32,
    pub attributes: Vec<Expr>,
    pub name_line: u32,
    pub modifiers: Vec<String>,
    pub return_type: Option<TypeRef>,
    pub name: String,
    pub destructor: bool,
    pub params: Vec<VarDecl>,
    pub body: Option<Block>,
    pub prototype: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    If {
        line: u32,
        cond: Option<Expr>,
        then: Option<Box<Item>>,
        otherwise: Option<Box<Item>>,
    },
    For {
        line: u32,
        init: Option<Box<Item>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Option<Box<Item>>,
    },
    Foreach {
        line: u32,
        variables: Vec<VarDecl>,
        iter: Option<Expr>,
        body: Option<Box<Item>>,
    },
    While {
        line: u32,
        cond: Option<Expr>,
        body: Option<Box<Item>>,
    },
    Switch {
        line: u32,
        cond: Option<Expr>,
        body: Option<Box<Item>>,
    },
    Case {
        line: u32,
        value: Option<Expr>,
    },
    Default {
        line: u32,
    },
    Return {
        line: u32,
        value: Option<Expr>,
    },
    Jump {
        line: u32,
        keyword: String,
        target: Option<String>,
    },
    Delete {
        line: u32,
        value: Option<Expr>,
    },
    Thread {
        line: u32,
        call: Option<Expr>,
    },
    Command {
        line: u32,
        call: Option<Expr>,
    },
    Expr {
        line: u32,
        expr: Expr,
    },
    Empty {
        line: u32,
    },
}

impl Stmt {
    pub fn line(&self) -> u32 {
        match self {
            Stmt::If { line, .. }
            | Stmt::For { line, .. }
            | Stmt::Foreach { line, .. }
            | Stmt::While { line, .. }
            | Stmt::Switch { line, .. }
            | Stmt::Case { line, .. }
            | Stmt::Default { line }
            | Stmt::Return { line, .. }
            | Stmt::Jump { line, .. }
            | Stmt::Delete { line, .. }
            | Stmt::Thread { line, .. }
            | Stmt::Command { line, .. }
            | Stmt::Expr { line, .. }
            | Stmt::Empty { line } => *line,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub line: u32,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    Leaf {
        text: String,
    },
    // A digit the lexer could not read as a number, as in `5.Name()`: the
    // game takes it for a name, which no variable has.
    DigitName {
        text: String,
    },
    Unary {
        op: String,
        operand: Option<Box<Expr>>,
        // `a++` rather than `++a`.
        postfix: bool,
    },
    Binary {
        op: String,
        left: Box<Expr>,
        right: Option<Box<Expr>>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Member {
        object: Box<Expr>,
        name: String,
    },
    Index {
        object: Box<Expr>,
        index: Option<Box<Expr>>,
    },
    New {
        type_: Option<TypeRef>,
        args: Vec<Expr>,
    },
    Cast {
        type_: TypeRef,
        operand: Option<Box<Expr>>,
    },
    List {
        items: Vec<Expr>,
    },
}

impl Expr {
    pub fn leaf_text(&self) -> Option<&str> {
        match &self.kind {
            ExprKind::Leaf { text } => Some(text),
            _ => None,
        }
    }
}
