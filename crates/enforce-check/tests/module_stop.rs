mod support;

use enforce_check::{Target, check};
use support::input;

fn lines(files: &[(&str, &str)]) -> Vec<(String, u32, String)> {
    check(&input(Target::RetailServer, files))
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.file.clone(), diagnostic.line, diagnostic.message.clone()))
        .collect()
}

// cnt-08: a syntax error at module level ends the parse of the whole module,
// so the files after it are never read.
#[test]
fn a_module_level_syntax_error_ends_the_module_parse() {
    let got = lines(&[("Scripts/3_Game/a.c", "int ;\n"), ("Scripts/3_Game/b.c", "float ;\n")]);
    assert_eq!(got, vec![("Scripts/3_Game/a.c".to_string(), 1, "Syntax error".to_string())]);
}

// The error count belongs to the module, not to the file: the limit is reached
// across two files.
#[test]
fn the_error_limit_counts_across_the_files_of_a_module() {
    let body = |count: usize| format!("void F{count}()\n{{\n\tint x;\n{}}}\n", "\tx = x +;\n".repeat(count));
    let (first, second) = (body(15), body(15));
    let got = lines(&[("Scripts/3_Game/a.c", &first), ("Scripts/3_Game/b.c", &second)]);
    let in_second: Vec<&(String, u32, String)> = got.iter().filter(|(file, _, _)| file.ends_with("b.c")).collect();
    // The function body and the module level each say the limit was passed.
    assert_eq!(got.len(), 23, "{got:?}");
    assert_eq!(in_second.len(), 8);
    assert_eq!(got.last().map(|(_, line, message)| (*line, message.as_str())), Some((9, "Too many errors")));
}

// lex-06: words outside ASCII at module level draw a warning, and the game
// gives the module up without an error line of its own.
#[test]
fn junk_words_at_module_level_fail_the_module_without_an_error_line() {
    let result = check(&input(
        Target::RetailServer,
        &[("Scripts/3_Game/a.c", "\u{e9}\u{e9}\nvoid F()\n{\n\tNope();\n}\n")],
    ));
    let printed: Vec<(u32, &str)> = result.diagnostics.iter().map(|d| (d.line, d.message.as_str())).collect();
    assert_eq!(printed, vec![(1, "Broken expression (missing ';'?)")]);
    assert_eq!(result.failed_modules, vec![enforce_check::Module::Game]);
}

// syn-028 and syn-042 on the Diag server: a rule the retail targets only warn
// about is a counted error there, so the rest of its line is dropped too.
#[test]
fn a_soft_error_drops_the_rest_of_its_line_on_a_diag_target() {
    let lines = |target, text: &str| -> Vec<String> {
        check(&input(target, &[("Scripts/3_Game/a.c", text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    };
    let open = "Opened scope at the end of file, missing '}' ?";
    let no_scope = "class PrA { void F() int x; }";
    assert_eq!(lines(Target::RetailServer, no_scope), ["Missing function scope"]);
    assert_eq!(lines(Target::DiagServer, no_scope), ["Missing function scope", open]);
    let brackets = "class PrA { void F() { int x = (1 + 2; } }";
    assert_eq!(lines(Target::DiagServer, brackets), ["Unmatched brackets detected!", open, open]);
}
