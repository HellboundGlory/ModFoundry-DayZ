use enforce_check::preproc::{Token, TokenKind, lex};
use enforce_check::{CheckInput, Layer, Module, Severity, SourceFile, Target, check};

fn file(text: &str) -> SourceFile {
    SourceFile {
        path: "Scripts/3_Game/a.c".into(),
        layer: Layer::Game,
        mod_index: 0,
        text: text.into(),
    }
}

fn tokens(text: &str, defines: &[&str]) -> Vec<Token> {
    let defines: Vec<String> = defines.iter().map(|d| (*d).to_string()).collect();
    lex(&file(text), &defines).tokens
}

fn token(kind: TokenKind, text: &str, line: u32) -> Token {
    Token {
        kind,
        text: text.into(),
        line,
    }
}

#[test]
fn words_carry_their_physical_line() {
    let got = tokens("int x = 1;\nclass A {}\n", &[]);
    let expected = vec![
        token(TokenKind::Identifier, "int", 1),
        token(TokenKind::Identifier, "x", 1),
        token(TokenKind::Symbol, "=", 1),
        token(TokenKind::DecimalNumber, "1", 1),
        token(TokenKind::Symbol, ";", 1),
        token(TokenKind::Symbol, "class", 2),
        token(TokenKind::Identifier, "A", 2),
        token(TokenKind::Symbol, "{", 2),
        token(TokenKind::Symbol, "}", 2),
    ];
    assert_eq!(got, expected);
}

#[test]
fn numbers_follow_the_scan_rules() {
    let cases = [
        ("x-1", vec!["x", "-1"]),
        ("x -1", vec!["x", "-1"]),
        ("x - 1", vec!["x", "-", "1"]),
        ("-x", vec!["-", "x"]),
        (".5", vec![".", "5"]),
        ("5.", vec!["5", "."]),
        ("1e3", vec!["1e3"]),
        ("1E3", vec!["1", "E3"]),
        ("5f", vec!["5", "f"]),
        ("1_000", vec!["1", "_000"]),
        ("0x1F", vec!["0x1F"]),
        ("0xG", vec!["0", "xG"]),
    ];
    for (text, expected) in cases {
        let got: Vec<String> = tokens(text, &[]).into_iter().map(|t| t.text).collect();
        assert_eq!(got, expected, "{text}");
    }
}

#[test]
fn number_kinds_split_decimal_hex_and_junk() {
    let got = tokens("0x1F 12 5f", &[]);
    let expected = vec![
        token(TokenKind::HexNumber, "0x1F", 1),
        token(TokenKind::DecimalNumber, "12", 1),
        token(TokenKind::DecimalNumber, "5", 1),
        token(TokenKind::Identifier, "f", 1),
    ];
    assert_eq!(got, expected);
}

#[test]
fn reserved_words_and_operators_are_symbols() {
    let got = tokens("class local int", &[]);
    let expected = vec![
        token(TokenKind::Symbol, "class", 1),
        token(TokenKind::Symbol, "local", 1),
        token(TokenKind::Identifier, "int", 1),
    ];
    assert_eq!(got, expected);

    let got: Vec<String> = tokens("<<= << <= < %= % >>", &[])
        .into_iter()
        .map(|t| t.text)
        .collect();
    let expected = ["<<=", "<<", "<=", "<", "%", "=", "%", ">>"];
    assert_eq!(got, expected);
}

#[test]
fn comments_and_strings() {
    let got: Vec<String> = tokens("a // x\nb /* c\nd */ e \"f g\"", &[])
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert_eq!(got, ["a", "b", "e", "f g"]);

    let got: Vec<u32> = tokens("a /* c\nd\ne */ b", &[])
        .into_iter()
        .map(|t| t.line)
        .collect();
    assert_eq!(got, [1, 3]);
}

#[test]
fn string_escapes_drop_the_backslash() {
    let cases = [
        ("\"a\\qb\"", "aqb"),
        ("\"a\\\\b\"", "a\\b"),
        ("\"a\\tb\"", "a\tb"),
        ("\"a\\\"b\"", "a\"b"),
    ];
    for (text, expected) in cases {
        let got = tokens(text, &[]);
        assert_eq!(got.len(), 1, "{text}");
        assert_eq!(got[0].kind, TokenKind::String, "{text}");
        assert_eq!(got[0].text, expected, "{text}");
    }
}

#[test]
fn a_newline_before_the_closing_quote_ends_the_string() {
    let got = tokens("string s = \"abc; } }", &[]);
    let last = got.last().expect("tokens");
    assert_eq!(last.kind, TokenKind::UnterminatedString);
    assert_eq!(last.text, "abc; } }");
    assert_eq!(last.line, 1);
}

#[test]
fn the_condition_tail_is_lexed_as_code() {
    let got = tokens("#ifdef SERVER && NOTDEFINED\nint x;\n#endif", &["SERVER"]);
    let texts: Vec<&str> = got.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["&&", "NOTDEFINED", "int", "x", ";"]);
    assert_eq!(got[0].line, 1);
    assert_eq!(got[2].line, 2);
}

#[test]
fn an_inactive_branch_produces_nothing() {
    let got = tokens("#ifdef NOTDEFINED && SERVER\nint x;\n#endif", &["SERVER"]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn branches_nest_and_else_flips() {
    let text = "#ifdef A\n#ifdef B\nx\n#else\ny\n#endif\n#else\nz\n#endif";
    let texts: Vec<String> = tokens(text, &["A"]).into_iter().map(|t| t.text).collect();
    assert_eq!(texts, ["y"]);
    let texts: Vec<String> = tokens(text, &["A", "B"])
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert_eq!(texts, ["x"]);
    let texts: Vec<String> = tokens(text, &[]).into_iter().map(|t| t.text).collect();
    assert_eq!(texts, ["z"]);
}

#[test]
fn ifndef_inverts_the_lookup() {
    let text = "#ifndef A\nx\n#endif\n#ifndef B\ny\n#endif";
    let texts: Vec<String> = tokens(text, &["A"]).into_iter().map(|t| t.text).collect();
    assert_eq!(texts, ["y"]);
}

#[test]
fn a_defined_inner_branch_stays_inactive_under_an_inactive_parent() {
    let text = "#ifdef A\n#ifdef B\nx\n#endif\ny\n#endif\nz";
    let texts: Vec<String> = tokens(text, &["B"]).into_iter().map(|t| t.text).collect();
    assert_eq!(texts, ["z"]);
}

#[test]
fn an_ifdef_with_no_name_inside_a_branch_is_silent() {
    let text = "#ifdef A\n#ifdef\nx\n#endif\n#endif";
    assert!(messages(text, &["A"]).is_empty());
    assert!(messages(text, &[]).is_empty());
}

#[test]
fn a_stray_else_or_endif_is_an_ordinary_word() {
    for text in ["#endif", "#else"] {
        let got = tokens(text, &[]);
        assert_eq!(got.len(), 1, "{text}");
        assert_eq!(got[0].kind, TokenKind::Symbol, "{text}");
        assert_eq!(got[0].text, text);
    }
}

#[test]
fn if_is_not_a_directive() {
    let got: Vec<String> = tokens("#if 1\n#endif", &[])
        .into_iter()
        .map(|t| t.text)
        .collect();
    assert_eq!(got, ["#", "if", "1", "#endif"]);
}

#[test]
fn a_comment_in_an_inactive_branch_hides_the_endif() {
    let got = tokens("#ifdef NOTDEFINED\n/*\n#endif\nint x;", &[]);
    assert!(got.is_empty(), "{got:?}");
    let still_hidden = tokens("#ifdef NOTDEFINED\n/*\n#endif\nint x;\n*/int y;", &[]);
    assert!(still_hidden.is_empty(), "{still_hidden:?}");
    let closes = tokens(
        "#ifdef NOTDEFINED\n/*\n#endif\n*/\nint y;\n#endif\nint z;",
        &[],
    );
    let texts: Vec<&str> = closes.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["int", "z", ";"]);
}

#[test]
fn define_records_the_name_for_ifdef() {
    let text = "#define FLAG\n#ifdef FLAG\nx\n#endif\n#ifdef OTHER\ny\n#endif";
    let texts: Vec<String> = tokens(text, &[]).into_iter().map(|t| t.text).collect();
    assert_eq!(texts, ["x"]);
}

#[test]
fn line_and_file_macros_are_strings() {
    let got = tokens("a\n__LINE__ __FILE__", &[]);
    let expected = vec![
        token(TokenKind::Identifier, "a", 1),
        token(TokenKind::String, "2", 2),
        token(TokenKind::String, "Scripts/3_Game/a.c", 2),
    ];
    assert_eq!(got, expected);
}

fn messages(text: &str, defines: &[&str]) -> Vec<(u32, String, bool, bool)> {
    let defines: Vec<String> = defines.iter().map(|d| (*d).to_string()).collect();
    lex(&file(text), &defines)
        .diags
        .into_iter()
        .map(|d| (d.line, d.message, d.fixme, d.game_crash))
        .collect()
}

type Expected = Vec<(u32, &'static str, bool, bool)>;

#[test]
fn diagnostics_follow_the_observed_probes() {
    let cases: Vec<(&str, &[&str], Expected)> = vec![
        (
            "#ifdef",
            &[],
            vec![(1, "CParser: #if[n]def expected an identifier", false, false)],
        ),
        (
            "class A { void F() { string s = \"abc; } }",
            &[],
            vec![(
                1,
                "CParser: quoted string not closed on line 1, module Scripts/3_Game/a.c",
                false,
                false,
            )],
        ),
        (
            "#define X 1\nclass A { string s = \"X\"; }",
            &[],
            vec![
                (1, "Expected 'class', not a '1'", false, false),
                (1, "Invalid identifier '1'", true, false),
            ],
        ),
        (
            "#define X 1\nclass A { int y = X; }",
            &[],
            vec![(2, "Expanding macro 'X'", false, true)],
        ),
    ];
    for (text, defines, expected) in cases {
        let got = messages(text, defines);
        let expected: Vec<(u32, String, bool, bool)> = expected
            .into_iter()
            .map(|(line, message, fixme, crash)| (line, message.to_string(), fixme, crash))
            .collect();
        assert_eq!(got, expected, "{text}");
    }
}

#[test]
fn an_identifier_that_is_only_defined_is_silent() {
    assert!(messages("#define FLAG\n#ifdef FLAG\nint x;\n#endif", &[]).is_empty());
}

#[test]
fn a_bom_is_not_skipped() {
    let got = tokens("\u{feff}class A {}", &[]);
    let texts: Vec<&str> = got.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["\u{feff}", "class", "A", "{", "}"]);
    assert_eq!(got[0].kind, TokenKind::Identifier);
    assert_eq!(got[0].line, 1);
}

#[test]
fn lexing_never_panics() {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "abc019_ -+*/%<>=!&|^~(){}[],;:.\\\"'\n\r\t#\u{0}\u{e9}ifdelsnx"
        .chars()
        .collect();
    let defines = ["FLAG".to_string(), "SERVER".to_string()];
    for _ in 0..200 {
        let len = (next() % 200) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let _ = lex(&file(&text), &defines);
    }

    let fixture = include_str!("../../../tests/fixtures/preproc/mixed.c");
    let chars: Vec<char> = fixture.chars().collect();
    for end in 0..=chars.len() {
        let prefix: String = chars[..end].iter().collect();
        let _ = lex(&file(&prefix), &defines);
    }
}

#[test]
fn check_adjusts_the_fixme_severity_for_the_target() {
    let input = CheckInput {
        target: Target::DiagClient,
        mods: Vec::new(),
        files: vec![file("#define X 1\nclass A {}\n")],
        extra_defines: Vec::new(),
        input_names: Vec::new(),
    };
    let result = check(&input);
    let messages: Vec<(&str, bool, bool)> = result
        .diagnostics
        .iter()
        .map(|d| (d.message.as_str(), d.fixme, d.severity == Severity::Error))
        .collect();
    assert_eq!(
        messages,
        [
            ("Expected 'class', not a '1'", false, true),
            ("Invalid identifier '1'", false, true),
        ]
    );
    assert_eq!(result.failed_modules, [Module::Game]);
}

fn last_line(text: &str, defines: &[&str]) -> u32 {
    tokens(text, defines).last().unwrap().line
}

#[test]
fn physical_lines_are_counted_everywhere() {
    let cases = [
        ("an escaped line break inside a string", "s = \"a\\\nb\";\nc", 3),
        ("an escaped CRLF inside a string", "s = \"a\\\r\nb\";\nc", 3),
        ("a skipped branch", "a\n#ifdef Z\nx\ny\n#endif\nc", 6),
        ("a comment in a skipped branch", "#ifdef Z\n/* x\ny */\n#endif\nc", 5),
        ("CRLF files", "a\r\nb\r\n#ifdef Z\r\nx\r\n#endif\r\nc", 6),
        ("a continued define line", "#define X\\\n\nint y;\nZ", 4),
    ];
    for (what, text, expected) in cases {
        assert_eq!(last_line(text, &[]), expected, "{what}");
    }
}

#[test]
fn deep_nesting_stays_fast_and_balanced() {
    let started = std::time::Instant::now();
    let mut text = "#ifdef A\n".repeat(200_000);
    text.push_str("x\n");
    text.push_str(&"#endif\n".repeat(200_000));
    text.push_str("y\n");
    let got = tokens(&text, &["A"]);
    let words: Vec<&str> = got.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(words, ["x", "y"]);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

#[test]
fn nested_branches_flip_against_their_parents() {
    let text = "#ifdef A\n#ifdef B\nb1\n#else\nb2\n#endif\n#else\n#ifdef B\nc1\n#else\nc2\n#endif\n#endif\n";
    let cases = [
        (vec!["A", "B"], "b1"),
        (vec!["A"], "b2"),
        (vec!["B"], "c1"),
        (vec![], "c2"),
    ];
    for (defines, expected) in cases {
        let got = tokens(text, &defines);
        let words: Vec<&str> = got.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, [expected], "{defines:?}");
    }
}

#[test]
fn a_define_carries_over_to_later_files_of_the_same_module_only() {
    let in_layer = |path: &str, layer: Layer, text: &str| SourceFile {
        path: path.into(),
        layer,
        mod_index: 0,
        text: text.into(),
    };
    let input = CheckInput {
        target: Target::DiagServer,
        mods: Vec::new(),
        files: vec![
            in_layer("Scripts/3_Game/a.c", Layer::Game, "#define FLAG\n"),
            in_layer("Scripts/3_Game/b.c", Layer::Game, "#ifdef FLAG\n\"open\n#endif\n"),
            in_layer("Scripts/4_World/c.c", Layer::World, "#ifdef FLAG\n\"open\n#endif\n"),
        ],
        extra_defines: Vec::new(),
        input_names: Vec::new(),
    };
    let result = check(&input);
    let at: Vec<&str> = result.diagnostics.iter().map(|d| d.file.as_str()).collect();
    assert_eq!(at, ["Scripts/3_Game/b.c", "Scripts/3_Game/b.c"]);
}
