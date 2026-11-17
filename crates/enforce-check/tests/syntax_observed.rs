mod support;

use enforce_check::Target;
use support::{Row, assert_rows, row};

// Odd lexer and parser shapes. Each row is a probe and every line the game
// printed for it.
fn rows() -> Vec<Row> {
    vec![
        // #define with a body that starts with a bracket.
        row("lex-02", "#define PRX (1)\nvoid F()\n{\n\tint y = 2;\n}\n", &[(1, "Syntax error")]),
        // Two #else in one #ifdef.
        row("lex-03", "#ifdef PRNOPE\nvoid F1()\n{\n\tNope1();\n}\n#else\nvoid F2()\n{\n\tNope2();\n}\n#else\nvoid F3()\n{\n\tNope3();\n}\n#endif\n", &[(9, "Undefined function 'Nope2'")]),
        // #ifdef with a number.
        row("lex-04", "#ifdef 123\nvoid F1()\n{\n\tNope1();\n}\n#else\nvoid F2()\n{\n\tNope2();\n}\n#endif\n", &[(1, "CParser: #if[n]def expected an identifier"), (1, "Missing ';' at the end of line"), (6, "Missing ';' at the end of line"), (11, "Missing ';' at the end of line")]),
        // Two non-ASCII characters as a statement.
        row("lex-05", "void F()\n{\n\tint x = 1;\n\téé\n\tx = 2;\n}\n", &[(4, "Broken expression (missing ';'?)"), (4, "Error in parameters"), (4, "Error in parameters")]),
        // Non-ASCII characters at module level.
        row("lex-06", "éé\nvoid F()\n{\n\tNope();\n}\n", &[(1, "Broken expression (missing ';'?)")]),
        // Goto to a label on its own line.
        row("asm-D54a", "void F()\n{\n\tgoto done;\n\tdone:\n\treturn;\n}\n", &[(4, "Invalid statement ':'"), (5, "Syntax error")]),
        // A label line with no goto.
        row("asm-D54b", "void F()\n{\n\tint x = 1;\n\tdone:\n\tx = 2;\n}\n", &[(4, "Invalid statement ':'"), (5, "Syntax error")]),
        // A `for` with no test: each line is refused and the next is read.
        row("lim-e24", "void F(int a)\n{\n\tfor (;;) { break; }\n\tfor (;;) { break; }\n\ta = 5;\n}\n", &[(3, "Syntax error"), (4, "Syntax error")]),
        // A ] alone at module level.
        row("par-01", "class PrA\n{\n}\n]\nvoid F()\n{\n\tNope();\n}\n", &[(4, "Unknown code")]),
        // A ] alone in a class body.
        row("par-02", "class PrA\n{\n\tint x;\n\t]\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n", &[(4, "Unknown code")]),
        // Class declared inside a function.
        row("par-03", "void F()\n{\n\tclass PrIn\n\t{\n\t}\n}\n", &[(3, "Class/Enum definition inside block. (missing '}' ?)"), (4, "Unexpected scope")]),
        // Enum declared inside a function.
        row("par-04", "void F()\n{\n\tenum PrIn\n\t{\n\t\tPRV\n\t}\n}\n", &[(3, "Class/Enum definition inside block. (missing '}' ?)"), (4, "Unexpected scope")]),
        // Class declared inside a class, then a body error.
        row("par-05", "class PrOut\n{\n\tclass PrIn\n\t{\n\t}\n}\nvoid F()\n{\n\tNope();\n}\n", &[(3, "Class/Enum definition inside block. (missing '}' ?)"), (4, "Unexpected scope")]),
        // Brackets nested two, three and ten deep, one per line.
        row("par-15", "void F()\n{\n\tint a = ((1));\n\tint b = (((1)));\n\tint c = ((1) + 2);\n\tint d = ((((((((((1))))))))));\n\tNope();\n}\n", &[(3, "Broken casting"), (4, "Broken casting"), (6, "Broken casting")]),
        // Doubled brackets in other places, one per line.
        row("par-22", "int G(int a)\n{\n\treturn a;\n}\nint F(int a, int b)\n{\n\tint c = ((a + b));\n\tif ((a))\n\t{\n\t}\n\tif (((a)))\n\t{\n\t}\n\tG((a));\n\tG(((a)));\n\tc = (a) + ((b));\n\treturn ((a));\n}\n", &[(11, "Broken casting"), (14, "Syntax error")]),
        // par-16: the brackets may nest as deep as they like.
        row(
            "deep-brackets",
            Box::leak(format!("void F()
{{
	int x = {}1{};
}}
", "(".repeat(400), ")".repeat(400)).into_boxed_str()),
            &[(3, "Broken casting")],
        ),
        // A workshop mod writes a method with no braces: the game warns and
        // reads the statement after it as the body.
        row(
            "function-with-no-braces",
            "class PrA\n{\n\tint F() return 1;\n\tint G()\n\t{\n\t\treturn 2;\n\t}\n}\n",
            &[(3, "FIX-ME: Missing function scope")],
        ),
    ]
}

#[test]
fn odd_shapes_print_what_the_game_prints() {
    assert_rows(Target::RetailServer, &rows());
}

// A string literal is a string whatever letters it holds: only a stray word
// of non-ASCII characters is a broken expression.
#[test]
fn a_non_ascii_string_reads_like_an_ascii_one() {
    for target in [Target::RetailServer, Target::DiagServer] {
        let body = |text: &str| support::lines(target, &format!("void F()\n{{\n\t{text}\n\t1;\n}}\n"));
        assert_eq!(body("\"w\u{f6}rld\" +"), body("\"world\" +"), "{target:?}");
    }
}

// syn-055 on Diag: an invalid identifier is a counted error, but the rest of
// its line is still read, so no scope is left open.
#[test]
fn a_diag_invalid_identifier_keeps_the_rest_of_its_line() {
    let got = support::lines(Target::DiagServer, "class PrA { void local() {} }\n");
    assert_eq!(got, vec![(1, "Invalid identifier 'local'".to_string())]);
}

// par-23 to par-26: a bracketed word before a comma is a cast with nothing to
// cast and a broken expression besides; before a closing bracket it is only
// the cast.
#[test]
fn a_bracketed_word_before_a_comma_is_also_a_broken_expression() {
    const G: &str = "int G(int a, int b)\n{\n\treturn a;\n}\nint F(int a, int b)\n{\n";
    const BROKEN: &str = "Broken expression (missing ';'?)";
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "par-23",
                support::text(&[G, "\tif ((a)) {\n\t}\n\tint c = G((a), b);\n\treturn c;\n}\n"]),
                &[(7, "Broken casting"), (9, "Broken casting"), (9, BROKEN), (9, BROKEN), (10, "Syntax error")],
            ),
            row(
                "par-24",
                support::text(&[G, "\tint c = G((a), b);\n\treturn c;\n}\n"]),
                &[(7, "Broken casting"), (7, BROKEN), (7, BROKEN)],
            ),
            row("par-25", support::text(&[G, "\tG((a), b);\n\treturn a;\n}\n"]), &[(7, "Broken casting"), (7, BROKEN)]),
            row("par-26", support::text(&[G, "\tint c = G(b, (a));\n\treturn c;\n}\n"]), &[(7, "Broken casting")]),
        ],
    );
}

// lex-09 to lex-11: a string left open warns about the missing `;` even when
// the next line closes the block, whatever letters it holds.
#[test]
fn an_open_string_before_a_closing_brace_still_misses_its_semicolon() {
    const OPEN: &[(u32, &str)] = &[
        (3, "CParser: quoted string not closed on line 3, module Scripts/3_Game/probe.c"),
        (3, "Missing ';' at the end of line"),
        (3, "Broken expression (missing ';'?)"),
    ];
    let open = |id: &'static str, letters: &str| {
        row(id, support::text(&["void F()\n{\n\tstring s = \"", letters, ";\n}\n"]), OPEN)
    };
    assert_rows(
        Target::RetailServer,
        &[
            open("lex-09", "\u{e9}\u{e9}"),
            open("lex-10", "ab"),
            // Unobserved: a `;` inside the open string is not the statement's.
            open("semicolon-inside", ";"),
            row("lex-11", "void F()\n{\n\t// \u{e9}\u{e9}\n\tstring s = \"\u{e9}\u{e9}\";\n}\n", &[]),
        ],
    );
}
