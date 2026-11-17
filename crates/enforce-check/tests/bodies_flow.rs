mod support;

use enforce_check::Target;
use support::{Row, assert_rows, row};

fn local_rows() -> Vec<Row> {
    vec![
        row(
            "body-P42",
            "void F()\n{\n\tint map;\n}\n",
            &[(3, "Variable name 'map' already used as type name")],
        ),
        row(
            "body-P43",
            "class PrA\n{\n\tint x;\n\tvoid M()\n\t{\n\t\tint x;\n\t}\n}\n",
            &[(6, "Possible variable name conflict 'x'")],
        ),
        row(
            "body-P44",
            "void F()\n{\n\tif (true)\n\t{\n\t\tint i;\n\t}\n\tif (true)\n\t{\n\t\tint i;\n\t}\n}\n",
            &[(9, "Multiple declaration of variable 'i'")],
        ),
        row(
            "body-P45",
            "void F()\n{\n\tfor (int i = 0; i < 1; i++)\n\t{\n\t}\n\tfor (int i = 0; i < 1; i++)\n\t{\n\t}\n}\n",
            &[(6, "Multiple declaration of variable 'i'")],
        ),
        row("body-P46", "void F(int a)\n{\n\tint a;\n}\n", &[(3, "Multiple declaration of variable 'a'")]),
        row("body-P47", "enum E\n{\n\tNONE\n}\nvoid F()\n{\n\tint NONE;\n}\n", &[]),
        // Vanilla names a local constant like a constant member of an
        // unrelated class.
        row(
            "local-constants",
            "class PrA\n{\n\tvoid M()\n\t{\n\t\tconst int SIZE = 8;\n\t\tstring names[SIZE];\n\t}\n}\nclass PrB\n{\n\tconst int SIZE = 9;\n}\n",
            &[],
        ),
        row(
            "body-P121",
            "class PrA\n{\n\tstatic int s;\n\tvoid M()\n\t{\n\t\tint s;\n\t}\n}\n",
            &[(6, "Multiple declaration of variable 's'")],
        ),
        row(
            "body-P50",
            "void F()\n{\n\ttypename t[3];\n}\n",
            &[(3, "FIX-ME: Static array of 'typename' types is not supported")],
        ),
        row(
            "body-P51",
            "void F()\n{\n\tvector v[2][2];\n}\n",
            &[(3, "FIX-ME: Static 2D array of 'vector' is not supported")],
        ),
        row("body-P52", "void F()\n{\n\tint a[0];\n}\n", &[(3, "Wrong array size")]),
        row("body-P53", "void F(int n)\n{\n\tint a[n];\n}\n", &[(3, "Expected numeric constant")]),
        row(
            "body-P54",
            "void F()\n{\n\tint a[70000];\n}\n",
            &[(3, "Maximum static array index is 65535 'a'")],
        ),
        row(
            "body-P55",
            "void F()\n{\n\tint a[40000];\n}\n",
            &[(3, "Variable 'a' too large"), (3, "Can't create variable 'a'")],
        ),
        row(
            "body-P56",
            "void F()\n{\n\tconst auto x = 1;\n}\n",
            &[(3, "static/const variables can't be auto")],
        ),
        row(
            "body-P57",
            "void F()\n{\n\tout int x;\n}\n",
            &[(3, "inout/out/notnull can be used only on function parameters")],
        ),
        row(
            "body-P59",
            "void F()\n{\n\tauto x;\n}\n",
            &[(3, "Syntax error"), (3, "Unknown type 'auto'"), (3, "Error in parameters")],
        ),
    ]
}

#[test]
fn a_local_declaration_the_game_rejects_is_reported() {
    assert_rows(Target::RetailServer, &local_rows());
}

fn statement_rows() -> Vec<Row> {
    vec![
        row(
            "body-P60",
            "int F()\n{\n\tint x = 1;\n}\n",
            &[(1, "No return statement in function returning non-void 'F'")],
        ),
        row(
            "body-P62",
            "class PrA\n{\n\tint M()\n\t{\n\t\tint x = 1;\n\t}\n}\n",
            &[(3, "No return statement in function returning non-void 'PrA.M'")],
        ),
        row(
            "body-P63",
            "int F()\n{\n\twhile (true)\n\t{\n\t\treturn 1;\n\t}\n}\n",
            &[(1, "No return statement in function returning non-void 'F'")],
        ),
        row(
            "body-P64",
            "int F(int a)\n{\n\tif (a)\n\t{\n\t\treturn 1;\n\t}\n\telse\n\t{\n\t\treturn 2;\n\t}\n}\n",
            &[],
        ),
        row(
            "body-P65",
            "int F(int a)\n{\n\tswitch (a)\n\t{\n\t\tcase 1:\n\t\t\treturn 1;\n\t\tdefault:\n\t\t\treturn 2;\n\t}\n}\n",
            &[(1, "No return statement in function returning non-void 'F'")],
        ),
        row("body-P66", "int F()\n{\n}\n", &[]),
        // A body that fails the return shape is not read any further.
        row(
            "no-return-hides-the-body",
            "int F()\n{\n\tint x = nope;\n}\n",
            &[(1, "No return statement in function returning non-void 'F'")],
        ),
        row("body-P67", "void F()\n{\n\tbreak;\n}\n", &[(3, "Misplaced break/case/continue")]),
        row(
            "body-P68",
            "void F(int a)\n{\n\tswitch (a)\n\t{\n\t\tcase 1:\n\t\t\tcontinue;\n\t}\n}\n",
            &[(6, "Misplaced break/case/continue")],
        ),
        row(
            "body-P69",
            "void F(int a)\n{\n\tswitch (a)\n\t{\n\t\tcase 1:\n\t\t\tPrint(\"a\");\n\t\tcase 2:\n\t\t\tbreak;\n\t}\n}\n",
            &[(6, "Switch: fallthrough cases can't have a body")],
        ),
        row(
            "body-P70",
            "void F(int a)\n{\n\tswitch (a)\n\t{\n\t\tcase 0:\n\t\t\tbreak;\n\t\tcase 1:\n\t\t\tPrint(\"a\");\n\t\tcase 2:\n\t\t\tbreak;\n\t}\n}\n",
            &[],
        ),
        // A case body that returns on every path does not fall through, and one
        // that ends any other way does.
        row(
            "case-returns-on-both-branches",
            "int F(int a, bool b)\n{\n\tswitch (a)\n\t{\n\t\tcase 1:\n\t\t\tif (b)\n\t\t\t{\n\t\t\t\treturn 1;\n\t\t\t}\n\t\t\telse\n\t\t\t{\n\t\t\t\treturn 2;\n\t\t\t}\n\t\tcase 2:\n\t\t\t{\n\t\t\t\treturn 3;\n\t\t\t}\n\t\tcase 3:\n\t\t\tbreak;\n\t}\n\treturn 0;\n}\n",
            &[],
        ),
        row(
            "case-ends-in-continue",
            "void F(int a)\n{\n\twhile (a)\n\t{\n\t\tswitch (a)\n\t\t{\n\t\t\tcase 1:\n\t\t\t\tcontinue;\n\t\t\tcase 2:\n\t\t\t\tbreak;\n\t\t}\n\t}\n}\n",
            &[(8, "Switch: fallthrough cases can't have a body")],
        ),
        row(
            "foreach-over-a-map",
            "void F(map<string, int> m)\n{\n\tforeach (int k, int v : m)\n\t{\n\t}\n\tforeach (string name, auto count : m)\n\t{\n\t}\n}\n",
            &[(3, "Incompatible parameter 'k'")],
        ),
        row("delete-a-class-value", "class PrA\n{\n}\nvoid F(PrA a)\n{\n\tdelete a;\n}\n", &[]),
        row("body-P78", "void F()\n{\n\twhile (true)\n\t{\n\t\tbreak;\n\t}\n}\n", &[]),
        row("body-P80", "int F()\n{\n\treturn;\n}\n", &[(3, "Function must return value")]),
        row(
            "body-P81",
            "void F()\n{\n\treturn 1;\n}\n",
            &[(3, "return-statement with a value, in function returning 'void'")],
        ),
        row(
            "body-P71",
            "class PrA\n{\n}\nvoid F(PrA a)\n{\n\tforeach (int x : a)\n\t{\n\t}\n}\n",
            &[(6, "'a' is not array")],
        ),
        row(
            "body-P72",
            "void F(array<string> a)\n{\n\tforeach (int x : a)\n\t{\n\t}\n}\n",
            &[(3, "Incompatible parameter 'x'")],
        ),
        row(
            "foreach-over-a-map-typedef",
            "typedef map<string, int> PrCounts;\nvoid F(PrCounts counts)\n{\n\tforeach (string name, int count : counts)\n\t{\n\t}\n}\n",
            &[],
        ),
        row("body-P73", "void F()\n{\n\tint i;\n\tdelete i;\n}\n", &[(4, "Incompatible parameter 'on delete'")]),
    ]
}

#[test]
fn a_statement_the_game_rejects_is_reported() {
    assert_rows(Target::RetailServer, &statement_rows());
}

fn list_rows() -> Vec<Row> {
    vec![
        row(
            "body-P103",
            "void F()\n{\n\tmap<int,int> m = {1};\n}\n",
            &[(3, "Array initialization of non-array type 'map<int,int>'")],
        ),
        row("body-P104", "void F()\n{\n\tint a[2] = {1,2,3};\n}\n", &[(3, "Too many values")]),
        row("body-P105", "void F()\n{\n\tPrint({1});\n}\n", &[(3, "Array initialization of non-array type")]),
        // Expansion fills a static array declared with no size.
        row(
            "unsized-static-array",
            "class PrDef\n{\n}\nvoid F()\n{\n\tautoptr PrDef defs[] = {null};\n\tint n[] = {1, 2};\n}\n",
            &[],
        ),
        row(
            "lists-that-fit",
            "typedef array<string> PrNames;\nvoid F()\n{\n\tint a[3] = {1,2,3};\n\tarray<int> b = {1,2};\n\tPrNames c = {\"a\"};\n\tb = {3};\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_list_needs_an_array_to_land_in() {
    assert_rows(Target::RetailServer, &list_rows());
}

// The game dies on these without printing a line, so each is reported as an
// error that says so.
#[test]
fn an_input_that_crashes_the_game_is_reported_as_one() {
    let crashes = |text: &str| -> Vec<(u32, bool)> {
        support::diagnostics(Target::RetailServer, text)
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.game_crash))
            .collect()
    };
    assert_eq!(crashes("void F()\n{\n\tint x = 5 / 0;\n}\n"), [(3, true)], "body-P36");
    assert_eq!(crashes("void F()\n{\n\tgoto nowhere;\n}\n"), [(3, true)], "body-P61");
    assert_eq!(crashes("void F(int n)\n{\n\tint x = 5 / 2;\n\tx = n / 0;\n}\n"), [], "a division the game computes later");
    assert_eq!(crashes("void F()\n{\n\tfloat f = 1.5 / 0;\n}\n"), [], "a float division");
    // asm-D54a: the game has no label to jump to. `name:` ends the parse, so
    // a `goto` beside one never reaches the compiler.
    let labelled = crashes("void F()\n{\n\tgoto done;\n\tdone:\n\treturn;\n}\n");
    assert_eq!(labelled, [(4, false), (5, false)]);
    // par-lad125 compiles and par-lad150 takes the game down.
    let ladder = |links: usize| {
        let branches: String = (1..=links).map(|at| format!("\telse if (a == {at}) {{}}\n")).collect();
        crashes(&format!("void F(int a)\n{{\n\tif (a == 0) {{}}\n{branches}}}\n"))
    };
    assert_eq!(ladder(125), []);
    assert_eq!(ladder(150), [(3, true)]);
    // par-11: however long the ladder, the crash is all there is to say.
    assert_eq!(ladder(2500), [(3, true)]);
    // dp-01: an `if` that is the whole body of another goes the same way.
    let bare = format!("void F(bool x)\n{{\n\t{}x = false;\n\tNope();\n}}\n", "if (x) ".repeat(200));
    assert_eq!(crashes(&bare), [(3, true)]);
    // Unobserved: written over several lines, the crash is taken to sit on
    // the first `if`, as the ladder's does.
    let spread = format!("void F(bool x)\n{{\n{}\tx = false;\n}}\n", "\tif (x)\n".repeat(200));
    assert_eq!(crashes(&spread), [(3, true)]);
    // par-lad150: the game is gone, so nothing after the crash is reported.
    let after = crashes("void F()
{
	int x = 5 / 0;
	Nope();
}
void G()
{
	Nope();
}
");
    assert_eq!(after, [(3, true)]);
}

// asm-D49: the compiler names its own source file, which the Diag build keeps
// under another path.
#[test]
fn the_internal_error_line_names_the_build_of_the_target() {
    let text = "void F()\n{\n\tstring s;\n\ts -= \"a\";\n}\n";
    let message = |target| support::diagnostics(target, text)[0].message.clone();
    assert!(message(Target::RetailServer).contains("/data/jenkins-agent/workspace/"));
    assert_eq!(
        message(Target::DiagServer),
        "Internal compiler error D:\\jenkins\\cpu\\workspace\\_Continuous_branches_stable_0130\\CodeSVN\\Enfusion\\src\\script\\enf_scriptcompiler.cpp : 11363"
    );
}

#[test]
fn a_parameter_default_must_be_a_constant() {
    let rows = [
        row(
            "body-P58",
            "int Foo()\n{\n\treturn 1;\n}\nvoid G(int a = Foo())\n{\n}\n",
            &[(5, "Unknown type or code outside function")],
        ),
        row(
            "constant-defaults",
            "enum PrKind\n{\n\tONE\n}\nconst int LIMIT = 4;\nvoid G(int a = 5, int b = -1, string s = \"x\", int k = PrKind.ONE, float f = 1.5, Class c = null, vector v = \"0 0 0\", bool on = true, int limit = LIMIT, int both = PrKind.ONE|PrKind.ONE)\n{\n}\n",
            &[],
        ),
    ];
    assert_rows(Target::RetailServer, &rows);
}

// The return shape of a non-void function is read along each `else if` ladder
// without a call per link, however the ladders nest.
#[test]
fn nested_else_if_ladders_in_a_non_void_function_are_read_without_deep_recursion() {
    let mut text = String::from("int F(int a)\n{\n");
    for _ in 0..6 {
        text.push_str("if (a == 0) { return 0; }\n");
        for link in 1..1900 {
            text.push_str(&format!("else if (a == {link}) {{ return {link}; }}\n"));
        }
        text.push_str("else {\n");
    }
    text.push_str("return 1;\n");
    text.push_str(&"}\n".repeat(7));
    // The first ladder is long enough to take the game down, and that is all
    // the checker says.
    let crashes = support::diagnostics(Target::RetailServer, &text);
    assert_eq!(crashes.len(), 1);
    assert!(crashes[0].game_crash, "{crashes:?}");
}

// lim-01 to lim-08: the game holds 256 `break`s that wait for their loop or
// `switch` to end, across every scope still open; each one past that is
// refused. A `continue` does not wait.
#[test]
fn the_257th_pending_break_is_one_too_many() {
    const TOO_MANY: &str = "Too many breaks per scope";
    let jumps = |count: usize, tabs: &str, keyword: &str| -> String {
        (1..=count).map(|n| format!("{tabs}if (a == {n}) {keyword};\n")).collect()
    };
    let in_loop = |body: String| format!("void F(int a)\n{{\n\twhile (a > 0)\n\t{{\n{body}\t}}\n\tNope();\n}}\n");
    let cases: String = (1..=257).map(|n| format!("\t\tcase {n}:\n\t\t\tbreak;\n")).collect();
    let switch = format!("void F(int a)\n{{\n\tswitch (a)\n\t{{\n{cases}\t}}\n\tNope();\n}}\n");
    let nested = in_loop(format!("{}\t\twhile (a > 1)\n\t\t{{\n{}\t\t}}\n", jumps(129, "\t\t", "break"), jumps(129, "\t\t\t", "break")));
    let two_loops = format!(
        "void F(int a)\n{{\n\twhile (a > 0)\n\t{{\n{0}\t}}\n\twhile (a > 0)\n\t{{\n{0}\t}}\n\tNope();\n}}\n",
        jumps(129, "\t\t", "break")
    );
    let texts = [
        ("lim-01", in_loop(jumps(256, "\t\t", "break")), vec![(262, "Undefined function 'Nope'")]),
        ("lim-02", in_loop(jumps(257, "\t\t", "break")), vec![(261, TOO_MANY), (263, "Undefined function 'Nope'")]),
        ("lim-03", two_loops, vec![(267, "Undefined function 'Nope'")]),
        ("lim-06", switch, vec![(518, TOO_MANY), (520, "Undefined function 'Nope'")]),
        ("lim-07", nested, vec![(263, TOO_MANY), (264, TOO_MANY), (267, "Undefined function 'Nope'")]),
        ("lim-08", in_loop(jumps(257, "\t\t", "continue")), vec![(263, "Undefined function 'Nope'")]),
    ];
    for (id, text, expected) in texts {
        let got = support::lines(Target::RetailServer, &text);
        let got: Vec<(u32, &str)> = got.iter().map(|(line, message)| (*line, message.as_str())).collect();
        assert_eq!(got, expected, "{id}");
    }
}
