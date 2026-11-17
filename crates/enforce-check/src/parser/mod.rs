mod ast;
mod decl;
mod expr;
mod stmt;
mod top;
mod words;

use crate::diag::{Diagnostic, Severity};
use crate::input::{Module, SourceFile};
use crate::preproc::Token;

pub use ast::*;
pub use words::is_assignment;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseOutput {
    pub ast: FileAst,
    pub diags: Vec<Diagnostic>,
    // The errors counted in the module so far, this file included.
    pub errors: u32,
    // An error at module level ended the parse, which ends it for the module.
    pub stopped: bool,
    // The game refused the module here and printed no error line for it.
    pub failed_silently: bool,
    // The parse ended at one of its `Limits`, not where the game would end it.
    pub over_limit: bool,
}

// How far the parser nests before it ends the parse as if the error limit was
// hit, so hostile input cannot overflow the stack. The tree it builds is as
// deep, and every later stage walks it by recursion.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    // Calls of the recursive rules open at once; a nested block takes two.
    nesting: u32,
    // Operators and postfix steps on one path through an expression.
    chain: u32,
}

impl Limits {
    // Safe on a 2 MB stack.
    pub(crate) const CALLER_STACK: Limits = Limits {
        nesting: 100,
        chain: 500,
    };
    // Past what the game compiles: it reads 3,000 nested blocks and a sum of
    // 3,000 terms, and a function much longer than that crashes it. Only for
    // a caller that runs every stage on a stack as deep as `check` makes.
    pub(crate) const DEEP_STACK: Limits = Limits {
        nesting: 8192,
        chain: 8192,
    };
}

pub fn parse(file: &SourceFile, tokens: &[Token]) -> ParseOutput {
    parse_in_module(file, tokens, 0, false, Limits::CALLER_STACK)
}

// One parser reads every file of a module, so the error count carries from
// file to file: `errors_before` is what the files before this one counted.
// `strict` is a Diag target, where a soft rule is a counted error.
pub(crate) fn parse_in_module(
    file: &SourceFile,
    tokens: &[Token],
    errors_before: u32,
    strict: bool,
    limits: Limits,
) -> ParseOutput {
    let mut parser = Parser::new(file, tokens, limits);
    parser.errors = errors_before;
    parser.strict = strict;
    let ast = parser.parse_file();
    ParseOutput {
        ast,
        diags: parser.diags,
        errors: parser.errors,
        stopped: parser.stopped,
        failed_silently: parser.failed_silently,
        over_limit: parser.over_limit,
    }
}

struct Line {
    number: u32,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Enclosing {
    Module,
    Class,
    Block,
}

pub(crate) struct Parser<'a> {
    file: &'a SourceFile,
    tokens: &'a [Token],
    lines: Vec<Line>,
    line: usize,
    word: usize,
    depth: u32,
    limits: Limits,
    // Calls of the recursive rules that are open right now.
    nesting: u32,
    // Operators and postfix steps open on the path from the outermost expression.
    chain: u32,
    // Line of the `class` keyword whose body is being read, for the class-body
    // `Syntax error` that the game reports there.
    class_line: Option<u32>,
    // Nesting depth of the type template lists being read.
    template_open: i32,
    // A line and the place of its last `>`, if it has one.
    pub(crate) last_closer: Option<(usize, Option<usize>)>,

    diags: Vec<Diagnostic>,
    errors: u32,
    // A counted error drops the rest of its line.
    skip: bool,
    stopped: bool,
    // An error the game gives up on altogether: every open level ends.
    halted: bool,
    strict: bool,
    failed_silently: bool,
    over_limit: bool,
    // The statement being read held a cast with nothing to cast.
    broken_cast: bool,
    // The level being read ends after the statement just read.
    leave_level: bool,
}

// The game tests `count > 20` at the top of each level loop.
const ERROR_LIMIT: u32 = 20;

impl<'a> Parser<'a> {
    fn new(file: &'a SourceFile, tokens: &'a [Token], limits: Limits) -> Parser<'a> {
        Parser {
            file,
            tokens,
            lines: group_lines(tokens),
            line: 0,
            word: 0,
            depth: 0,
            limits,
            nesting: 0,
            chain: 0,
            class_line: None,
            template_open: 0,
            last_closer: None,

            diags: Vec::new(),
            errors: 0,
            skip: false,
            stopped: false,
            halted: false,
            strict: false,
            failed_silently: false,
            over_limit: false,
            broken_cast: false,
            leave_level: false,
        }
    }

    fn parse_file(&mut self) -> FileAst {
        let items = self.parse_level(Enclosing::Module, 0);
        FileAst { items }
    }

    fn line_end(&self) -> usize {
        self.lines
            .get(self.line)
            .map_or(self.tokens.len(), |line| line.end)
    }

    fn line_start(&self) -> usize {
        self.lines
            .get(self.line)
            .map_or(self.tokens.len(), |line| line.start)
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn cur(&self) -> Option<&'a Token> {
        if self.word < self.line_end() {
            self.tokens.get(self.word)
        } else {
            None
        }
    }

    pub(crate) fn cur_text(&self) -> &'a str {
        self.cur().map_or("", |token| token.text.as_str())
    }

    pub(crate) fn peek_text(&self) -> Option<&'a str> {
        if self.word + 1 < self.line_end() {
            self.tokens
                .get(self.word + 1)
                .map(|token| token.text.as_str())
        } else {
            None
        }
    }

    pub(crate) fn at_line_end(&self) -> bool {
        self.word >= self.line_end()
    }

    pub(crate) fn bump(&mut self) {
        if self.word < self.line_end() {
            self.word += 1;
        }
    }

    // A punctuation or keyword word, never a string or number that spells it.
    pub(crate) fn is_symbol(&self, text: &str) -> bool {
        self.cur()
            .is_some_and(|token| token.kind == crate::preproc::TokenKind::Symbol)
            && self.cur_text() == text
    }

    pub(crate) fn is_any_symbol(&self, texts: &[&str]) -> bool {
        texts.iter().any(|text| self.is_symbol(text))
    }

    pub(crate) fn eat(&mut self, text: &str) -> bool {
        if self.is_symbol(text) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn next_line(&mut self) -> bool {
        if self.line + 1 < self.lines.len() {
            self.line += 1;
            self.word = self.line_start();
            true
        } else {
            self.line = self.lines.len();
            self.word = self.tokens.len();
            false
        }
    }

    pub(crate) fn drop_rest_of_line(&mut self) {
        self.word = self.line_end();
    }

    pub(crate) fn words_from_cursor(&self) -> &'a [Token] {
        let start = self.word.min(self.tokens.len());
        let end = self.line_end().min(self.tokens.len());
        self.tokens.get(start..end).unwrap_or(&[])
    }

    // The line under the cursor; past its last word it is still that line.
    pub(crate) fn cursor_line(&self) -> u32 {
        match self.cur() {
            Some(token) => token.line,
            None => self
                .lines
                .get(self.line)
                .map_or(self.last_line(), |line| line.number),
        }
    }

    // The line of the word read last.
    pub(crate) fn previous_line(&self) -> u32 {
        match self.word.checked_sub(1).and_then(|at| self.tokens.get(at)) {
            Some(token) => token.line,
            None => self.cursor_line(),
        }
    }

    pub(crate) fn last_line(&self) -> u32 {
        self.lines.last().map_or(1, |line| line.number)
    }

    pub(crate) fn found_word(&self) -> String {
        self.cur().map_or(String::new(), |token| token.text.clone())
    }

    fn diagnostic(&self, line: u32, message: String) -> Diagnostic {
        Diagnostic {
            file: self.file.path.clone(),
            line,
            module: Module::from(self.file.layer),
            severity: Severity::Error,
            message,
            fixme: false,
            game_crash: false,
            reports_only: false,
            related: None,
        }
    }

    pub(crate) fn error(&mut self, message: String) {
        self.error_at(self.cursor_line(), message);
    }

    pub(crate) fn error_at(&mut self, line: u32, message: String) {
        self.diags.push(self.diagnostic(line, message));
        self.errors += 1;
        self.skip = true;
    }

    pub(crate) fn warning_at(&mut self, line: u32, message: String) {
        let mut diagnostic = self.diagnostic(line, message);
        diagnostic.severity = Severity::Warning;
        self.diags.push(diagnostic);
    }

    pub(crate) fn soft(&mut self, message: String) {
        self.soft_at(self.cursor_line(), message);
    }

    // A soft rule: warning with the `FIX-ME:` prefix on retail, counted error on
    // the Diag targets. The target picks the severity in `check`.
    pub(crate) fn soft_at(&mut self, line: u32, message: String) {
        self.soft_rule(line, message);
        if self.strict {
            self.skip = true;
        }
    }

    // A soft rule that the Diag targets count but whose line is still read.
    pub(crate) fn soft_keeping_line_at(&mut self, line: u32, message: String) {
        self.soft_rule(line, message);
    }

    fn soft_rule(&mut self, line: u32, message: String) {
        let mut diagnostic = self.diagnostic(line, message);
        diagnostic.severity = Severity::Warning;
        diagnostic.fixme = true;
        self.diags.push(diagnostic);
        if self.strict {
            self.errors += 1;
        }
    }

    // False when the nesting limit is passed; the caller returns at once and
    // skips `leave`.
    pub(crate) fn enter(&mut self) -> bool {
        if self.nesting >= self.limits.nesting {
            self.stop_over_limit();
            return false;
        }
        self.nesting += 1;
        true
    }

    pub(crate) fn chain(&self) -> u32 {
        self.chain
    }

    // Counts one more operator or postfix step; false ends the parse.
    pub(crate) fn extend_chain(&mut self) -> bool {
        self.chain += 1;
        if self.chain > self.limits.chain {
            self.stop_over_limit();
            return false;
        }
        true
    }

    pub(crate) fn set_chain(&mut self, chain: u32) {
        self.chain = chain;
    }

    fn stop_over_limit(&mut self) {
        self.over_limit = true;
        self.stop_parse();
    }

    // Ends the parse as if the error limit was hit.
    pub(crate) fn stop_parse(&mut self) {
        self.errors = self.errors.max(ERROR_LIMIT + 1);
    }

    pub(crate) fn leave(&mut self) {
        self.nesting -= 1;
    }

    pub(crate) fn stop_module(&mut self) {
        self.stopped = true;
    }

    pub(crate) fn halt(&mut self) {
        self.halted = true;
        self.stopped = true;
    }

    pub(crate) fn note_broken_cast(&mut self) {
        self.broken_cast = true;
    }

    pub(crate) fn take_broken_cast(&mut self) -> bool {
        std::mem::take(&mut self.broken_cast)
    }

    pub(crate) fn end_level_after_statement(&mut self) {
        self.leave_level = true;
    }

    pub(crate) fn take_leave_level(&mut self) -> bool {
        std::mem::take(&mut self.leave_level)
    }

    pub(crate) fn fail_silently(&mut self) {
        self.failed_silently = true;
        self.halt();
    }

    pub(crate) fn halted(&self) -> bool {
        self.halted
    }

    pub(crate) fn errors_over_limit(&self) -> bool {
        self.errors > ERROR_LIMIT
    }

    pub(crate) fn take_skip(&mut self) -> bool {
        let skip = self.skip;
        self.skip = false;
        skip
    }

    pub(crate) fn skipping(&self) -> bool {
        self.skip
    }

    pub(crate) fn depth(&self) -> u32 {
        self.depth
    }

    pub(crate) fn set_depth(&mut self, depth: u32) {
        self.depth = depth;
    }

    pub(crate) fn class_line(&self) -> Option<u32> {
        self.class_line
    }

    pub(crate) fn set_class_line(&mut self, line: Option<u32>) -> Option<u32> {
        std::mem::replace(&mut self.class_line, line)
    }

    // `Type name` on this line, the shape the declaration parser takes.
    pub(crate) fn starts_declaration(&self) -> bool {
        let words = self.words_from_cursor();
        match (words.first(), words.get(1)) {
            (Some(first), Some(second)) => {
                !words::is_delimiter(&first.text)
                    && matches!(second.kind, crate::preproc::TokenKind::Identifier)
            }
            _ => false,
        }
    }

    pub(crate) fn open_template_lists(&mut self) {
        self.template_open += 1;
    }

    pub(crate) fn close_template_lists(&mut self) {
        self.template_open = 0;
    }

    pub(crate) fn nest_template(&mut self, levels: i32) {
        self.template_open += levels;
    }

    pub(crate) fn template_open(&self) -> i32 {
        self.template_open
    }

    // True when the line after the cursor's line opens with `text`.
    pub(crate) fn next_line_starts_with(&self, text: &str) -> bool {
        let Some(next) = self.lines.get(self.line + 1) else {
            return false;
        };
        self.tokens
            .get(next.start)
            .is_some_and(|token| token.kind == crate::preproc::TokenKind::Symbol && token.text == text)
    }

    pub(crate) fn restore(&mut self, line: usize, word: usize) {
        self.line = line;
        self.word = word;
    }

    pub(crate) fn line_has_symbol(&self, number: u32, symbol: &str) -> bool {
        self.lines
            .iter()
            .find(|line| line.number == number)
            .is_some_and(|line| {
                self.tokens
                    .get(line.start..line.end)
                    .is_some_and(|words| words.iter().any(|token| token.text == symbol))
            })
    }

    // The `[` at the cursor opens a list whose closing `]` is the last word of the line.
    pub(crate) fn list_fills_line(&self) -> bool {
        let mut depth = 0u32;
        let words = self.words_from_cursor();
        for (index, token) in words.iter().enumerate() {
            match token.text.as_str() {
                "[" => depth += 1,
                "]" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return index + 1 == words.len();
                    }
                }
                _ => {}
            }
        }
        false
    }

    // Puts the cursor back at the first word of `number`, so the enclosing loop
    // reads the lines after it again.
    pub(crate) fn rewind_to_line(&mut self, number: u32) {
        if let Some(index) = self.lines.iter().position(|line| line.number == number) {
            self.line = index;
            self.word = self.lines[index].start;
        }
    }
}

fn group_lines(tokens: &[Token]) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if line.number == token.line => line.end = index + 1,
            _ => lines.push(Line {
                number: token.line,
                start: index,
                end: index + 1,
            }),
        }
    }
    lines
}
