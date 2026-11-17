mod support;

use enforce_check::{Target, check};
use support::{Row, assert_rows, input, row};

fn assignment_rows() -> Vec<Row> {
    vec![
        row("body-P92", "void F()\n{\n\tstring s = 5;\n}\n", &[(3, "Incompatible parameter '5'")]),
        row("body-P93", "void F()\n{\n\tint i = \"a\";\n}\n", &[(3, "Incompatible parameter 'i'")]),
        row(
            "body-P96",
            "class PrA\n{\n}\nvoid F()\n{\n\tPrA a = 5;\n}\n",
            &[(6, "Incompatible parameter '5'")],
        ),
        row("body-P122", "void F()\n{\n\ttypename t = 5;\n}\n", &[(3, "Incompatible parameter '5'")]),
        row("body-P94", "void F()\n{\n\tint i = 1.5;\n}\n", &[]),
        row("body-P95", "void F()\n{\n\tfloat f = 1;\n\tbool b = 2;\n}\n", &[]),
    ]
}

#[test]
fn a_value_that_cannot_become_its_destination_is_reported() {
    assert_rows(Target::RetailServer, &assignment_rows());
}

const ICE: &str = "Internal compiler error /data/jenkins-agent/workspace/_Continuous_branches_stable_0130/CodeSVN/Enfusion/src/script/enf_scriptcompiler.cpp : 11363";

fn operator_rows() -> Vec<Row> {
    vec![
        row("body-P82", "void F()\n{\n\tstring s = \"a\" - \"b\";\n}\n", &[(3, "Unknown operator '-'")]),
        row("body-P83", "void F()\n{\n\tstring s = 5 + \"a\";\n}\n", &[(3, "Incompatible parameter '5'")]),
        row("body-P84", "void F()\n{\n\tstring s = \"a\" + 5;\n}\n", &[]),
        row(
            "body-P85",
            "void F(vector v)\n{\n\tvector w = v / 2.0;\n}\n",
            &[(3, "Incompatible parameter '2.0'")],
        ),
        row("body-P86", "void F(vector v)\n{\n\tvector w = 2.0 * v;\n}\n", &[]),
        row("body-P87", "void F(vector a, vector b)\n{\n\tfloat d = a * b;\n}\n", &[]),
        row(
            "body-P76",
            "void F()\n{\n\tint a;\n\tint b;\n\tint c;\n\ta = b = c;\n}\n",
            &[(6, "Unknown operator '='")],
        ),
        row("body-P119", "void F()\n{\n\tint a[3];\n\tint b = a + 1;\n}\n", &[(4, "Mixed value and array")]),
        row(
            "body-P108",
            "void F()\n{\n\tstring s = (1 == 2);\n}\n",
            &[(3, "Can't condition to non numeric type")],
        ),
        row(
            "body-P109",
            "void F(int a)\n{\n\tstring s = a && a;\n}\n",
            &[(3, "Can't condition to non numeric type")],
        ),
        row(
            "body-P74",
            "void V()\n{\n}\nvoid F()\n{\n\tif (V())\n\t{\n\t}\n}\n",
            &[(6, "Can't condition 'void' type")],
        ),
        row("body-P106", "void F()\n{\n\tstring s;\n\tstring t = -s;\n}\n", &[(4, "Bad type 's'")]),
        row(
            "body-P107",
            "void F()\n{\n\tfloat f = ~1;\n}\n",
            &[(3, "'~' : illegal on operands of type 'float'")],
        ),
        row(
            "body-P88",
            "void F(array<int> a)\n{\n\ta[0] += 1;\n}\n",
            &[(3, "Operator '+=' not supported with array accessor [] (TODO)")],
        ),
        row(
            "body-P89",
            "void F(array<int> a)\n{\n\ta[0]++;\n}\n",
            &[(3, "Operator '++' is not supported on temporary values (TODO)")],
        ),
        // Only a subscript on a container class is known to be a temporary.
        row("static-array-step", "void F()\n{\n\tint a[3];\n\ta[1]++;\n}\n", &[]),
        row("body-P111", "void F()\n{\n\tstring s;\n\ts -= \"a\";\n}\n", &[(4, ICE)]),
        row("body-P16", "void F()\n{\n\tbool b = (map == 8);\n}\n", &[(3, "Incompatible parameter '8'")]),
        row(
            "body-P97",
            "class PrA\n{\n}\nvoid F(PrA a)\n{\n\tif (a == 0)\n\t{\n\t}\n}\n",
            &[(6, "Incompatible parameter '0'")],
        ),
        row(
            "body-P35",
            "class PrA\n{\n}\nclass PrB\n{\n}\nvoid F(PrA a, PrB b)\n{\n\tbool r = (a == b);\n}\n",
            &[],
        ),
        row("body-P79", "void F(string s)\n{\n\tif (s)\n\t{\n\t}\n}\n", &[]),
    ]
}

#[test]
fn an_operator_its_operands_do_not_support_is_reported() {
    assert_rows(Target::RetailServer, &operator_rows());
}

// The engine declares its handles as `typedef int[] Name;` beside a class of
// the same name, and vanilla compares them with numbers.
#[test]
fn an_engine_handle_compares_with_a_number() {
    let text = "typedef int[] PrHandle;\nclass PrHandle\n{\n}\nPrHandle G();\nvoid F(PrHandle h)\n{\n\tif (h == 0)\n\t{\n\t}\n\tif (G() == 0x0)\n\t{\n\t}\n}\n";
    assert_rows(Target::RetailServer, &[row("handle", text, &[])]);
}

// Shapes the vanilla scripts rely on, which must stay silent.
fn vanilla_rows() -> Vec<Row> {
    vec![
        row("literal-vector-sum", "void F(vector p)\n{\n\tvector v = \"0 0.5 0\" + p;\n}\n", &[]),
        row(
            "literal-vector-scale",
            "void F(vector p, array<vector> a)\n{\n\ta.Insert(p + (\"0 0 1\" * 2.5));\n}\n",
            &[],
        ),
        row(
            "class-named-like-an-enum-value",
            "class PrFlare\n{\n}\nenum PrKind\n{\n\tPrFlare\n}\nbool Is(typename type);\nvoid F()\n{\n\tIs(PrFlare);\n}\n",
            &[],
        ),
    ]
}

#[test]
fn shapes_vanilla_compiles_stay_silent() {
    assert_rows(Target::RetailServer, &vanilla_rows());
}

type Lines = &'static [(u32, &'static str)];

fn class_rows(strict: bool) -> Vec<Row> {
    // The retail targets print a down-cast as one warning; the Diag targets
    // make it an error and print the rows after it.
    let (as_argument, as_cast, as_value): (Lines, Lines, Lines) = match strict {
        false => (
            &[(12, "FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting")],
            &[(9, "FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting")],
            &[(9, "FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting")],
        ),
        true => (
            &[
                (12, "Unsafe down-casting, use 'PrB.Cast' for safe down-casting"),
                (12, "Types 'PrA' and 'PrB' are unrelated"),
                (12, "Cannot convert 'PrA' to 'PrB' for argument '0' to method 'G'"),
            ],
            &[
                (9, "Unsafe down-casting, use 'PrB.Cast' for safe down-casting"),
                (9, "Types 'PrA' and 'PrB' are unrelated"),
            ],
            &[
                (9, "Unsafe down-casting, use 'PrB.Cast' for safe down-casting"),
                (9, "Types 'PrA' and 'PrB' are unrelated"),
                (9, "Incompatible parameter 'b'"),
            ],
        ),
    };
    vec![
        row(
            "body-P22",
            "void G(string s)\n{\n}\nvoid F()\n{\n\tG(5);\n}\n",
            &[(6, "Incompatible parameter '5'")],
        ),
        row(
            "body-P23",
            "class PrA\n{\n}\nclass PrB\n{\n}\nvoid G(PrA a)\n{\n}\nvoid F(PrB b)\n{\n\tG(b);\n}\n",
            &[
                (12, "Types 'PrB' and 'PrA' are unrelated"),
                (12, "Cannot convert 'PrB' to 'PrA' for argument '0' to method 'G'"),
            ],
        ),
        row(
            "body-P24",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid G(PrB b)\n{\n}\nvoid F(PrA a)\n{\n\tG(a);\n}\n",
            as_argument,
        ),
        row(
            "body-P25",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid F(PrA a)\n{\n\tPrB b = (PrB)a;\n}\n",
            as_cast,
        ),
        row(
            "body-P98",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid F(PrA a)\n{\n\tPrB b = a;\n}\n",
            as_value,
        ),
        row(
            "up-cast",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid G(PrA a)\n{\n}\nvoid F(PrB b)\n{\n\tPrA a = b;\n\tG(b);\n\ta = null;\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_class_value_must_derive_from_its_destination() {
    assert_rows(Target::RetailServer, &class_rows(false));
    assert_rows(Target::DiagServer, &class_rows(true));
}

fn write_rows() -> Vec<Row> {
    vec![
        row(
            "body-P99",
            "const int K = 1;\nvoid F()\n{\n\tK = 2;\n}\n",
            &[(4, "Trying to modify write protected variable 'K'")],
        ),
        row(
            "body-P120",
            "const int K = 1;\nvoid F()\n{\n\tK++;\n}\n",
            &[(4, "Trying to modify write protected variable 'K'")],
        ),
        row(
            "body-P40",
            "const int K = 1;\nvoid G(out int a)\n{\n}\nvoid F()\n{\n\tG(K);\n}\n",
            &[(7, "Trying to modify write protected variable 'K'")],
        ),
        row(
            "body-P39",
            "void G(out int a)\n{\n}\nvoid F()\n{\n\tG(5);\n}\n",
            &[(6, "Trying to modify write protected variable '(null)'")],
        ),
        row(
            "out-variable",
            "void G(out int a, inout int b)\n{\n}\nvoid F()\n{\n\tint x;\n\tint y;\n\tG(x, y);\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_const_variable_takes_no_new_value() {
    assert_rows(Target::RetailServer, &write_rows());
}

// body-P120: `K++` on a const prints the error and the module still loads; an assignment fails it.
#[test]
fn an_increment_of_a_const_does_not_fail_the_module() {
    for (statement, fails) in [("K++;", false), ("K = 2;", true)] {
        let text = format!("const int K = 1;\nvoid F()\n{{\n\t{statement}\n}}\n");
        let result = check(&input(Target::RetailServer, &[("Scripts/3_Game/a.c", &text)]));
        assert_eq!(result.diagnostics.len(), 1, "{statement}");
        assert_eq!(!result.failed_modules.is_empty(), fails, "{statement}");
    }
}

// Each override is held to the return type of the overload it replaces, not of
// the last one with the same number of parameters.
#[test]
fn an_override_is_held_to_the_overload_with_its_parameter_types() {
    let text = "class PrFoo\n{\n}\nclass PrBar\n{\n}\nclass PrP\n{\n\tPrFoo Make(int a)\n\t{\n\t\treturn null;\n\t}\n\tPrBar Make(string a)\n\t{\n\t\treturn null;\n\t}\n}\n\
        class PrQ : PrP\n{\n\toverride PrFoo Make(int a)\n\t{\n\t\treturn new PrFoo();\n\t}\n\toverride PrBar Make(string a)\n\t{\n\t\treturn new PrBar();\n\t}\n}\n";
    assert_rows(Target::RetailServer, &[row("override-overloads", text, &[])]);
}

fn cast_rows() -> Vec<Row> {
    vec![
        row(
            "body-P37",
            "class PrA\n{\n}\nclass PrB\n{\n}\nvoid F(PrA a)\n{\n\tPrB b = PrB.Cast(a);\n}\n",
            &[(9, "Types 'PrB' and 'PrA' are not related")],
        ),
        row(
            "body-P38",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid F(PrB b)\n{\n\tPrA a = PrA.Cast(b);\n}\n",
            &[(9, "No need to use 'Cast' for up-casting")],
        ),
        row(
            "down-cast",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nvoid F(PrA a)\n{\n\tPrB b = PrB.Cast(a);\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_cast_between_classes_follows_their_ancestry() {
    assert_rows(Target::RetailServer, &cast_rows());
}

fn strong_ref_rows() -> Vec<Row> {
    vec![
        row(
            "body-P100",
            "class PrB extends Managed\n{\n}\nclass PrA\n{\n\tPrB m;\n\tvoid M()\n\t{\n\t\tm = new PrB();\n\t}\n}\n",
            &[(9, "Variable 'm' is not strong ref (missing 'ref'?) !")],
        ),
        row(
            "body-P125",
            "class PrP\n{\n}\nclass PrA\n{\n\tPrP m;\n\tvoid M()\n\t{\n\t\tm = new PrP();\n\t}\n}\n",
            &[(9, "Variable 'm' is not strong ref (missing 'ref'?) !")],
        ),
        row(
            "body-P101",
            "class PrA\n{\n\tarray<int> m;\n\tvoid M()\n\t{\n\t\tm = {1};\n\t}\n}\n",
            &[(6, "Variable 'm' is not strong ref (missing autoptr?) !")],
        ),
        row(
            "body-P102",
            "class PrB extends Managed\n{\n}\nvoid F()\n{\n\tPrB b = new PrB();\n}\n",
            &[],
        ),
        // Vanilla fills a static array member from a list and builds its game
        // object into a plain global.
        row(
            "static-array-member",
            "class PrA\n{\n\tfloat m[2];\n\tvoid M()\n\t{\n\t\tm = {1, 2};\n\t}\n}\n",
            &[],
        ),
        row(
            "member-through-this",
            "class PrP\n{\n}\nclass PrA\n{\n\tPrP m;\n\tref PrP r;\n\tvoid M(PrA other)\n\t{\n\t\tthis.m = new PrP();\n\t\tother.r = new PrP();\n\t}\n}\n",
            &[(10, "Variable 'm' is not strong ref (missing 'ref'?) !")],
        ),
        row(
            "ref-member",
            "class PrP\n{\n}\nclass PrA\n{\n\tref PrP m;\n\tautoptr PrP n;\n\tref array<int> a;\n\tvoid M(out PrP o)\n\t{\n\t\tm = new PrP();\n\t\tn = new PrP();\n\t\ta = {1};\n\t\to = new PrP();\n\t}\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_new_object_needs_a_strong_reference_to_hold_it() {
    assert_rows(Target::RetailServer, &strong_ref_rows());
}

fn index_rows() -> Vec<Row> {
    vec![
        row(
            "body-P115",
            "void F()\n{\n\tint a[3];\n\ta[5] = 1;\n}\n",
            &[(4, "Index out of range (3 >= 5) 'a'"), (4, "Incompatible parameter 'a'")],
        ),
        row(
            "body-P116",
            "void F()\n{\n\tint a[3];\n\tint b = a[3];\n}\n",
            &[(4, "Index out of range (3 >= 3) 'a'")],
        ),
        // Vanilla writes a subscript as a statement of its own, which reads
        // nothing.
        row("unused-subscript", "void F()\n{\n\tfloat q[4];\n\tq[4];\n}\n", &[]),
        row("in-range", "void F(int n)\n{\n\tint a[3];\n\ta[2] = 1;\n\tint b = a[0];\n\tb = a[n];\n}\n", &[]),
    ]
}

#[test]
fn a_constant_index_past_a_static_array_is_reported() {
    assert_rows(Target::RetailServer, &index_rows());
}

const UNKNOWN_CALLBACK: &str = "void F(ScriptCallQueue queue)\n{\n\tqueue.CallLater(Nope, 100);\n}\n";

// The retail targets warn twice and then fail on the name itself; the Diag
// targets stop at the two rows, now errors.
#[test]
fn an_unknown_name_passed_as_a_callback_is_reported() {
    assert_rows(
        Target::RetailServer,
        &[row(
            "body-P114",
            UNKNOWN_CALLBACK,
            &[
                (3, "FIX-ME: Can't make callback from unknown method 'Nope'"),
                (3, "FIX-ME: Failed to create callback"),
                (3, "Can't find variable 'Nope'"),
            ],
        )],
    );
    assert_rows(
        Target::DiagServer,
        &[row(
            "body-P114",
            UNKNOWN_CALLBACK,
            &[
                (3, "Can't make callback from unknown method 'Nope'"),
                (3, "Failed to create callback"),
            ],
        )],
    );
}

fn initialiser_rows() -> Vec<Row> {
    vec![
        // `__LINE__` is text to the game, as `__FILE__` is.
        row(
            "syn-066",
            "class PrA { string f = __FILE__; int l = __LINE__; }\n",
            &[(1, "Incompatible parameter 'l'")],
        ),
        row("global", "int g_Count = \"a\";\n", &[(1, "Incompatible parameter 'g_Count'")]),
        row(
            "member-names",
            "class PrA\n{\n\tint m = nope;\n}\n",
            &[(3, "Can't find variable 'nope'")],
        ),
        // The engine registers the float limits; no script declares them.
        row(
            "engine-limits",
            "class PrA\n{\n\tconst float MAX = FLT_MAX;\n\tconst float LOWEST = -FLT_MIN;\n}\n",
            &[],
        ),
        row(
            "initialisers-that-fit",
            "class PrP\n{\n}\nconst int LIMIT = 5;\nclass PrA\n{\n\tconst int K = LIMIT;\n\tstatic const float F = 1;\n\tstring s = \"x\";\n\tref array<int> a = new array<int>;\n\tref PrP p = new PrP();\n\tint n = K;\n}\n",
            &[],
        ),
    ]
}

#[test]
fn an_initialiser_outside_a_function_is_checked_like_one_inside() {
    assert_rows(Target::RetailServer, &initialiser_rows());
}

// `.5` is the operator `.` then `5`, and `%=` is `%` then `=`: both parse, and
// the body pass finds an operator where an operand belongs.
#[test]
fn an_operator_in_an_operand_position_is_a_syntax_error() {
    assert_rows(
        Target::RetailServer,
        &[
            row("syn-007", "class PrA { void F() { float f = .5; } }\n", &[(1, "Syntax error")]),
            row("syn-016", "class PrA { void F() { int x = 1; x %= 2; } }\n", &[(1, "Syntax error")]),
        ],
    );
}

// The name the game gives a returned value is taken from its message table and
// not from a probe.
fn return_rows() -> Vec<Row> {
    vec![
        row("string-as-int", "int F()\n{\n\treturn \"a\";\n}\n", &[(3, "Incompatible parameter '#return'")]),
        row("number-as-string", "string F()\n{\n\treturn 5;\n}\n", &[(3, "Incompatible parameter '5'")]),
        row(
            "returns-that-fit",
            "class PrA\n{\n}\nclass PrB extends PrA\n{\n}\nPrA F(PrB b)\n{\n\treturn b;\n}\nfloat G()\n{\n\treturn 1;\n}\nvector H()\n{\n\treturn \"0 0 0\";\n}\nPrA N()\n{\n\treturn null;\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_returned_value_must_become_the_return_type() {
    assert_rows(Target::RetailServer, &return_rows());
}

fn constructor_rows() -> Vec<Row> {
    vec![
        row(
            "too-few",
            "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA();\n}\n",
            &[(9, "Not enough parameters in function 'PrA'")],
        ),
        row(
            "none-declared",
            "class PrA\n{\n}\nvoid F()\n{\n\tPrA a = new PrA(1);\n}\n",
            &[(6, "Too many parameters for 'PrA' method")],
        ),
        row(
            "wrong-type",
            "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nvoid F(string s)\n{\n\tPrA a = new PrA(s);\n}\n",
            &[(9, "Cannot convert 'string' to 'int' for argument '0' to method 'PrA'")],
        ),
        // A `modded` layer is built through the constructor of the class it
        // wraps.
        row(
            "modded-layer",
            "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nmodded class PrA\n{\n}\nvoid F()\n{\n\tPrA a = new PrA(1);\n}\n",
            &[],
        ),
        row(
            "constructors-that-fit",
            "class PrA\n{\n\tvoid PrA(int a, int b = 2)\n\t{\n\t}\n}\nclass PrB extends PrA\n{\n}\nclass PrBox<Class T>\n{\n\tvoid PrBox(T value)\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA(1);\n\tPrB b = new PrB(1, 2);\n\tPrBox<string> c = new PrBox<string>(\"x\");\n\tarray<int> d = new array<int>;\n}\n",
            &[],
        ),
    ]
}

#[test]
fn a_new_object_is_built_with_the_arguments_its_constructor_takes() {
    assert_rows(Target::RetailServer, &constructor_rows());
}

// A line printed after the game crashed is dropped, and must not soften the crash.
#[test]
fn an_increment_of_a_const_after_a_crash_keeps_the_crash_failing_the_module() {
    let text = "const int K = 1;\nvoid F()\n{\n\tint a = 5 / 0;\n\tK++;\n}\n";
    let result = check(&input(Target::RetailServer, &[("Scripts/3_Game/a.c", text)]));
    assert!(result.diagnostics.iter().any(|d| d.game_crash), "{:?}", result.diagnostics);
    assert!(result.diagnostics.iter().all(|d| !d.reports_only), "{:?}", result.diagnostics);
    assert!(!result.failed_modules.is_empty());
}

// sz-01 to sz-06: a parameter written `T name[N]` takes a static array of `T`
// of any size, on a global function and on a method alike.
#[test]
fn a_sized_parameter_takes_a_static_array_of_its_element_type() {
    let arrays = "\tstring a[2];\n\tstring b[3];\n\tint c[2];\n\tarray<string> d = new array<string>;\n";
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "sz-01",
                "void G(string s[2])\n{\n}\nvoid F()\n{\n\tG(5);\n\tG(\"x\");\n}\n",
                &[
                    (6, "Incompatible parameter '5'"),
                    (7, "Incompatible parameter '(null)'"),
                    (7, "Cannot convert 'string' to 'string' for argument '0' to method 'G'"),
                ],
            ),
            row(
                "sz-02",
                support::text(&[
                    "void G(string s[2])\n{\n\tstring x = s[0];\n}\nvoid F()\n{\n",
                    arrays,
                    "\tG(a);\n\tG(b);\n\tG(c);\n\tG(d);\n}\n",
                ]),
                &[
                    (13, "Cannot convert 'int' to 'string' for argument '0' to method 'G'"),
                    (14, "Incompatible parameter 'd'"),
                    (14, "Cannot convert 'array<string>' to 'string' for argument '0' to method 'G'"),
                ],
            ),
            row(
                "sz-03",
                "class PrA\n{\n\tvoid M(string s[2])\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA;\n\ta.M(5);\n\ta.M(\"x\");\n}\n",
                &[
                    (10, "Incompatible parameter '5'"),
                    (11, "Incompatible parameter '(null)'"),
                    (11, "Cannot convert 'string' to 'string' for argument '0' to method 'M'"),
                ],
            ),
            row(
                "sz-04",
                support::text(&[
                    "class PrA\n{\n\tvoid M(string s[2])\n\t{\n\t\tstring x = s[0];\n\t}\n}\nvoid F()\n{\n\tPrA p = new PrA;\n",
                    arrays,
                    "\tp.M(a);\n\tp.M(b);\n\tp.M(c);\n\tp.M(d);\n}\n",
                ]),
                &[
                    (17, "Cannot convert 'int' to 'string' for argument '0' to method 'M'"),
                    (18, "Incompatible parameter 'd'"),
                    (18, "Cannot convert 'array<string>' to 'string' for argument '0' to method 'M'"),
                ],
            ),
            row("sz-05", "void G(int n[])\n{\n}\nvoid F()\n{\n\tint a[2];\n\tG(a);\n}\n", &[]),
            row(
                "sz-06",
                "int H()\n{\n\treturn 1;\n}\nvoid G(string s[2])\n{\n}\nvoid F()\n{\n\tG(-5);\n\tG(H());\n}\n",
                &[
                    (10, "Incompatible parameter '-5'"),
                    (11, "Incompatible parameter '#return'"),
                    (11, "Cannot convert 'int' to 'string' for argument '0' to method 'G'"),
                ],
            ),
            // Vanilla sorts a dynamic array through `Sort(void param_array[], int num)`.
            row(
                "void-element",
                "proto void Sort(void param_array[], int num);\nvoid F()\n{\n\tarray<int> d = new array<int>;\n\tSort(d, 1);\n}\n",
                &[],
            ),
        ],
    );
}

// sz-07 to sz-17: an array declared in a function body, of a class written in
// script from the roots up, is named and refused by a parameter of its class.
#[test]
fn a_local_array_of_a_script_class_fits_no_parameter() {
    const LOCALS: &str = "void F()\n{\n\tPrA c[2];\n\tPrB b[2];\n\tG(c);\n\tG(b);\n}\n";
    const REFUSED: &[(u32, &str)] = &[
        (14, "Incompatible parameter 'c'"),
        (14, "Cannot convert 'PrA' to 'PrA' for argument '0' to method 'G'"),
        (15, "Incompatible parameter 'b'"),
        (15, "Cannot convert 'PrB' to 'PrA' for argument '0' to method 'G'"),
    ];
    let classes = |base: &str| format!("class PrA{base}\n{{\n}}\nclass PrB : PrA\n{{\n}}\n");
    let global = |base: &str, brackets: &str| support::text(&[&classes(base), "void G(PrA a", brackets, ")\n{\n}\n", LOCALS]);
    assert_rows(
        Target::RetailServer,
        &[
            row("sz-07", global("", "[2]"), REFUSED),
            row("sz-09", global("", "[]"), REFUSED),
            row(
                "sz-10",
                support::text(&[
                    &classes(""),
                    "class PrH\n{\n\tvoid M(PrA a[2])\n\t{\n\t}\n\tvoid F()\n\t{\n\t\tPrA c[2];\n\t\tPrB b[2];\n\t\tM(c);\n\t\tM(b);\n\t}\n}\n",
                ]),
                &[
                    (16, "Incompatible parameter 'c'"),
                    (16, "Cannot convert 'PrA' to 'PrA' for argument '0' to method 'M'"),
                    (17, "Incompatible parameter 'b'"),
                    (17, "Cannot convert 'PrB' to 'PrA' for argument '0' to method 'M'"),
                ],
            ),
            row("sz-12", global(" : Managed", "[2]"), REFUSED),
            // A class with an engine class among its ancestors passes; the
            // test below has the shape sz-13 and sz-14 ran.
            row("engine-ancestor", global(" : ScriptCallQueue", "[2]"), &[]),
            row(
                "sz-15",
                "class PrA\n{\n}\nvoid G(PrA a[2])\n{\n}\nvoid F()\n{\n\tref PrA c[2];\n\tG(c);\n}\n",
                &[
                    (10, "Incompatible parameter 'c'"),
                    (10, "Cannot convert 'PrA' to 'PrA' for argument '0' to method 'G'"),
                ],
            ),
            // A parameter of the class itself refuses it too; reading and
            // writing an element is fine.
            row(
                "sz-17",
                "class PrA\n{\n}\nvoid H(PrA p)\n{\n}\nvoid F()\n{\n\tPrA c[2];\n\tH(c);\n\tPrA x = c[0];\n\tc[1] = x;\n}\n",
                &[
                    (10, "Incompatible parameter 'c'"),
                    (10, "Cannot convert 'PrA' to 'PrA' for argument '0' to method 'H'"),
                ],
            ),
            // Unobserved: a sized parameter handed on is not held to it.
            row(
                "parameter-handed-on",
                "class PrA\n{\n}\nvoid G(PrA a[2])\n{\n}\nvoid F(PrA p[2])\n{\n\tG(p);\n}\n",
                &[],
            ),
            // Unobserved: nothing is said to a parameter of another class or of a container.
            row(
                "other-class-parameter",
                "class PrA\n{\n}\nclass PrC\n{\n}\nvoid H(PrC p)\n{\n}\nvoid F()\n{\n\tPrA c[2];\n\tH(c);\n}\n",
                &[],
            ),
            row(
                "container-parameter",
                "class PrA\n{\n}\nvoid H(array<PrA> p)\n{\n}\nvoid F()\n{\n\tPrA c[2];\n\tH(c);\n}\n",
                &[],
            ),
        ],
    );
}

// sz-13 and sz-14: a class with an engine class among its ancestors is not
// held to it, although that class is itself built on `Managed` (vanilla's
// `IEntity`, and with it every `ItemBase`).
#[test]
fn a_local_array_of_an_engine_built_class_fits_a_sized_parameter() {
    let result = enforce_check::check(&support::input(
        Target::RetailServer,
        &[
            (support::ENGINE, support::ENGINE_TEXT),
            ("Scripts/1_Core/proto/entity.c", "class IEntity : Managed\n{\n}\n"),
            (
                support::PROBE,
                "class PrA : IEntity\n{\n}\nclass PrB : PrA\n{\n}\nvoid G(PrA a[2])\n{\n}\nvoid F()\n{\n\tPrA c[2];\n\tPrB b[2];\n\tG(c);\n\tG(b);\n}\n",
            ),
        ],
    ));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// sz-07 to sz-17: an array of a class, or of a class derived from it, fits a
// sized parameter when it is a member or a global.
#[test]
fn a_class_array_fits_a_sized_parameter_of_its_class_or_an_ancestor() {
    const CLASSES: &str = "class PrA\n{\n}\nclass PrB : PrA\n{\n}\n";
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "sz-08",
                support::text(&[
                    CLASSES,
                    "void G(PrA a[2])\n{\n}\nclass PrH\n{\n\tPrA m_c[2];\n\tPrB m_b[2];\n\tvoid F()\n\t{\n\t\tG(m_c);\n\t\tG(m_b);\n\t}\n}\n",
                ]),
                &[],
            ),
            row(
                "sz-11",
                support::text(&[
                    CLASSES,
                    "class PrH\n{\n\tPrA m_c[2];\n\tPrB m_b[2];\n\tvoid M(PrA a[])\n\t{\n\t}\n\tvoid F()\n\t{\n\t\tM(m_c);\n\t\tM(m_b);\n\t}\n}\n",
                ]),
                &[],
            ),
            row(
                "sz-16",
                support::text(&[CLASSES, "PrA g_c[2];\nPrB g_b[2];\nvoid G(PrA a[2])\n{\n}\nvoid F()\n{\n\tG(g_c);\n\tG(g_b);\n}\n"]),
                &[],
            ),
            // Unobserved: an array of an ancestor, or of an unrelated class, is
            // refused like an array of any other type.
            row(
                "ancestor-array",
                support::text(&[
                    CLASSES,
                    "class PrC\n{\n}\nvoid G(PrB a[2])\n{\n}\nclass PrH\n{\n\tPrA m_c[2];\n\tPrC m_d[2];\n\tvoid F()\n\t{\n\t\tG(m_c);\n\t\tG(m_d);\n\t}\n}\n",
                ]),
                &[
                    (19, "Cannot convert 'PrA' to 'PrB' for argument '0' to method 'G'"),
                    (20, "Cannot convert 'PrC' to 'PrB' for argument '0' to method 'G'"),
                ],
            ),
        ],
    );
}
