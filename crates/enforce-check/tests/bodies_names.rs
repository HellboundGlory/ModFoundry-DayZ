mod support;

use enforce_check::{CheckInput, ModInfo, Target, check};

// One row per probe: the probe source and every line the game printed for it, in
// order.
struct Row {
    id: &'static str,
    files: &'static [(&'static str, &'static str)],
    expected: &'static [(u32, &'static str)],
}

const PROBE: &str = "Scripts/3_Game/probe.c";

// The engine's own declarations the probes lean on: `Print` and the `map`
// container. Vanilla declares both in `1_Core`.
const ENGINE: &str = "Scripts/1_Core/proto/EnDebug.c";
const ENGINE_TEXT: &str = "class map<Class K, Class V>\n{\n}\n\nclass int\n{\n\tproto int Length();\n}\n\nproto void Print(void var);\n";

fn input(files: &[(&str, &str)]) -> CheckInput {
    support::input(Target::RetailServer, files)
}

fn lines(files: &[(&str, &str)]) -> Vec<(u32, String)> {
    let mut all: Vec<(&str, &str)> = vec![(ENGINE, ENGINE_TEXT)];
    all.extend_from_slice(files);
    check(&input(&all))
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
        .collect()
}

fn messages(text: &str) -> Vec<String> {
    check(&input(&[(PROBE, text)]))
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

// Every case compiles clean; the failures name the cases that did not.
fn assert_clean(cases: &[(&str, &str)]) {
    let mut failures = Vec::new();
    for (name, text) in cases {
        let got = check(&input(&[(PROBE, text)])).diagnostics;
        if !got.is_empty() {
            failures.push(format!("{name}: {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// A row whose probe lives in one file, which is the common shape.
fn row(
    id: &'static str,
    text: &'static str,
    expected: &'static [(u32, &'static str)],
) -> Row {
    Row {
        id,
        files: Box::leak(vec![(PROBE, text)].into_boxed_slice()),
        expected,
    }
}

fn rows() -> Vec<Row> {
    vec![
        // Names, types and function lookup.
        row("body-P01", "void F()\n{\n\tint a = b;\n}\n", &[(3, "Can't find variable 'b'")]),
        row(
            "body-P02",
            "enum E\n{\n\tA\n}\nvoid F()\n{\n\tint a = E.B;\n}\n",
            &[(7, "Can't find variable 'B'")],
        ),
        row(
            "body-P03",
            "class PrA\n{\n}\nvoid F(PrA a)\n{\n\tint x = a.nofield;\n}\n",
            &[(6, "Can't find variable 'nofield'")],
        ),
        row("body-P04", "void F()\n{\n\tNope();\n}\n", &[(3, "Undefined function 'Nope'")]),
        row(
            "body-P05",
            "class PrA\n{\n\tvoid M()\n\t{\n\t\tNope();\n\t}\n}\n",
            &[(5, "Undefined function 'PrA.Nope'")],
        ),
        row(
            "body-P06",
            "class PrA\n{\n}\nvoid F(PrA a)\n{\n\ta.Nope();\n}\n",
            &[(6, "Undefined function 'PrA.Nope'")],
        ),
        row(
            "body-P07",
            "class PrA\n{\n}\nvoid F()\n{\n\tPrA.Nope();\n}\n",
            &[(6, "Undefined function 'PrA.Nope'")],
        ),
        row("body-P08", "void F()\n{\n\tint i;\n\ti.Nope();\n}\n", &[(4, "Undefined function 'int.Nope'")]),
        row(
            "body-P09",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n\tvoid M()\n\t{\n\t\tsuper.Nope();\n\t}\n}\n",
            &[(8, "Undefined function 'PrA.Nope'")],
        ),
        // The game reads a local's type twice before it gives up on the
        // declaration, so the rows repeat.
        row(
            "body-P11",
            "void F()\n{\n\tFoo x;\n}\n",
            &[(3, "Unknown type 'Foo'"), (3, "Unknown type 'Foo'"), (3, "Error in parameters")],
        ),
        row(
            "body-P12",
            "void F()\n{\n\tref Foo x;\n}\n",
            &[
                (3, "Bad type 'Foo'"),
                (3, "Unknown type 'Foo'"),
                (3, "Bad type 'Foo'"),
                (3, "Unknown type 'Foo'"),
                (3, "Error in parameters"),
            ],
        ),
        row(
            "body-P13",
            "class PrA\n{\n}\nvoid F()\n{\n\tPrA a = new Nope();\n}\n",
            &[(6, "Bad type 'Nope'"), (6, "Can't find class Nope")],
        ),
        row(
            "body-P15",
            "class PrA\n{\n\tint x;\n}\nvoid F()\n{\n\tint y = PrA.x;\n}\n",
            &[(7, "Variable 'x' is not static")],
        ),
        row("body-P17", "void F()\n{\n\tint x = map;\n}\n", &[(3, "Can't find variable 'map'")]),
        row("body-P19", "void F()\n{\n\tif (nope)\n\t{\n\t}\n}\n", &[(3, "Can't find variable 'nope'")]),
        // Calls.
        row(
            "body-P20",
            "void G(int a)\n{\n}\nvoid F()\n{\n\tG();\n}\n",
            &[(6, "Not enough parameters in function 'G'")],
        ),
        row(
            "body-P21",
            "void G()\n{\n}\nvoid F()\n{\n\tG(1);\n}\n",
            &[(6, "Too many parameters for 'G' method")],
        ),
        row(
            "body-P28",
            "class PrA\n{\n\tvoid M()\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA.M();\n}\n",
            &[(9, "Trying to call non-static function 'M' as static")],
        ),
        row(
            "body-P29",
            "class PrA\n{\n\tint x;\n\tstatic void S()\n\t{\n\t\tx = 1;\n\t}\n}\n",
            &[
                (6, "Trying to access non-static member 'x' from static method 'S'"),
                (6, "Can't find variable 'x'"),
            ],
        ),
        row(
            "body-P30",
            "class PrA\n{\n\tprivate void P()\n\t{\n\t}\n}\nclass PrB extends PrA\n{\n\tvoid M()\n\t{\n\t\tP();\n\t}\n}\n",
            &[(11, "Method 'P' is private")],
        ),
        row(
            "body-P31",
            "class PrA\n{\n\tprotected void P()\n\t{\n\t}\n}\nvoid F(PrA a)\n{\n\ta.P();\n}\n",
            &[(9, "Method 'P' is protected")],
        ),
        row(
            "body-P32",
            "class PrA\n{\n\tprivate int x;\n}\nvoid F(PrA a)\n{\n\ta.x = 1;\n}\n",
            &[(7, "Variable 'x' is private"), (7, "Can't find variable 'x'")],
        ),
        row(
            "body-P33",
            "class PrA\n{\n\tvoid PrA()\n\t{\n\t}\n}\nvoid F(PrA a)\n{\n\ta.PrA();\n}\n",
            &[(9, "Cannot call constructor or destructor 'PrA' method directly!")],
        ),
        // `this`, `super` and type names as values.
        row(
            "body-P112",
            "void F()\n{\n\tPrint(this);\n}\n",
            &[(3, "'this' or 'super' outside class is not valid"), (3, "Can't find variable 'this'")],
        ),
        row(
            "body-P118",
            "void F()\n{\n\tint i = int.Cast(1);\n}\n",
            &[(3, "Cast not supported on type 'int'")],
        ),
        // The probes that compile: a type name as a void argument, and an enum
        // value reused as a local name.
        row("body-P18", "void F()\n{\n\tPrint(map);\n}\n", &[]),
        row("body-P47", "enum E\n{\n\tNONE\n}\nvoid F()\n{\n\tint NONE;\n}\n", &[]),
    ]
}

#[test]
fn the_body_rows_report_the_lines_the_game_printed() {
    let mut failures = Vec::new();
    for row in rows() {
        let got = lines(row.files);
        let expected: Vec<(u32, String)> = row
            .expected
            .iter()
            .map(|(line, message)| (*line, (*message).to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{}: got {got:?}, want {expected:?}", row.id));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// A module's bodies see what the modules before it declared, so a call to a
// function of the next layer is the error the game prints.
#[test]
fn a_call_to_a_later_layer_is_undefined() {
    let case = input(&[
        ("Scripts/3_Game/a.c", "void F()\n{\n\tLaterFn();\n}\n"),
        ("Scripts/4_World/b.c", "void LaterFn()\n{\n}\n"),
    ]);
    assert_eq!(
        check(&case)
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(3, "Undefined function 'LaterFn'")]
    );
}

// A name the earlier layer declared resolves in the later one.
#[test]
fn a_call_to_an_earlier_layer_resolves() {
    let text = "void LaterFn()\n{\n}\n\nvoid F()\n{\n\tLaterFn();\n}\n";
    let case = input(&[("Scripts/1_Core/a.c", text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// Locals are function scoped: a name declared in one block is still the same
// name in the block after it.
#[test]
fn a_local_of_one_block_is_still_in_scope_in_the_next() {
    let text = "void F()\n{\n\tif (true)\n\t{\n\t\tint i;\n\t}\n\tif (true)\n\t{\n\t\ti = 1;\n\t}\n}\n";
    let case = input(&[(PROBE, text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// A method named as a value is the callback the game builds out of it.
#[test]
fn a_method_named_as_a_value_compiles() {
    let text = "class Q\n{\n\tvoid Call(func f)\n\t{\n\t}\n}\n\nclass G\n{\n\tQ GetQueue()\n\t{\n\t\treturn null;\n\t}\n\n\tvoid CheckDialogs()\n\t{\n\t}\n}\n\nG g_Game;\n\nvoid F()\n{\n\tg_Game.GetQueue().Call(g_Game.CheckDialogs);\n}\n";
    let case = input(&[(PROBE, text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// Every script class derives from `Class`, so its methods resolve everywhere.
#[test]
fn the_root_class_methods_resolve_on_any_class() {
    let text = "class C\n{\n\tvoid M()\n\t{\n\t\ttypename t = Type();\n\t}\n}\n";
    let case = input(&[
        ("Scripts/1_Core/root.c", "class Class\n{\n\tproto native typename Type();\n}\n"),
        (PROBE, text),
    ]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// A foreach over a map yields the key then the value, and over anything else the
// element.
#[test]
fn a_foreach_takes_the_element_type_its_iterable_yields() {
    let text = "class V\n{\n\tint m_A;\n}\n\nclass C\n{\n\tvoid M(map<int, V> m)\n\t{\n\t\tforeach (int k, V v : m)\n\t\t{\n\t\t\tv.m_A = k;\n\t\t}\n\t}\n}\n";
    let case = input(&[(ENGINE, ENGINE_TEXT), (PROBE, text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// The body checker runs on whatever the parser produced, including hostile text,
// and must never panic. The text is built from words so that it forms method
// bodies: statements over a small set of classes, typedefs and templates.
#[test]
fn any_bodies_are_checked_without_panicking() {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    const PRELUDE: &str = "class Class\n{\n}\nclass A\n{\n\tint f;\n\tvoid M(int a)\n\t{\n\t}\n\tvoid M(float a)\n\t{\n\t}\n}\n\
        class B : A\n{\n}\nclass T<Class V>\n{\n\tV item;\n\tV Get()\n\t{\n\t\treturn item;\n\t}\n}\n\
        typedef T<A> TA;\ntypedef int Num;\nclass Num2 : Num\n{\n}\nmodded class B\n{\n}\n";
    let words = [
        "a", "b", "f", "M", "Get", "item", "this", "super", "new", "A", "B", "TA", "Num", "Num2", "T<A>", "int",
        "float", "string", "auto", "ref", "null", "0", "1.5", "\"s\"", "=", "==", "+", "*", "&&", "!", "(", ")",
        "[", "]", "{", "}", ";", ",", ".", "<", ">", "if", "else", "for", "foreach", "while", "return", "delete",
        "Sleep", "Print", "Cast", "typename", "static", "private", "override", "value", "m", "x",
    ];
    let mut reached = 0;
    for _ in 0..400 {
        // A few statements of one to five words each, so most inputs parse.
        let mut body = String::new();
        for _ in 0..(1 + next() % 5) {
            for _ in 0..(1 + next() % 5) {
                body.push_str(words[(next() % words.len() as u64) as usize]);
                body.push(' ');
            }
            body.push_str(";\n\t\t");
        }
        let text = format!("{PRELUDE}class X : B\n{{\n\tvoid F(A a, TA t)\n\t{{\n\t\t{body}\n\t}}\n}}\n");
        let got = check(&input(&[(PROBE, &text)]));
        reached += usize::from(got.diagnostics.iter().any(|d| {
            d.message.starts_with("Can't find variable") || d.message.starts_with("Undefined function")
        }));
    }
    assert!(reached >= 20, "only {reached} of 400 inputs reached the body checker");
}

// Cycles in bases and aliases end the check cleanly rather than looping.
#[test]
fn cyclic_bases_and_aliases_are_checked_without_panicking() {
    let cases = [
        "class E : E\n{\n\tvoid F()\n\t{\n\t\tsuper.F();\n\t}\n}\n",
        "class P : Q\n{\n\tvoid F()\n\t{\n\t\tNope();\n\t}\n}\nclass Q : P\n{\n}\n",
        "typedef B A;\ntypedef A B;\nclass C\n{\n\tvoid F(A a)\n\t{\n\t\ta.x = 1;\n\t}\n}\n",
        "class T<Class V> : T<V>\n{\n\tvoid F()\n\t{\n\t\tNope();\n\t}\n}\n",
    ];
    for text in cases {
        let _ = check(&input(&[(PROBE, text)]));
    }
}

// A class that derives a primitive keeps that primitive's implicit `value`.
#[test]
fn a_primitive_derived_class_has_value() {
    // The shape the mods write: a chain of classes each deriving the one below,
    // bottoming out on a primitive the engine declares.
    let cases: [(&str, &str); 2] = [
        (
            "direct",
            "class int\n{\n}\n\nclass MyByte : int\n{\n\tint Get()\n\t{\n\t\treturn value;\n\t}\n}\n",
        ),
        (
            "two-level",
            "class int\n{\n}\n\nclass Uint : int\n{\n}\n\nclass MyByte : Uint\n{\n\tint Get()\n\t{\n\t\treturn value;\n\t}\n}\n",
        ),
    ];
    assert_clean(&cases);
}

// A comparison is not an assignment: its operands are read with no expected
// type, so a type name on either side is a `typename` constant, and the result
// is a bool.
#[test]
fn a_comparison_is_not_an_assignment() {
    let cases: [(&str, &str); 3] = [
        (
            "type name on the right",
            "class int\n{\n}\nclass bool\n{\n}\nclass C\n{\n\tvoid M(int i)\n\t{\n\t\tif (i == bool)\n\t\t{\n\t\t}\n\t}\n}\n",
        ),
        (
            "type name in a conjunction",
            "class int\n{\n}\nclass bool\n{\n}\nclass C\n{\n\tvoid M(int i, int t)\n\t{\n\t\tif (i == 1 && t == int)\n\t\t{\n\t\t}\n\t}\n}\n",
        ),
        (
            "not equal",
            "class bool\n{\n}\nclass C\n{\n\tvoid M(int i)\n\t{\n\t\tif (i != bool)\n\t\t{\n\t\t}\n\t}\n}\n",
        ),
    ];
    assert_clean(&cases);
}

// `Sleep` and `Wait` are statements, not functions: the argument is checked and
// the name itself is never looked up.
#[test]
fn a_sleep_or_wait_statement_is_not_a_call() {
    let text = "class C\n{\n\tvoid M(float t)\n\t{\n\t\tSleep(10);\n\t\tSleep(t);\n\t\tWait(true);\n\t}\n}\n";
    let case = input(&[(PROBE, text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// After an expression fails to resolve, its type is unknown and nothing more is
// said about what is done with it. The game prints the first line only.
#[test]
fn a_failed_expression_says_nothing_further() {
    let text = "class C\n{\n\tvoid M(C o)\n\t{\n\t\to.Unknowable() == int;\n\t}\n}\n";
    assert_eq!(messages(text), vec!["Undefined function 'C.Unknowable'"]);
}

// A call against overloads that no candidate accepts prints the missing-overload
// row, including when every candidate was dropped by argument count or type.
#[test]
fn an_overloaded_call_with_no_match_reports_it() {
    let overloads = "class C\n{\n\tvoid M(int a)\n\t{\n\t}\n\tvoid M(string a)\n\t{\n\t}\n";
    let cases: [(&str, &str); 3] = [
        ("too many", "void Run(C o)\n\t{\n\t\to.M(1, 2);\n\t}\n}\n"),
        ("too few", "void Run(C o)\n\t{\n\t\to.M();\n\t}\n}\n"),
        ("no conversion", "void Run(C o)\n\t{\n\t\to.M(o);\n\t}\n}\n"),
    ];
    let mut failures = Vec::new();
    for (name, body) in cases {
        let text = format!("{overloads}{body}");
        let got = messages(&text);
        let want = vec!["Can't find matching overload for function 'M'".to_string()];
        if got != want {
            failures.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// Overloaded global functions resolve the same way, and their arguments are
// still typed so a missing name inside one is reported.
#[test]
fn overloaded_global_functions_resolve() {
    let head = "void G(int a)\n{\n}\nvoid G(float a)\n{\n}\n";
    let cases: [(&str, &str, Vec<&str>); 4] = [
        (
            "no candidate",
            "void H(float f)\n{\n\tG(f, f);\n}\n",
            vec!["Can't find matching overload for function 'G'"],
        ),
        (
            "too few",
            "void H()\n{\n\tG();\n}\n",
            vec!["Can't find matching overload for function 'G'"],
        ),
        (
            "argument name missing",
            "void H()\n{\n\tG(nope);\n}\n",
            // Its own missing name is the only line: no follow-on row.
            vec!["Can't find variable 'nope'"],
        ),
        ("resolves", "void H()\n{\n\tG(1);\n}\n", vec![]),
    ];
    let mut failures = Vec::new();
    for (name, body, expected) in cases {
        let text = format!("{head}{body}");
        let got = messages(&text);
        if got != expected {
            failures.push(format!("{name}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// `a.Cast(x)` on an instance is the shape the parser rejects, so the game's line
// is a syntax error rather than a cast on a type.
#[test]
fn a_cast_on_an_instance_is_a_syntax_error() {
    let text = "class PrA\n{\n}\nvoid F(PrA a)\n{\n\tPrA b = a.Cast(a);\n}\n";
    assert_eq!(messages(text), vec!["Syntax error"]);
}

// A private constructor reached through `new` prints the access row.
#[test]
fn a_private_constructor_reports_it() {
    let text = "class PrA\n{\n\tprivate void PrA()\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA();\n}\n";
    assert_eq!(messages(text), vec!["Method 'PrA' is private"]);
}

// A statement split over two lines reports the line it starts on, never the line
// of the word that failed.
#[test]
fn a_split_statement_reports_its_first_line() {
    let cases: [(&str, &str, u32); 2] = [
        (
            "call",
            "void G(int a)\n{\n}\nvoid F()\n{\n\tG(\n\t\tnope);\n}\n",
            6,
        ),
        (
            "assignment",
            "void F()\n{\n\tint a =\n\t\tnope;\n}\n",
            3,
        ),
    ];
    let mut failures = Vec::new();
    for (name, text, line) in cases {
        let case = input(&[(PROBE, text)]);
        let got: Vec<(u32, String)> = check(&case)
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let want = vec![(line, "Can't find variable 'nope'".to_string())];
        if got != want {
            failures.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// A nested call against overloads is typed once per argument, so the work is
// linear in the depth and each cause is reported once.
#[test]
fn a_nested_overloaded_call_is_typed_once_per_argument() {
    let depth = 12;
    let mut expr = String::from("o");
    for _ in 0..depth {
        expr = format!("o.M({expr})");
    }
    let text = format!(
        "class C\n{{\n\tvoid M(C c)\n\t{{\n\t}}\n\tvoid M(int i)\n\t{{\n\t}}\n\tvoid Run(C o)\n\t{{\n\t\t{expr}.M(o.p);\n\t}}\n\tprivate int p;\n}}\n"
    );
    let case = input(&[(PROBE, &text)]);
    let started = std::time::Instant::now();
    let got = check(&case).diagnostics;
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "depth {depth} took {elapsed:?}"
    );
    let private: Vec<&String> = got
        .iter()
        .map(|diagnostic| &diagnostic.message)
        .filter(|message| message.contains("is private"))
        .collect();
    assert!(private.len() <= 1, "repeated: {private:?}");
}

// A long `else if` ladder is walked without growing the stack per branch.
#[test]
fn a_long_else_if_ladder_is_checked_without_deep_recursion() {
    let mut text = String::from("class C\n{\n\tvoid F(int x)\n\t{\n");
    text.push_str("\t\tif (x == 0)\n\t\t{\n\t\t}\n");
    for level in 1..1200 {
        text.push_str(&format!("\t\telse if (x == {level})\n\t\t{{\n\t\t}}\n"));
    }
    text.push_str("\t}\n}\n");
    let case = input(&[(PROBE, &text)]);
    // The one line is the game's own crash on a ladder this long.
    let lines: Vec<(u32, bool)> = check(&case).diagnostics.iter().map(|d| (d.line, d.game_crash)).collect();
    assert_eq!(lines, [(5, true)]);
}

// A warning does not stop a file: the game still compiles its bodies, so a body
// error beside a warning is reported.
#[test]
fn a_warning_does_not_hide_a_body_error() {
    let text = "void F(int x)\n{\n\twhile (x);\n\tNope();\n}\n";
    let case = input(&[(PROBE, text)]);
    let got: Vec<String> = check(&case)
        .diagnostics
        .iter()
        .map(|diagnostic| format!("{:?} {}", diagnostic.severity, diagnostic.message))
        .collect();
    assert!(
        got.iter().any(|line| line.contains("Undefined function 'Nope'")),
        "{got:?}"
    );
}

// A field whose declared type is one of its class's template parameters takes
// the argument the use site supplied, the way a method's return type does.
#[test]
fn a_generic_field_takes_the_type_argument() {
    let text = "class A\n{\n\tint a;\n}\n\nclass Box<Class K>\n{\n\tK item;\n}\n\nclass C\n{\n\tvoid M(Box<A> b)\n\t{\n\t\tint x = b.item.a;\n\t}\n}\n";
    let case = input(&[(PROBE, text)]);
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// Only the overload whose parameter is the argument's class or a base of it
// stays; a class that merely sits next to it in the hierarchy is dropped.
#[test]
fn an_argument_picks_the_overload_it_derives_from() {
    let text = "class Class\n{\n}\n\nclass SV\n{\n}\n\nclass GP\n{\n}\n\nclass MBP : SV\n{\n\tvoid MBP(string s)\n\t{\n\t}\n}\n\nclass D\n{\n\tvoid Add(GP g)\n\t{\n\t}\n\n\tvoid Add(SV s)\n\t{\n\t}\n\n\tvoid F()\n\t{\n\t\tAdd(new MBP(\"x\"));\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A `modded` class reads the members the class it modifies declared, protected
// ones included.
#[test]
fn a_modded_class_reads_the_protected_members_of_its_original() {
    let text = "class ES\n{\n\tprotected int m_W;\n}\n\nmodded class ES\n{\n\tint Get()\n\t{\n\t\treturn m_W;\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A field and a method typed by the base's template parameter take the
// argument the derived class gave its base.
#[test]
fn an_inherited_member_takes_the_argument_of_the_template_base() {
    let text = "class VT<Class T>\n{\n\tprotected T m_C;\n\n\tT GetC()\n\t{\n\t\treturn m_C;\n\t}\n}\n\nclass Ctl\n{\n\tint Active;\n}\n\nclass LI : VT<Ctl>\n{\n\tvoid F()\n\t{\n\t\tm_C.Active = 1;\n\t\tGetC().Active = 2;\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// The widget type ids are registered by the engine; the vanilla list of them
// sits in a documentation-only block.
#[test]
fn the_widget_type_ids_are_engine_names() {
    let text = "void F()\n{\n\tint a = MultilineTextWidgetTypeID;\n\tint b = WorkspaceWidgetTypeID;\n\tint c = RTTextureWidgetTypeID;\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
    // A widget class the list leaves out has its id all the same.
    let derived = "class Widget\n{\n}\nclass PrPreviewWidget : Widget\n{\n}\nclass PrOther\n{\n}\nvoid F()\n{\n\tint a = PrPreviewWidgetTypeID;\n\tint b = PrOtherTypeID;\n}\n";
    assert_eq!(messages(derived), ["Can't find variable 'PrOtherTypeID'"]);
}

// A mod's `modded` class reads the protected members of the vanilla class it
// modifies, which an earlier mod declared.
#[test]
fn a_modded_class_from_a_later_mod_reads_the_vanilla_members() {
    let mut case = input(&[
        ("Scripts/3_Game/Effects/EffectSound.c", "class EffectSound\n{\n\tprotected int m_W;\n}\n"),
        ("Scripts/3_Game/DayZGame.c", "modded class EffectSound\n{\n\tint Get()\n\t{\n\t\treturn m_W;\n\t}\n}\n"),
    ]);
    case.files[0].mod_index = 0;
    case.mods.insert(0, ModInfo { name: "DayZ".into(), cfg_class: "DayZ".into() });
    assert_eq!(check(&case).diagnostics, Vec::new());
}

// The overloads come from a base class and the call sits in a subclass; the
// argument is built in place from a class that derives only one parameter type.
#[test]
fn an_inherited_overload_set_is_picked_by_the_argument_class() {
    let text = "class SV\n{\n}\n\nclass PB<Class V> : SV\n{\n}\n\nclass GP : PB<int>\n{\n}\n\nclass MBP : SV\n{\n\tvoid MBP(string s)\n\t{\n\t}\n}\n\nclass DB : SV\n{\n\tSV Add(GP g)\n\t{\n\t\treturn Add((SV)g);\n\t}\n\n\tSV Add(SV s)\n\t{\n\t\treturn s;\n\t}\n}\n\nclass MB : DB\n{\n\tvoid MB(string c)\n\t{\n\t\tAdd(new MBP(c));\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// The body of a template class is compiled when something instantiates it; a
// mod ships one that names members its container lacks (dabs ObservableSet) and
// loads, so an uninstantiated template body is not checked.
#[test]
fn a_template_class_body_is_not_checked() {
    let text = "class Box<Class T>\n{\n\tvoid F()\n\t{\n\t\tNope();\n\t}\n}\n\nclass Plain\n{\n\tvoid G()\n\t{\n\t\tNope();\n\t}\n}\n";
    assert_eq!(messages(text), vec!["Undefined function 'Plain.Nope'".to_string()]);
}

// A parameter declared through a typedef takes the class the typedef names.
#[test]
fn a_typedef_parameter_accepts_its_class() {
    let text = "class Class\n{\n}\n\nclass Ser\n{\n}\n\ntypedef Ser Ctx;\n\nclass C\n{\n\tvoid M(Ctx c)\n\t{\n\t}\n\n\tvoid M(int a, Ctx c)\n\t{\n\t}\n\n\tvoid F(Ser s)\n\t{\n\t\tM(1, s);\n\t\tM(s);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// Inside the body of a class a `modded` wrapper replaced, `this` still passes
// where the plain class name is declared.
#[test]
fn a_replaced_class_passes_as_its_plain_name() {
    let text = "class Class\n{\n}\n\nclass T\n{\n\tvoid Take(P p)\n\t{\n\t}\n\n\tvoid Take(P p, int a)\n\t{\n\t}\n}\n\nclass P\n{\n\tvoid F(T t)\n\t{\n\t\tt.Take(this);\n\t}\n}\n\nmodded class P\n{\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// Two typedefs of the same container with different arguments stay different
// types, so each call finds its own overload.
#[test]
fn typedefs_of_one_container_stay_distinct() {
    let text = "class Class\n{\n}\n\nclass array<Class T>\n{\n}\n\nclass A\n{\n}\n\nclass B\n{\n}\n\ntypedef array<A> AA;\ntypedef array<B> BB;\n\nclass C\n{\n\tvoid R(AA a)\n\t{\n\t}\n\n\tvoid R(BB b)\n\t{\n\t}\n\n\tvoid F(AA a)\n\t{\n\t\tR(a);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A typedef of a container and the container written out are one type.
#[test]
fn a_container_typedef_is_the_container_it_names() {
    let text = "class Class\n{\n}\n\nclass array<Class T>\n{\n}\n\ntypedef array<string> Strs;\n\nclass C\n{\n\tstatic string J(array<string> a, string s)\n\t{\n\t\treturn s;\n\t}\n\n\tstatic string J(Strs a, string s, string t)\n\t{\n\t\treturn s;\n\t}\n\n\tvoid F(Strs a)\n\t{\n\t\tJ(a, \"x\");\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// An overload set told apart by a container's argument, and a parameter typed
// by a template parameter.
#[test]
fn container_arguments_and_template_parameters_pick_overloads() {
    let text = "class Class\n{\n}\n\nclass array<Class T>\n{\n}\n\nclass Item\n{\n}\n\ntypedef array<string> Strs;\n\nclass W<Class T>\n{\n\tstatic void Put(T s, int v)\n\t{\n\t}\n\n\tstatic void Put(T s, float v, int b)\n\t{\n\t}\n}\n\nclass C\n{\n\tstatic string J(array<Item> a)\n\t{\n\t\treturn \"\";\n\t}\n\n\tstatic string J(Strs a)\n\t{\n\t\treturn \"\";\n\t}\n\n\tvoid F(Strs a, Item i)\n\t{\n\t\tJ(a);\n\t\tW<Item>.Put(i, 1);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A call told apart only by a container's argument type, with the argument a
// member declared `autoptr` through the typedef.
#[test]
fn a_member_typedef_container_picks_its_overload() {
    let text = "class Class\n{\n}\n\nclass string\n{\n}\n\nclass array<Class T>\n{\n}\n\ntypedef array<string> Strs;\n\nclass ES : string\n{\n\tstatic string J(array<ES> a, string g = \", \", bool r = false)\n\t{\n\t\treturn g;\n\t}\n\n\tstatic string J(Strs a, string g = \", \", bool r = false)\n\t{\n\t\treturn g;\n\t}\n}\n\ntypedef string ES;\n\nclass S\n{\n\tstatic autoptr Strs m_B = {};\n\n\tstatic void F()\n\t{\n\t\tES.J(m_B, \"x\");\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// `null` is a valid argument for a `Class` parameter.
#[test]
fn null_is_accepted_for_a_class_parameter() {
    let text = "class Class\n{\n}\n\nclass T\n{\n\tstatic void P(bool y, Class c, string m)\n\t{\n\t}\n\n\tstatic void P(bool y, typename t, string m)\n\t{\n\t}\n\n\tstatic void F()\n\t{\n\t\tP(true, null, \"x\");\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A generic type nested in a member or a return type takes the arguments of the
// receiver the way a bare parameter does.
#[test]
fn nested_generic_types_take_the_receivers_arguments() {
    let text = "class Class\n{\n}\n\nclass array<Class T>\n{\n\tproto native int Count();\n\tproto T Get(int n);\n}\n\nclass LX\n{\n\tint f;\n}\n\nclass LNode<Class T>\n{\n\tT m_Value;\n\tref LNode<T> m_Next;\n}\n\nclass LList<Class T>\n{\n\tref LNode<T> m_Head;\n\tarray<T> All()\n\t{\n\t\treturn null;\n\t}\n}\n\nclass C\n{\n\tvoid Do(LX x, int a)\n\t{\n\t}\n\n\tvoid Do(LX x, float a, int b)\n\t{\n\t}\n\n\tvoid F(LList<LX> l)\n\t{\n\t\tauto n = l.m_Head;\n\t\tn.m_Value.f = 1;\n\t\tn.m_Next.m_Value.f = 2;\n\t\tDo(n.m_Value, 1);\n\t\tDo(l.All().Get(0), 1);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A class built on a template instance still reports a missing method.
#[test]
fn a_template_instance_base_does_not_hide_a_missing_method() {
    let text = "class Class\n{\n}\n\nclass GB<Class T>\n{\n}\n\nclass GD : GB<int>\n{\n\tvoid F()\n\t{\n\t\tNope();\n\t}\n}\n";
    assert_eq!(messages(text), vec!["Undefined function 'GD.Nope'".to_string()]);
}

// An enum member passes where an `int` parameter is declared.
#[test]
fn an_enum_argument_fits_an_int_parameter() {
    let text = "class Class\n{\n}\n\nenum E\n{\n\tA,\n\tB\n}\n\nclass C\n{\n\tvoid G(int a)\n\t{\n\t}\n\n\tvoid G(int a, int b)\n\t{\n\t}\n\n\tvoid F()\n\t{\n\t\tE e = E.A;\n\t\tG(e);\n\t\tG(E.B);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// A nested call whose overloads resolve is typed once per argument, so deep
// nesting stays fast and an error inside it prints once. Sixteen is as deep
// as calls are read.
#[test]
fn a_deeply_nested_resolved_call_stays_fast_and_prints_once() {
    let depth = 16;
    let mut call = String::from("o.p");
    for _ in 0..depth {
        call = format!("o.M({call})");
    }
    let text = format!("class Class\n{{\n}}\n\nclass O\n{{\n\tprivate int p;\n\n\tint M(int a)\n\t{{\n\t\treturn a;\n\t}}\n\n\tint M(float a)\n\t{{\n\t\treturn 1;\n\t}}\n}}\n\nclass C\n{{\n\tvoid F(O o)\n\t{{\n\t\tint x = {call};\n\t}}\n}}\n");
    let started = std::time::Instant::now();
    let got = messages(&text);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    assert_eq!(got.iter().filter(|m| m.contains("is private")).count(), 1, "{got:?}");
}

// An error inside a call or a member access that starts on a later line still
// carries the first line of its statement.
#[test]
fn an_error_in_a_split_call_or_member_reports_the_statement_line() {
    let text = "class O\n{\n}\n\nvoid G(int a)\n{\n}\n\nvoid F(O o)\n{\n\tG(\n\t\tNope());\n\tint a =\n\t\to.nofield;\n}\n";
    let lines: Vec<u32> = check(&input(&[(PROBE, text)])).diagnostics.iter().map(|d| d.line).collect();
    assert_eq!(lines, vec![11, 13], "{lines:?}");
}

// Protection still holds for an engine method called from an unrelated class.
#[test]
fn a_protected_native_method_is_still_held_against_an_unrelated_class() {
    let text = "class Class\n{\n}\n\nclass L\n{\n\tproto protected native Class Get();\n}\n\nclass U\n{\n\tvoid F(L l)\n\t{\n\t\tl.Get();\n\t}\n}\n";
    assert_eq!(messages(text), vec!["Method 'Get' is protected".to_string()]);
}

// Access is judged on the overload the call resolves to, not on the first method
// of that name.
#[test]
fn access_follows_the_resolved_overload() {
    let text = "class Class\n{\n}\n\nclass V\n{\n\tprotected void U()\n\t{\n\t}\n\n\tvoid U(int a, bool b)\n\t{\n\t}\n}\n\nclass M\n{\n\tvoid F(V v)\n\t{\n\t\tv.U(1, true);\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}

// The engine registers every input as a number under its name: its own start
// with `UA`, a mod's are whatever its `inputs.xml` says.
#[test]
fn an_input_name_is_a_number() {
    let text = "void F()\n{\n\tint a = UASomeEngineInput;\n\tint b = GearToggle;\n}\n";
    let mut registered = input(&[(PROBE, text)]);
    registered.input_names = vec!["GearToggle".into()];
    assert_eq!(check(&registered).diagnostics, []);
    assert_eq!(messages(text), ["Can't find variable 'GearToggle'"]);
}

// `thread` on a name with several overloads has no single signature to hold
// the arguments to.
#[test]
fn a_thread_of_an_overloaded_method_is_not_held_to_the_first_overload() {
    let text = "class PrA\n{\n\tvoid G(int a)\n\t{\n\t}\n\tvoid G(string a, string b)\n\t{\n\t}\n\tvoid M()\n\t{\n\t\tthread G(\"x\", \"y\");\n\t}\n}\n";
    assert_eq!(messages(text), Vec::<String>::new());
}
