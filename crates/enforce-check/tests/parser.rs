use enforce_check::{CheckInput, Layer, ModInfo, Severity, SourceFile, Target, check};

// One row per probe: the probe source, then the messages the game printed for
// that file, taken from `oracle/results/<id>.txt`. `First` is for a row whose
// later lines come from a later stage than the parser.
enum Expect {
    Nothing,
    All(&'static [(u32, char, &'static str)]),
    First(u32, char, &'static str, usize),
    Crash,
}

const fn all(messages: &'static [(u32, char, &'static str)]) -> Expect {
    Expect::All(messages)
}

const fn first(line: u32, kind: char, message: &'static str, count: usize) -> Expect {
    Expect::First(line, kind, message, count)
}

struct Case {
    id: &'static str,
    files: &'static [(&'static str, Layer, &'static str)],
    expected: Expect,
}

// The game prints the file path in the message; it is the path the lexer holds.
fn expected_message(case: &Case, message: &str) -> String {
    let path = case.files.first().map_or("", |(path, _, _)| *path);
    message.replace("@FILE@", path)
}

const GAME: &str = "Scripts/3_Game/a.c";

fn game(text: &'static str) -> &'static [(&'static str, Layer, &'static str)] {
    Box::leak(vec![(GAME, Layer::Game, text)].into_boxed_slice())
}

struct Seen {
    line: u32,
    kind: char,
    message: String,
    crash: bool,
}

fn run(case: &Case) -> Vec<Seen> {
    let files: Vec<SourceFile> = case
        .files
        .iter()
        .map(|(path, layer, text)| SourceFile {
            path: (*path).into(),
            layer: *layer,
            mod_index: 1,
            text: (*text).into(),
        })
        .collect();
    let input = CheckInput {
        target: Target::RetailServer,
        mods: vec![ModInfo {
            name: case.id.into(),
            cfg_class: format!("Probe_{}", case.id.replace('-', "_")),
        }],
        files,
        extra_defines: Vec::new(),
        input_names: Vec::new(),
    };
    check(&input)
        .diagnostics
        .into_iter()
        .map(|d| Seen {
            line: d.line,
            kind: match (d.severity, d.fixme) {
                (_, true) => 'F',
                (Severity::Error, false) => 'E',
                (Severity::Warning, false) => 'W',
            },
            message: d.message,
            crash: d.game_crash,
        })
        .collect()
}

fn summary(case: &Case) -> String {
    let got = run(case);
    let seen: Vec<(u32, char, &str)> = got
        .iter()
        .map(|d| (d.line, d.kind, d.message.as_str()))
        .collect();
    match case.expected {
        Expect::Nothing => {
            if !seen.is_empty() {
                return format!("{}: expected nothing, got {seen:?}", case.id);
            }
            String::new()
        }
        Expect::All(expected) => {
            let wanted: Vec<(u32, char, String)> = expected
                .iter()
                .map(|(line, kind, message)| (*line, *kind, expected_message(case, message)))
                .collect();
            let seen: Vec<(u32, char, String)> = got
                .iter()
                .map(|d| (d.line, d.kind, d.message.clone()))
                .collect();
            if seen != wanted {
                return format!("{}: expected {wanted:?}, got {seen:?}", case.id);
            }
            String::new()
        }
        Expect::First(line, kind, message, count) => {
            let Some(found) = got.first() else {
                return format!("{}: expected {message}, got nothing", case.id);
            };
            let message = expected_message(case, message);
            if (found.line, found.kind, found.message.as_str()) != (line, kind, message.as_str()) {
                return format!(
                    "{} first line: expected {:?}, got {:?}",
                    case.id,
                    (line, kind, &message),
                    (found.line, found.kind, &found.message)
                );
            }
            if got.len() != count {
                return format!(
                    "{}: expected {count} lines, got {}: {:?}",
                    case.id,
                    got.len(),
                    seen.iter().map(|d| (d.2, d.1)).collect::<Vec<_>>()
                );
            }
            String::new()
        }
        Expect::Crash => match seen.as_slice() {
            [(_, _, _)] if got[0].crash => String::new(),
            _ => format!("{}: expected one crash line, got {seen:?}", case.id),
        },
    }
}

fn check_all(cases: &[Case]) {
    let failures: Vec<String> = cases.iter().map(summary).filter(|s| !s.is_empty()).collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// What the lexer and parser alone report. A case about the shape of an
// expression says nothing about whether its names resolve, so these run the
// parsing only.
fn summary_of_parse(case: &Case) -> String {
    let mut seen = Vec::new();
    for (path, layer, text) in case.files {
        let file = SourceFile {
            path: (*path).into(),
            layer: *layer,
            mod_index: 1,
            text: (*text).into(),
        };
        let lexed = enforce_check::preproc::lex(&file, &[]);
        let parsed = enforce_check::parser::parse(&file, &lexed.tokens);
        for d in lexed.diags.iter().chain(&parsed.diags) {
            seen.push((d.line, format!("{:?}", d.severity), d.message.clone()));
        }
    }
    if seen.is_empty() {
        String::new()
    } else {
        format!("{}: expected nothing, got {seen:?}", case.id)
    }
}

fn parse_all(cases: &[Case]) {
    let failures: Vec<String> = cases.iter().map(summary_of_parse).filter(|s| !s.is_empty()).collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

const OPENED: &str = "Opened scope at the end of file, missing '}' ?";
const BROKEN: &str = "Broken expression (missing ';'?)";
const MISSING_SEMI: &str = "Missing ';' at the end of line";

#[test]
fn number_expressions_follow_the_scan_rules() {
    check_all(&[
        Case {
            id: "syn-003",
            files: game("class A { void F() { int x = 1; int y = x -1; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-004",
            files: game("class A { void F() { int x = 1; int y = x-1; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-005",
            files: game("class PrA { void F() { int x = 1; int y = x - 1; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-006",
            files: game("class A { void F() { int x = 5f; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-008",
            files: game("class A { void F() { float f = 5.; } }"),
            expected: all(&[(1, 'E', "Syntax error"), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-009",
            files: game("class PrA { void F() { float f = 1e3; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-010",
            files: game("class A { void F() { float f = 1E3; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-011",
            files: game("class PrA { void F() { int x = 0x1F; } }"),
            expected: Expect::Nothing,
        },
    ]);
}

#[test]
fn strings_and_junk_words() {
    check_all(&[
        Case {
            id: "syn-012",
            files: game("class A { void F() { string s = 'a'; } }"),
            expected: all(&[
                (1, 'E', BROKEN),
                (1, 'E', BROKEN),
                (1, 'E', OPENED),
                (1, 'E', OPENED),
            ]),
        },
        Case {
            id: "syn-013",
            files: game("class A { void F() { string s = \"abc; } }"),
            expected: all(&[
                (
                    1,
                    'E',
                    "CParser: quoted string not closed on line 1, module @FILE@",
                ),
                (1, 'W', MISSING_SEMI),
                (1, 'W', BROKEN),
                (1, 'E', OPENED),
                (1, 'E', OPENED),
            ]),
        },
        Case {
            id: "syn-014",
            files: game("class PrA { void F() { string s = \"a\\qb\"; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-040",
            files: game("class PrA { void F() { int x = (int)1; } }"),
            expected: Expect::Nothing,
        },
    ]);
}

#[test]
fn operators_that_the_game_parses_differently() {
    check_all(&[
        Case {
            id: "syn-015",
            files: game("class A { void F() { int x = true ? 1 : 2; } }"),
            expected: all(&[
                (1, 'E', BROKEN),
                (1, 'E', BROKEN),
                (1, 'W', BROKEN),
                (1, 'E', OPENED),
                (1, 'E', OPENED),
            ]),
        },
        Case {
            id: "syn-018",
            files: game("class A { void F() { int a; if (a = 1) {} } }"),
            expected: all(&[
                (1, 'E', "Assign operator '=' not allowed here"),
                (1, 'E', "Expected ')', not a '='"),
                (1, 'E', OPENED),
                (1, 'E', OPENED),
            ]),
        },
        Case {
            id: "syn-019",
            files: game("class A { void F() { int b; int a = b = 1; } }"),
            expected: all(&[
                (1, 'E', "Assign operator '=' not allowed here"),
                (1, 'W', BROKEN),
                (1, 'E', OPENED),
                (1, 'E', OPENED),
            ]),
        },
        Case {
            id: "syn-020",
            files: game("class A { void F() { int local; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-021",
            files: game("class A { void F() { bool debug; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-042",
            files: game("class PrA { void F() { int x = (1 + 2; } }"),
            expected: all(&[(1, 'F', "Unmatched brackets detected!")]),
        },
        Case {
            id: "syn-047",
            files: game("class A { void F() { return 1 2; } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
    ]);
}

#[test]
fn statements_and_layout() {
    check_all(&[
        Case {
            id: "syn-025",
            files: game("class PrA { void F() { int a = 1 \n int b = 2; } }"),
            expected: all(&[(1, 'W', MISSING_SEMI), (1, 'W', BROKEN)]),
        },
        Case {
            id: "syn-026",
            files: game("class PrA { void F() { if (true); } }"),
            expected: all(&[(1, 'W', "condition with no body?")]),
        },
        Case {
            id: "syn-028",
            files: game("class PrA { void F() int x; }"),
            expected: all(&[(1, 'F', "Missing function scope")]),
        },
        Case {
            id: "syn-029",
            files: game("class PrA { void F() { } } \n }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-030",
            files: game("class A { void F() { }"),
            expected: all(&[(1, 'E', OPENED)]),
        },
        Case {
            id: "syn-032",
            files: game("{"),
            expected: all(&[(1, 'E', "Unexpected scope")]),
        },
        Case {
            id: "syn-033",
            files: game("class A { { } }"),
            expected: all(&[(1, 'E', "Unexpected scope")]),
        },
        Case {
            id: "syn-034",
            files: game("class A { void F() { void B::G() {} } }"),
            expected: all(&[
                (1, 'E', "Function definition inside block. (missing '}' ?)"),
                (1, 'E', OPENED),
            ]),
        },
        // The second line, `Can't find class void`, is pass 2.
        Case {
            id: "syn-035",
            files: game("class PrA { void F() { void G() {} } }"),
            // The second line is the body's: `Can't find class void`.
            expected: first(1, 'W', BROKEN, 2),
        },
        Case {
            id: "syn-036",
            files: game("class A { void F() { class B {} } }"),
            expected: all(&[(1, 'E', BROKEN), (1, 'E', OPENED), (1, 'E', OPENED)]),
        },
        Case {
            id: "syn-037",
            files: game("class A { typedef int X; }"),
            expected: all(&[(1, 'E', "Typedef definition inside block. (missing '}' ?)")]),
        },
        Case {
            id: "syn-038",
            files: game("class PrA { int x; \n ? \n int y; }"),
            expected: all(&[(2, 'W', MISSING_SEMI)]),
        },
        Case {
            id: "syn-043",
            files: game("class PrA { void G(int a, int b) {} void F() { G(1, \n 2); } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-044",
            files: game("enum PrE { PrA, PrB \n C }"),
            expected: Expect::Nothing,
        },
    ]);
}

#[test]
fn declarations_and_names() {
    check_all(&[
        Case {
            id: "syn-027",
            files: game("class A;"),
            expected: all(&[(1, 'W', "Forward declarations are deprecated!")]),
        },
        Case {
            id: "syn-039",
            files: game("[Foo] \n class A {}"),
            expected: all(&[(1, 'E', "Expected attribute call")]),
        },
        Case {
            id: "syn-045",
            files: game("enum PrE { PrA; PrB }"),
            expected: all(&[(1, 'F', "Expected ',' or '}', not a ';'")]),
        },
        Case {
            id: "syn-046",
            files: game("class PrB {} class PrA : ref PrB {}"),
            expected: all(&[(1, 'F', "Unexpected statement 'ref'")]),
        },
        Case {
            id: "syn-053",
            files: game("class local {}"),
            expected: all(&[(1, 'F', "Invalid identifier 'local'")]),
        },
        Case {
            id: "syn-054",
            files: game("class Cast {}"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-055",
            files: game("class PrA { void local() {} }"),
            expected: all(&[(1, 'F', "Invalid identifier 'local'")]),
        },
    ]);
}

#[test]
fn errors_that_end_a_module_level_parse() {
    check_all(&[
        Case {
            id: "syn-056",
            files: game(
                "int ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\n\
                 int ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\nint ;\n",
            ),
            expected: all(&[(1, 'E', "Syntax error")]),
        },
        Case {
            id: "syn-062",
            files: game("#if 1 \n #endif"),
            expected: all(&[(1, 'E', "Syntax error")]),
        },
        Case {
            id: "syn-067",
            files: game("\u{feff}class A {}"),
            expected: all(&[(1, 'E', "Syntax error")]),
        },
    ]);
}

#[test]
fn preprocessor_regions() {
    check_all(&[
        Case {
            id: "syn-058",
            files: game("#ifdef NOTDEFINED \n int x = ; \n #endif"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-059",
            files: game("#ifdef"),
            expected: all(&[(1, 'E', "CParser: #if[n]def expected an identifier")]),
        },
        Case {
            id: "syn-060",
            files: game("#endif"),
            expected: all(&[(1, 'W', MISSING_SEMI)]),
        },
        Case {
            id: "syn-061",
            files: game("#else"),
            expected: all(&[(1, 'W', MISSING_SEMI)]),
        },
        Case {
            id: "syn-063",
            files: game("#ifdef NOTDEFINED \n /* \n #endif \n int x = ;"),
            expected: Expect::Nothing,
        },
        Case {
            id: "syn-064",
            files: game("#define X 1 \n class A { int y = X; }"),
            expected: Expect::Crash,
        },
        Case {
            id: "syn-065",
            files: game("#define X 1 \n class A { string s = \"X\"; }"),
            expected: all(&[
                (1, 'E', "Expected 'class', not a '1'"),
                (1, 'F', "Invalid identifier '1'"),
            ]),
        },
    ]);
}

#[test]
fn a_scope_left_open_is_reported_at_the_last_line_of_its_own_file() {
    check_all(&[Case {
        id: "syn-031",
        files: &[
            ("Scripts/3_Game/a.c", Layer::Game, "class A { void F() {"),
            ("Scripts/3_Game/b.c", Layer::Game, "class B {}"),
        ],
        expected: all(&[(1, 'E', OPENED), (1, 'E', OPENED)]),
    }]);
}

fn parse_text(text: &str) {
    let file = SourceFile {
        path: GAME.into(),
        layer: Layer::Game,
        mod_index: 1,
        text: text.into(),
    };
    let lexed = enforce_check::preproc::lex(&file, &[]);
    let _ = enforce_check::parser::parse(&file, &lexed.tokens);
}

#[test]
fn any_token_stream_is_parsed_without_panicking() {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "ab01_{}[]()<>;:,.=+-*/%!&|^~?#\"'\\\n\r\t class void enum typedef if for \
                              foreach while switch case return new ref modded sealed int float"
        .chars()
        .collect();
    for _ in 0..400 {
        let len = (next() % 300) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        parse_text(&text);
    }

    // Every short sequence of the words that open and close things.
    let words = ["]", ")", "}", "{", "(", "[", ";", "x", "class", "if", "=", "\n"];
    let mut texts = vec![String::new()];
    for _ in 0..4 {
        texts = texts
            .iter()
            .flat_map(|prefix| words.iter().map(move |word| format!("{prefix} {word}")))
            .collect();
        for text in &texts {
            parse_text(text);
            parse_text(&format!("class A {{ void F() {{ {text}"));
        }
    }

    let fixture = include_str!("../../../tests/fixtures/preproc/mixed.c");
    let chars: Vec<char> = fixture.chars().collect();
    for end in 0..=chars.len() {
        let prefix: String = chars[..end].iter().collect();
        parse_text(&prefix);
    }
}

#[test]
fn an_expression_error_still_reports_the_lines_after_it() {
    check_all(&[
        Case {
            id: "syn-001",
            files: game("class A { void F() { int x = 1 + \n 2; } }"),
            expected: all(&[
                (1, 'E', "Syntax error"),
                (1, 'W', MISSING_SEMI),
                (1, 'W', BROKEN),
            ]),
        },
        Case {
            id: "syn-002",
            files: game("class A { void F() { if (true && \n true) {} } }"),
            expected: all(&[
                (1, 'E', "Syntax error"),
                (1, 'W', MISSING_SEMI),
                (1, 'E', "Expected ')', not a '&&'"),
                (2, 'E', "Invalid statement ')'"),
                (2, 'E', OPENED),
            ]),
        },
        // The third line, `Can't find variable 'int'`, is pass 2.
        Case {
            id: "syn-041",
            files: game("class PrA { void F() { int x = (int) \n ; } }"),
            // The third line is the body's: `Can't find variable 'int'`.
            expected: first(1, 'W', MISSING_SEMI, 3),
        },
    ]);
}

#[test]
fn scripts_that_compile_in_the_game_parse_clean() {
    parse_all(&[
        Case {
            id: "string-colon-case",
            files: game("class A { void F(string s) { switch (s) { case \":\": break; case \";\": break; } } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "modded-enum",
            files: game("enum E\n{\n\tA,\n\tB\n}\n\nmodded enum E\n{\n\tC\n}\n"),
            expected: Expect::Nothing,
        },
        Case {
            id: "attribute-then-modded-enum",
            files: game("enum E { A }\n[Attr()] modded enum E { B }\n"),
            expected: Expect::Nothing,
        },
        Case {
            id: "one-line-if-else",
            files: game("class A { void F(int x) { if (x) G(); else H(); if (x) a = 1; else b = 2; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "call-closed-on-the-next-line",
            files: game("class A { void F() {\n f(\n a,\n b\n);\n g(\n a,\n new T(\"x\", 1)\n);\n} }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "cast-of-a-list",
            files: game("class A { void F() { G((TStringArray){\"a\", \"b\"}); } }"),
            expected: Expect::Nothing,
        },
    ]);
}

#[test]
fn a_long_else_if_ladder_parses_clean() {
    // Short of the length the game itself gives up at.
    let ladder = "if (x == 0) {}\n".to_string() + &"else if (x == 1) {}\n".repeat(120);
    let text: &'static str = Box::leak(format!("class A {{\nvoid F(int x)\n{{\n{ladder}}}\n}}\n").into_boxed_str());
    check_all(&[Case {
        id: "else-if-ladder",
        files: game(text),
        expected: Expect::Nothing,
    }]);
}

#[test]
fn text_that_spells_punctuation_is_an_operand() {
    parse_all(&[
        Case {
            id: "string-brace-in-list",
            files: game("class A { void F() { array<string> a = {\"a\", \"}\"}; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "string-bracket-index",
            files: game("class A { void F() { string x = b[\"]\"]; } }"),
            expected: Expect::Nothing,
        },
        Case {
            id: "string-paren-group",
            files: game("class A { void F() { string x = (\")\" + \"a\"); } }"),
            expected: Expect::Nothing,
        },
    ]);
}

#[test]
fn a_missing_operand_before_a_closer_is_a_syntax_error() {
    check_all(&[Case {
        id: "ord-009",
        files: game("class ProbeY\n{\n\tvoid F()\n\t{\n\t\tint x = 1;\n\t\tx = x +;\n\t}\n}\n"),
        expected: all(&[(6, 'E', "Syntax error")]),
    }]);
}

// A class body that fails leaves the level; the body's `{` is then read again
// at module level and reported on its own line.
#[test]
fn a_failed_class_body_is_read_again_at_module_level() {
    check_all(&[
        Case {
            id: "decl-035",
            files: game("class ProbeV7\n{\n\tint new;\n}\n"),
            expected: all(&[(1, 'E', "Syntax error"), (2, 'E', "Unexpected scope")]),
        },
        Case {
            id: "decl-036",
            files: game("class ProbeV7b\n{\n\tint Friend;\n}\n"),
            expected: all(&[(1, 'E', "Syntax error"), (2, 'E', "Unexpected scope")]),
        },
        Case {
            id: "decl-037",
            files: game("class ProbeV7c\n{\n\tint Cast;\n}\n"),
            expected: all(&[(1, 'E', "Syntax error"), (2, 'E', "Unexpected scope")]),
        },
        Case {
            id: "decl-076",
            files: game("[Obsolete]\nvoid ProbeObs()\n{\n}\n"),
            expected: all(&[
                (1, 'E', "Expected attribute call"),
                (3, 'E', "Unexpected scope"),
            ]),
        },
    ]);
}

// Only the layout the game was observed on is read again: an own-line brace
// below the class, or an attribute list that fills its line.
#[test]
fn recovery_stays_to_the_observed_layouts() {
    check_all(&[
        Case {
            id: "same-line brace",
            files: game("class ProbeBrace {\n\tint new;\n}\n"),
            expected: all(&[(1, 'E', "Syntax error")]),
        },
        Case {
            id: "attribute before an array declaration",
            files: game("[Obsolete] int a[2]\nint z z;\nint q q;\n"),
            expected: all(&[(1, 'E', "Expected attribute call"), (2, 'E', "Syntax error")]),
        },
    ]);
}

#[test]
fn two_files_in_nested_folders_parse_clean() {
    check_all(&[Case {
        id: "syn-068",
        files: &[
            ("Scripts/3_Game/a.c", Layer::Game, "class PrA {}"),
            ("Scripts/3_Game/Sub/c.c", Layer::Game, "class PrB {}"),
        ],
        expected: Expect::Nothing,
    }]);
}

#[test]
fn deep_nesting_ends_the_parse_instead_of_overflowing_the_stack() {
    const N: usize = 1000;
    let in_body = |tail: String| format!("class A {{\n void F() {{\n{tail}");
    let shapes = [
        in_body(format!("x = {}", "(".repeat(N))),
        in_body("{\n".repeat(N)),
        in_body(format!("x = {}", "a[".repeat(N))),
        in_body(format!("x = {}", "f(".repeat(N))),
        in_body(format!("x = {}", "new A(".repeat(N))),
        in_body("if (x) ".repeat(N)),
        in_body(format!("A{} x;", "<A".repeat(N))),
        "class A {\n".repeat(N),
        in_body(format!("x = 1{};", "+1".repeat(100_000))),
        in_body(format!("x = a{};", ".b".repeat(100_000))),
        in_body(format!("x = a{};", "[0]".repeat(100_000))),
        in_body(format!("x = a{};", "()".repeat(100_000))),
        in_body(format!("x = {}a;", "!".repeat(100_000))),
        in_body("if (x) {}\n".to_string() + &"else if (x) {}\n".repeat(100_000)),
        in_body(format!(
            "x = {}1{};",
            format!("({}", "1+".repeat(400)).repeat(90),
            ")".repeat(90)
        )),
    ];
    let thread = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            for text in &shapes {
                let file = SourceFile {
                    path: GAME.into(),
                    layer: Layer::Game,
                    mod_index: 1,
                    text: text.clone(),
                };
                let lexed = enforce_check::preproc::lex(&file, &[]);
                let parsed = enforce_check::parser::parse(&file, &lexed.tokens);
                if text.len() < 20_000 {
                    assert!(
                        parsed.diags.iter().any(|d| d.message == "Too many errors"),
                        "{}",
                        &text[..text.len().min(40)]
                    );
                }
            }
        })
        .unwrap();
    thread.join().unwrap();
}

#[test]
fn a_stray_closer_never_stalls_the_parse() {
    let texts = ["]", ")", "] x", "class A {\n]\n}", "class A {\n)\n}", "class A { void F() {\n]\n}}"];
    for text in texts {
        let (done, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            parse_text(text);
            let _ = done.send(());
        });
        assert!(
            wait.recv_timeout(std::time::Duration::from_secs(5)).is_ok(),
            "{text:?}"
        );
    }
}

fn ast_of(text: &str) -> enforce_check::parser::FileAst {
    let file = SourceFile {
        path: GAME.into(),
        layer: Layer::Game,
        mod_index: 1,
        text: text.into(),
    };
    let lexed = enforce_check::preproc::lex(&file, &[]);
    enforce_check::parser::parse(&file, &lexed.tokens).ast
}

#[test]
fn the_tree_carries_the_nodes_the_game_has() {
    use enforce_check::parser::{ExprKind, Item};

    let ast = ast_of(
        "modded class PrA : PrB\n\
         {\n\
         \tint m_Value;\n\
         \tstatic proto void F(int a, string b);\n\
         \tvoid G() { if (a) { b = 1 + 2; } for (int i = 0; i < 3; i++) { c[i].D(); } }\n\
         }\n\
         enum PrE { One, Two }\n\
         typedef int PrInt;\n\
         [Attribute()]\n\
         void H() { foreach (int x : list) { x++; } switch (v) { case 1: break; } }\n",
    );

    let class = match &ast.items[0] {
        Item::Class(class) => class,
        other => panic!("{other:?}"),
    };
    assert_eq!(class.name, "PrA");
    assert_eq!(class.line, 1);
    assert_eq!(class.modifiers, ["modded"]);
    assert_eq!(class.base.as_ref().map(|base| base.name.as_str()), Some("PrB"));

    let field = match &class.members[0] {
        Item::Var(field) => field,
        other => panic!("{other:?}"),
    };
    assert_eq!(field.name, "m_Value");
    assert_eq!(field.type_.as_ref().map(|t| t.text()).as_deref(), Some("int"));

    let function = match &class.members[1] {
        Item::Function(function) => function,
        other => panic!("{other:?}"),
    };
    assert_eq!(function.name, "F");
    assert!(function.prototype);
    assert_eq!(function.params.len(), 2);
    assert_eq!(function.params[1].name, "b");
    assert!(function.modifiers.contains(&"proto".to_string()));

    let method = match &class.members[2] {
        Item::Function(method) => method,
        other => panic!("{other:?}"),
    };
    let body = method.body.as_ref().expect("body");
    match &body.items[0] {
        Item::Stmt(enforce_check::parser::Stmt::If { then, .. }) => {
            let Some(then) = then else { panic!("no then") };
            match &**then {
                Item::Block(block) => match &block.items[0] {
                    Item::Stmt(enforce_check::parser::Stmt::Expr { expr, .. }) => {
                        assert!(matches!(expr.kind, ExprKind::Binary { .. }));
                    }
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }

    let enumeration = ast
        .items
        .iter()
        .find_map(|item| match item {
            Item::Enum(enumeration) => Some(enumeration),
            _ => None,
        })
        .expect("enum");
    assert_eq!(enumeration.name, "PrE");
    assert_eq!(enumeration.members.len(), 2);

    assert!(ast
        .items
        .iter()
        .any(|item| matches!(item, Item::Typedef(typedef) if typedef.name == "PrInt")));

    let attributed = ast
        .items
        .iter()
        .find_map(|item| match item {
            Item::Function(function) if !function.attributes.is_empty() => Some(function),
            _ => None,
        })
        .expect("attributed function");
    assert_eq!(attributed.attributes.len(), 1);
    let body = attributed.body.as_ref().expect("body");
    assert!(matches!(
        &body.items[0],
        Item::Stmt(enforce_check::parser::Stmt::Foreach { .. })
    ));
    assert!(matches!(
        &body.items[1],
        Item::Stmt(enforce_check::parser::Stmt::Switch { .. })
    ));
}

// A postfix step binds tighter than any binary operator, so `a.b + c.d` is a sum
// of two members and `0 + i.Length()` is a sum with the call on the right.
#[test]
fn a_member_access_stays_on_its_own_operand() {
    use enforce_check::parser::{Expr, ExprKind, Item, Stmt};

    fn expr(text: &str) -> Expr {
        let ast = ast_of(&format!("void F() {{ {text} }}"));
        let Item::Function(function) = &ast.items[0] else {
            panic!("no function")
        };
        let body = function.body.as_ref().expect("body");
        let Item::Stmt(Stmt::Expr { expr, .. }) = &body.items[0] else {
            panic!("no statement")
        };
        expr.clone()
    }

    fn shape(expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Leaf { text } => text.clone(),
            ExprKind::Member { object, name } => format!("{}.{name}", shape(object)),
            ExprKind::Index { object, .. } => format!("{}[]", shape(object)),
            ExprKind::Call { callee, .. } => format!("{}()", shape(callee)),
            ExprKind::Binary { op, left, right } => format!(
                "({} {op} {})",
                shape(left),
                right.as_deref().map(shape).unwrap_or_default()
            ),
            other => format!("{other:?}"),
        }
    }

    let cases = [
        ("a.b + c.d;", "(a.b + c.d)"),
        ("a.b + c;", "(a.b + c)"),
        ("a + b.c;", "(a + b.c)"),
        ("a[0] + b[1];", "(a[] + b[])"),
        ("a.F() + b.G();", "(a.F() + b.G())"),
        ("a.b.c + d;", "(a.b.c + d)"),
        ("a.b && c.d;", "(a.b && c.d)"),
        ("a.b = c.d;", "(a.b = c.d)"),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got = shape(&expr(text));
        if got != expected {
            failures.push(format!("{text}: got {got}, want {expected}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A `for` header may declare several names of one type; the game keeps them all
// as locals of the function.
#[test]
fn a_for_header_declares_every_name_it_lists() {
    use enforce_check::parser::{Item, Stmt};

    let ast = ast_of("void F(int n)\n{\n\tfor (int i = 0, count = n; i < count; ++i)\n\t{\n\t}\n}\n");
    let Item::Function(function) = &ast.items[0] else {
        panic!("no function")
    };
    let body = function.body.as_ref().expect("body");
    let Item::Stmt(Stmt::For { init, .. }) = &body.items[0] else {
        panic!("no for")
    };
    let init = init.as_deref().expect("init");
    let declared: Vec<&str> = match init {
        Item::Block(block) => block
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Var(var) => Some(var.name.as_str()),
                _ => None,
            })
            .collect(),
        Item::Var(var) => vec![var.name.as_str()],
        other => panic!("{other:?}"),
    };
    assert_eq!(declared, ["i", "count"]);
}

// The precedence table: a smaller level binds tighter. The
// tree must group the tighter operators inside the looser ones.
#[test]
fn a_binary_tree_groups_by_the_precedence_table() {
    use enforce_check::parser::{Expr, ExprKind, Item, Stmt};

    fn expr(text: &str) -> Expr {
        let ast = ast_of(&format!("void F() {{ {text} }}"));
        let Item::Function(function) = &ast.items[0] else {
            panic!("no function")
        };
        let body = function.body.as_ref().expect("body");
        let Item::Stmt(Stmt::Expr { expr, .. }) = &body.items[0] else {
            panic!("no statement")
        };
        expr.clone()
    }

    fn shape(node: &Expr) -> String {
        match &node.kind {
            ExprKind::Leaf { text } => text.clone(),
            ExprKind::Member { object, name } => format!("{}.{name}", shape(object)),
            ExprKind::Index { object, .. } => format!("{}[]", shape(object)),
            ExprKind::Call { callee, .. } => format!("{}()", shape(callee)),
            ExprKind::Unary { op, operand, .. } => match operand {
                Some(operand) => format!("({op}{})", shape(operand)),
                None => op.clone(),
            },
            ExprKind::Binary { op, left, right } => format!(
                "({} {op} {})",
                shape(left),
                right.as_deref().map(shape).unwrap_or_default()
            ),
            other => format!("{other:?}"),
        }
    }

    let cases = [
        // Assignment is the loosest level, so it wraps the whole sum.
        ("x = a + b * c;", "(x = (a + (b * c)))"),
        // `*` binds tighter than `+`.
        ("a + b * c;", "(a + (b * c))"),
        ("a * b + c;", "((a * b) + c)"),
        // `||` is looser than `>` and `!=`.
        ("i > j || t != bool;", "((i > j) || (t != bool))"),
        // `&&` binds tighter than `||`.
        ("a && b || c;", "((a && b) || c)"),
        // `<<` binds looser than `+`.
        // A `<` after a name opens a template list in this grammar, so the
        // looser `<<` and `==` levels are checked on the sides that do parse.
        ("aa + bb << cc;", "((aa + bb) << cc)"),
        ("(aa < bb) == cc;", "((aa < bb) == cc)"),
        // Left-associative at one level.
        ("a - b - c;", "((a - b) - c)"),
        ("a = b = 1;", "((a = b) = 1)"),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got = shape(&expr(text));
        if got != expected {
            failures.push(format!("{text}: got {got}, want {expected}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A nested list closes itself only: the arguments written after it belong to
// the list around it.
#[test]
fn a_nested_template_list_leaves_the_arguments_after_it_to_the_outer_list() {
    use enforce_check::parser::Item;

    let ast = ast_of(
        "class PrU\n{\n\tPrP<int, ref array<string>, float> m_A;\n\tmap<string, ref array<ref set<int>>> m_B;\n\tPrP<array<int>, int> m_C;\n}\n",
    );
    let Item::Class(class) = &ast.items[0] else {
        panic!("{:?}", ast.items[0]);
    };
    let types: Vec<String> = class
        .members
        .iter()
        .filter_map(|member| match member {
            Item::Var(var) => var.type_.as_ref().map(|type_| type_.text()),
            _ => None,
        })
        .collect();
    assert_eq!(
        types,
        ["PrP<int,array<string>,float>", "map<string,array<set<int>>>", "PrP<array<int>,int>"]
    );
}
