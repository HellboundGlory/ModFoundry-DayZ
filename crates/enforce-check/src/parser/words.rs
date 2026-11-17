pub const VARIABLE_MODIFIERS: [&str; 12] = [
    "owned",
    "static",
    "const",
    "out",
    "autoptr",
    "ref",
    "inout",
    "notnull",
    "reference",
    "local",
    "private",
    "protected",
];

pub const FUNCTION_MODIFIERS: [&str; 15] = [
    "proto",
    "static",
    "volatile",
    "private",
    "native",
    "protected",
    "event",
    "owned",
    "external",
    "override",
    "ref",
    "notnull",
    "func",
    "sealed",
    "autoptr",
];

pub const CLASS_MODIFIERS: [&str; 2] = ["modded", "sealed"];

pub const ASSIGNMENT_OPERATORS: [&str; 11] = [
    "=", "+=", "-=", "*=", "/=", "|=", "&=", "<<=", ">>=", "^=", "%=",
];

pub const SOFT_NAMES: [&str; 10] = [
    "Obsolete",
    "Friend",
    "Cast",
    "CompileBreak",
    "func",
    "EnumLinear",
    "EnumBitFlag",
    "EnumCount",
    "EnumMin",
    "EnumMax",
];

pub const DELIMITERS: [&str; 40] = [
    "[",
    "{",
    "}",
    ";",
    "::",
    "(",
    ")",
    ".",
    ":",
    "if",
    "else",
    "for",
    "foreach",
    "while",
    "switch",
    "case",
    "break",
    "continue",
    "delete",
    "return",
    "goto",
    "debug",
    "thread",
    "typedef",
    "func",
    "=",
    "+=",
    "-=",
    "*=",
    "^=",
    "/=",
    "|=",
    "&=",
    "<<=",
    ">>=",
    "default",
    "++",
    "--",
    "new",
    "!",
];

pub fn is_assignment(op: &str) -> bool {
    ASSIGNMENT_OPERATORS.contains(&op)
}

pub fn is_variable_modifier(word: &str) -> bool {
    VARIABLE_MODIFIERS.contains(&word)
}

pub fn is_function_modifier(word: &str) -> bool {
    FUNCTION_MODIFIERS.contains(&word)
}

pub fn is_class_modifier(word: &str) -> bool {
    CLASS_MODIFIERS.contains(&word)
}

pub fn is_soft_name(word: &str) -> bool {
    SOFT_NAMES.contains(&word)
}

pub fn is_delimiter(word: &str) -> bool {
    DELIMITERS.contains(&word)
}

pub fn is_operator(word: &str) -> bool {
    matches!(
        word,
        "." | "[" | "++" | "--" | "!" | "~" | "*" | "/" | "%" | "+" | "-" | "<<" | ">>" | ">"
            | ">=" | "<" | "<=" | "==" | "!=" | "&" | "^" | "|" | "&&" | "||" | ":"
    ) || is_assignment(word)
}

// Smaller number binds tighter, as the game's precedence table reads.
pub fn precedence(word: &str) -> Option<u8> {
    Some(match word {
        "." | "[" => 1,
        "++" | "--" | "!" | "~" => 2,
        "*" | "/" | "%" => 3,
        "+" | "-" => 4,
        "<<" | ">>" => 5,
        ">" | ">=" | "<" | "<=" => 6,
        "==" | "!=" => 7,
        "&" => 8,
        "^" => 9,
        "|" => 10,
        "&&" => 11,
        "||" => 12,
        ":" => 13,
        "=" | "+=" | "-=" | "*=" | "/=" | "|=" | "&=" | "<<=" | ">>=" | "^=" => 14,
        _ => return None,
    })
}
