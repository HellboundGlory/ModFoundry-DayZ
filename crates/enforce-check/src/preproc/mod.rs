mod scan;
mod tables;

use crate::diag::Diagnostic;
use crate::input::SourceFile;

// The variant order is the game's word-kind numbering: 0 unterminated
// string, 1 hex, 2 decimal, 3 keyword or punctuation, 4 string,
// 5 identifier and also every one-character junk word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    UnterminatedString,
    HexNumber,
    DecimalNumber,
    Symbol,
    String,
    Identifier,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub line: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LexOutput {
    pub tokens: Vec<Token>,
    pub diags: Vec<Diagnostic>,
    // Every name defined at the end of the file, for the next file of the module.
    pub defines: Vec<String>,
}

pub fn lex(file: &SourceFile, defines: &[String]) -> LexOutput {
    scan::scan(file, defines)
}
