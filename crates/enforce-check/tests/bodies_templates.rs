mod support;

use enforce_check::Target;
use support::{Row, assert_rows, row, text};

const NOPE: &str = "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n";
const UNDEFINED: &str = "Undefined function 'PrT<int>.Nope'";
const WHILE_INT: &str = "...while compiling template class 'PrT<int>'";

// The body of a template class is compiled once for each set of arguments it
// is used with, and every line it prints sits on the class's closing line.
fn instantiation_rows() -> Vec<Row> {
    vec![
        row(
            "tpl-01",
            "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t\tint x = nope;\n\t}\n}\n",
            &[],
        ),
        row(
            "tpl-02",
            text(&[NOPE, "void F()\n{\n\tPrT<int> t;\n}\n"]),
            &[(7, UNDEFINED), (7, WHILE_INT), (10, "Bad type 'PrT'")],
        ),
        row(
            "tpl-03",
            text(&[NOPE, "void F()\n{\n\tPrT<int> t = new PrT<int>();\n\tt.M();\n}\n"]),
            &[(7, UNDEFINED), (7, WHILE_INT), (10, "Bad type 'PrT'")],
        ),
        row("tpl-04", text(&[NOPE, "class PrU : PrT<int>\n{\n}\n"]), &[(7, UNDEFINED), (7, WHILE_INT)]),
        row(
            "tpl-05",
            text(&[NOPE, "class PrU\n{\n\tref PrT<int> m_T;\n}\n"]),
            &[(7, UNDEFINED), (7, WHILE_INT)],
        ),
        row(
            "tpl-06",
            "class PrT<Class T>\n{\n\tvoid M(T v)\n\t{\n\t\tv.Nope();\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(7, "Undefined function 'int.Nope'"), (7, WHILE_INT)],
        ),
        row(
            "tpl-07",
            "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tint x = nope;\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_A;\n\tref PrT<string> m_B;\n}\n",
            &[
                (7, "Can't find variable 'nope'"),
                (7, WHILE_INT),
                (7, "Can't find variable 'nope'"),
                (7, "...while compiling template class 'PrT<string>'"),
            ],
        ),
        row(
            "tpl-08",
            "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tint x = nope;\n\t\tint y = \"a\";\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(8, "Can't find variable 'nope'"), (8, "Incompatible parameter 'y'"), (8, WHILE_INT)],
        ),
        row(
            "tpl-09",
            "class PrT<Class T>\n{\n\tint M(T v)\n\t{\n\t\treturn v.Length();\n\t}\n}\nclass PrU\n{\n\tref PrT<string> m_A;\n\tref PrT<int> m_B;\n}\n",
            &[(7, "Undefined function 'int.Length'"), (7, WHILE_INT)],
        ),
        row(
            "tpl-10",
            "class PrU : array<int>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n",
            &[(5, "Undefined function 'PrU.Nope'")],
        ),
        row(
            "tpl-20",
            "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n\tint m_After;\n\tvoid N()\n\t{\n\t}\n\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(12, UNDEFINED), (12, WHILE_INT)],
        ),
        // A typedef is read before the functions, whatever the order they are written in.
        row(
            "tpl-22",
            text(&[NOPE, "void F(PrT<int> t)\n{\n}\nPrT<string> G()\n{\n\treturn null;\n}\ntypedef PrT<float> PrTF;\n"]),
            &[
                (7, "Undefined function 'PrT<float>.Nope'"),
                (7, "...while compiling template class 'PrT<float>'"),
                (7, UNDEFINED),
                (7, WHILE_INT),
                (7, "Undefined function 'PrT<string>.Nope'"),
                (7, "...while compiling template class 'PrT<string>'"),
            ],
        ),
        row(
            "tpl-24",
            text(&[NOPE, "class PrU\n{\n\tref array<ref PrT<int>> m_T;\n}\n"]),
            &[(7, UNDEFINED), (7, WHILE_INT)],
        ),
        row(
            "tpl-27",
            "class PrT<Class T>\n{\n\tint m_X = nope;\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(4, "Can't find variable 'nope'"), (4, WHILE_INT)],
        ),
        row(
            "tpl-28",
            "class PrT<Class T>\n{\n\tint M()\n\t{\n\t\tint x = 1;\n\t}\n\tvoid N()\n\t{\n\t\tNope();\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[
                (11, "No return statement in function returning non-void 'PrT<int>.M'"),
                (11, UNDEFINED),
                (11, WHILE_INT),
            ],
        ),
        // An instance is compiled once; a later name for it prints nothing more.
        row(
            "tpl-29",
            text(&[NOPE, "class PrU\n{\n\tref PrT<int> m_A;\n\tref PrT<int> m_B;\n}\nvoid F()\n{\n\tPrT<int> t;\n}\n"]),
            &[(7, UNDEFINED), (7, WHILE_INT)],
        ),
        row(
            "tpl-30",
            "class PrA\n{\n}\nclass PrB : PrA\n{\n}\nclass PrT<Class T>\n{\n\tvoid M(PrA a)\n\t{\n\t\tPrB b = a;\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(13, "FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting")],
        ),
        // A local's instance is compiled where the local is declared.
        row(
            "tpl-33",
            text(&[NOPE, "void F()\n{\n\tint a = nopeA;\n\tPrT<int> t;\n\tint b = nopeB;\n}\n"]),
            &[
                (10, "Can't find variable 'nopeA'"),
                (7, UNDEFINED),
                (7, WHILE_INT),
                (11, "Bad type 'PrT'"),
                (12, "Can't find variable 'nopeB'"),
            ],
        ),
        // A member's instance is compiled before any function body.
        row(
            "tpl-34",
            "void F()\n{\n\tint a = nopeA;\n}\nclass PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\nclass PrU\n{\n\tref PrT<int> m_T;\n}\n",
            &[(11, UNDEFINED), (11, WHILE_INT), (3, "Can't find variable 'nopeA'")],
        ),
        row(
            "tpl-35",
            "class PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tT v;\n\t\tv.Nope();\n\t}\n}\nclass PrU\n{\n\tref PrT<string> m_T;\n}\n",
            &[(8, "Undefined function 'string.Nope'"), (8, "...while compiling template class 'PrT<string>'")],
        ),
    ]
}

#[test]
fn a_template_body_is_checked_for_each_set_of_arguments_it_is_used_with() {
    assert_rows(Target::RetailServer, &instantiation_rows());
}

// Expansion's bit stream writer: aliases of one container with different
// arguments are different types to an overload pick.
#[test]
fn an_overload_on_container_aliases_takes_the_exact_one() {
    let text = "typedef array<bool> PrBools;\ntypedef array<int> PrInts;\ntypedef array<float> PrFloats;\n\
        class PrS\n{\n\tvoid Write(PrBools v)\n\t{\n\t}\n\tvoid Write(PrInts v)\n\t{\n\t}\n\tvoid Write(PrFloats v)\n\t{\n\t}\n}\n\
        class PrW<Class T>\n{\n\tT m_S;\n\tvoid Put(PrInts values)\n\t{\n\t\tm_S.Write(values);\n\t}\n}\n\
        class PrU\n{\n\tref PrW<PrS> m_W;\n}\n";
    assert_rows(Target::RetailServer, &[row("container-alias-overloads", text, &[])]);
}

// No input may abort the checker: a typedef that names itself, and a template
// that names itself with ever larger arguments.
#[test]
fn a_template_that_names_itself_ends() {
    let texts = [
        "typedef array<PrZ> PrZ;\n",
        "typedef map<string, PrZ> PrZ;\nclass PrH\n{\n\tref PrZ m_c;\n}\n",
        "class PrY<Class T>\n{\n\tref T m_t;\n\tvoid M()\n\t{\n\t\tT x;\n\t}\n}\ntypedef PrY<PrZ> PrZ;\n",
        "class PrQ<Class T>\n{\n\tref PrQ<array<T>> m_a;\n\tref PrQ<map<int, T>> m_b;\n\tref PrQ<set<T>> m_c;\n}\nclass PrF\n{\n\tref PrQ<int> m_q;\n}\n",
    ];
    let chain: String = (1..40).map(|n| format!("typedef map<PrA{}, PrA{}> PrA{n};\n", n + 1, n + 1)).collect::<String>()
        + "typedef array<int> PrA40;\n";
    let texts = texts.map(String::from).into_iter().chain([chain]);
    for text in texts {
        let started = std::time::Instant::now();
        let _ = support::diagnostics(Target::RetailServer, &text);
        assert!(started.elapsed().as_secs() < 10, "{text}");
    }
}

// An instance's parameter names its argument everywhere, `T.Cast` included.
#[test]
fn a_cast_through_a_template_parameter_names_its_argument() {
    let text = "class PrCast<Class T>\n{\n\tT Conv(Class o)\n\t{\n\t\treturn T.Cast(o);\n\t}\n}\nclass PrUse\n{\n\tref PrCast<PrUse> m_a;\n}\n";
    assert_rows(Target::RetailServer, &[row("template-parameter-cast", text, &[])]);
}

// ta-01 to ta-03: an alias and the type it stands for name one instance, which
// the game prints under the type, also when the alias sits inside another
// argument.
#[test]
fn an_alias_and_its_target_name_one_instance() {
    const BAD: &str = "class PrBad<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n";
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "ta-01",
                text(&[
                    BAD,
                    "class PrUse\n{\n\tref PrBad<PrStrings> a;\n\tref PrBad<array<string>> b;\n}\ntypedef array<string> PrStrings;\n",
                ]),
                &[
                    (7, "Undefined function 'PrBad<array<string>>.Nope'"),
                    (7, "...while compiling template class 'PrBad<array<string>>'"),
                ],
            ),
            row(
                "ta-03",
                text(&[
                    BAD,
                    "class PrUse\n{\n\tref PrBad<array<PrStrings>> a;\n\tref PrBad<array<array<string>>> b;\n}\ntypedef array<string> PrStrings;\n",
                ]),
                &[
                    (7, "Undefined function 'PrBad<array<array<string>>>.Nope'"),
                    (7, "...while compiling template class 'PrBad<array<array<string>>>'"),
                ],
            ),
            row(
                "ta-02",
                text(&[BAD, "typedef PrBad<int> PrBadInt;\nclass PrUse\n{\n\tref PrBadInt a;\n\tref PrBad<int> b;\n}\n"]),
                &[(7, "Undefined function 'PrBad<int>.Nope'"), (7, "...while compiling template class 'PrBad<int>'")],
            ),
        ],
    );
}

// tk-01 to tk-03: `K++` on a const prints its line and the module still loads,
// so an instance that prints nothing else did not fail.
#[test]
fn an_increment_of_a_const_does_not_fail_a_template_instance() {
    const WRITE: &str = "Trying to modify write protected variable 'K'";
    let texts = [
        ("tk-01", "class PrT<Class T>\n{\n\tconst int K = 1;\n\tvoid M()\n\t{\n\t\tK++;\n\t}\n}\nclass PrUse\n{\n\tref PrT<int> a;\n}\n", 8),
        ("tk-02", "class PrN\n{\n\tconst int K = 1;\n\tvoid M()\n\t{\n\t\tK++;\n\t}\n}\n", 6),
        ("tk-03", "const int K = 1;\nclass PrT<Class T>\n{\n\tvoid M()\n\t{\n\t\tK++;\n\t}\n}\nclass PrUse\n{\n\tref PrT<int> a;\n}\n", 8),
    ];
    for (id, text, line) in texts {
        let result = enforce_check::check(&support::input(Target::RetailServer, &[(support::ENGINE, support::ENGINE_TEXT), (support::PROBE, text)]));
        let lines: Vec<(u32, &str)> = result.diagnostics.iter().map(|d| (d.line, d.message.as_str())).collect();
        assert_eq!(lines, vec![(line, WRITE)], "{id}");
        assert!(result.failed_modules.is_empty(), "{id}: {:?}", result.failed_modules);
    }
}

// rec-01 to rec-08: a typedef that names itself among its target's arguments,
// alone or through another typedef, is a circle. A template that names itself
// with the argument it was given is not.
#[test]
fn a_typedef_named_in_its_own_arguments_is_a_circle() {
    assert_rows(
        Target::RetailServer,
        &[
            row("rec-01", "typedef array<PrZ> PrZ;\n", &[(1, "Circle inheritance in typedef 'PrZ'")]),
            row(
                "rec-02",
                "typedef map<string, PrZ> PrZ;\nclass PrH\n{\n\tref PrZ m_c;\n}\n",
                &[(1, "Circle inheritance in typedef 'PrZ'")],
            ),
            row(
                "rec-03",
                "class PrY<Class T>\n{\n\tref T m_t;\n\tvoid M()\n\t{\n\t\tT x;\n\t}\n}\ntypedef PrY<PrZ> PrZ;\n",
                &[(9, "Circle inheritance in typedef 'PrZ'")],
            ),
            row(
                "rec-05",
                "typedef array<PrB> PrA;\ntypedef array<PrA> PrB;\n",
                &[(1, "Circle inheritance in typedef 'PrA'")],
            ),
            row("rec-06", "class PrQ<Class T>\n{\n\tref PrQ<array<T>> m_a;\n}\n", &[]),
            row("rec-08", "class PrQ<Class T>\n{\n\tref PrQ<T> m_a;\n}\nclass PrF\n{\n\tref PrQ<int> m_q;\n}\n", &[]),
        ],
    );
}

// rec-04 and rec-07: a template that names itself with a larger argument is
// compiled again and again once something uses it, until the game falls over.
#[test]
fn a_template_that_grows_with_every_instance_crashes_the_game() {
    let texts = [
        ("rec-04", "class PrQ<Class T>\n{\n\tref PrQ<array<T>> m_a;\n\tref PrQ<map<int, T>> m_b;\n\tref PrQ<set<T>> m_c;\n}\nclass PrF\n{\n\tref PrQ<int> m_q;\n}\n"),
        ("rec-07", "class PrQ<Class T>\n{\n\tref PrQ<array<T>> m_a;\n}\nclass PrF\n{\n\tref PrQ<int> m_q;\n}\n"),
    ];
    for (id, text) in texts {
        let crashes: Vec<_> = support::diagnostics(Target::RetailServer, text).into_iter().filter(|d| d.game_crash).collect();
        assert_eq!(crashes.len(), 1, "{id}: {crashes:?}");
    }
}

// tm-01 to tm-06: a template class cannot be modded. With no parameters on the
// `modded` line the game cannot inherit the original, and two of its lines sit
// where the file ends; with parameters the name stops being a type, for a
// member and for a parameter.
#[test]
fn a_modded_template_class_is_refused() {
    const BOX: &str = "class PrBox<Class T>\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n";
    const BODY: &str = "\n{\n\tvoid N()\n\t{\n\t\tNope2();\n\t}\n}\n";
    const USE: &str = "class PrUse\n{\n\tref PrBox<int> a;\n}\n";
    const FUNCTIONS: &str = "void G(PrBox<int> p)\n{\n}\nvoid F()\n{\n\tPrBox<int> a;\n}\n";
    const FEW: &str = "FIX-ME: Not enough parameters for template class 'PrBox#0#8'";
    const WRONG: &str = "Wrong number of template parameters";
    const FEW_12: &str = "FIX-ME: Not enough parameters for template class 'PrBox#0#12'";
    let bare = |last: u32| -> &'static [(u32, &'static str)] {
        Box::leak(Box::new([
            (8, FEW),
            (last, FEW),
            (last, "Wrong number of parameters"),
            (8, FEW),
            (8, WRONG),
            (8, "Can't inherit class 'PrBox'"),
        ]))
    };
    assert_rows(
        Target::RetailServer,
        &[
            row("tm-01", text(&[BOX, "modded class PrBox", BODY, USE]), bare(18)),
            row(
                "tm-02",
                text(&[BOX, "modded class PrBox<Class T>", BODY, USE]),
                &[(8, FEW), (8, WRONG), (17, "Bad type 'PrBox'"), (17, "Unknown type 'PrBox'")],
            ),
            row("tm-03", text(&[BOX, "modded class PrBox", BODY]), bare(14)),
            // The lines of the `modded` class come first wherever the use is written.
            row(
                "tm-05",
                text(&[BOX, USE, "modded class PrBox<Class T>", BODY]),
                &[(12, FEW_12), (12, WRONG), (10, "Bad type 'PrBox'"), (10, "Unknown type 'PrBox'")],
            ),
            // The two lines sit where the file ends, which is past its last class here.
            row("tm-04", text(&[BOX, "modded class PrBox", BODY, FUNCTIONS]), bare(21)),
            row(
                "tm-06",
                text(&[BOX, "modded class PrBox<Class T>", BODY, FUNCTIONS]),
                &[(8, FEW), (8, WRONG), (15, "Unknown type 'PrBox'"), (15, "Error in parameters")],
            ),
        ],
    );
    // On the Diag server the first line is an error: the other five are never
    // printed, and the name stops being a type as it does with parameters.
    const FEW_DIAG: &str = "Not enough parameters for template class 'PrBox#0#8'";
    assert_rows(
        Target::DiagServer,
        &[
            row(
                "tm-01",
                text(&[BOX, "modded class PrBox", BODY, USE]),
                &[(8, FEW_DIAG), (17, "Bad type 'PrBox'"), (17, "Unknown type 'PrBox'")],
            ),
            row("tm-03", text(&[BOX, "modded class PrBox", BODY]), &[(8, FEW_DIAG)]),
            row(
                "tm-04",
                text(&[BOX, "modded class PrBox", BODY, FUNCTIONS]),
                &[(8, FEW_DIAG), (15, "Unknown type 'PrBox'"), (15, "Error in parameters")],
            ),
        ],
    );
}

// tm-07: the two lines sit at the end of the module's last file.
#[test]
fn a_modded_template_class_points_at_the_end_of_the_last_file() {
    const FEW: &str = "FIX-ME: Not enough parameters for template class 'PrBox#0#4'";
    let result = enforce_check::check(&support::input(
        Target::RetailServer,
        &[
            (support::ENGINE, support::ENGINE_TEXT),
            ("Scripts/3_Game/a.c", "class PrBox<Class T>\n{\n}\nmodded class PrBox\n{\n}\n"),
            ("Scripts/3_Game/b.c", "class PrLast\n{\n\tint x;\n}\nclass PrUse\n{\n\tref PrBox<int> a;\n}\n"),
        ],
    ));
    let lines: Vec<(&str, u32, String)> = result
        .diagnostics
        .iter()
        .map(|d| (&d.file[d.file.len() - 3..], d.line, format!("{}{}", if d.fixme { "FIX-ME: " } else { "" }, d.message)))
        .collect();
    let expected = [
        ("a.c", 4, FEW),
        ("b.c", 8, FEW),
        ("b.c", 8, "Wrong number of parameters"),
        ("a.c", 4, FEW),
        ("a.c", 4, "Wrong number of template parameters"),
        ("a.c", 4, "Can't inherit class 'PrBox'"),
    ];
    let expected: Vec<(&str, u32, String)> = expected.iter().map(|(file, line, text)| (*file, *line, text.to_string())).collect();
    assert_eq!(lines, expected);
}

// Unobserved: a last file with no text still has a first line to point at.
#[test]
fn a_modded_template_class_before_an_empty_file_points_at_its_first_line() {
    let result = enforce_check::check(&support::input(
        Target::RetailServer,
        &[
            (support::ENGINE, support::ENGINE_TEXT),
            ("Scripts/3_Game/a.c", "class PrBox<Class T>\n{\n}\nmodded class PrBox\n{\n}\n"),
            ("Scripts/3_Game/b.c", ""),
        ],
    ));
    let at_the_end: Vec<u32> = result.diagnostics.iter().filter(|d| d.file.ends_with("b.c")).map(|d| d.line).collect();
    assert_eq!(at_the_end, [1, 1]);
}
