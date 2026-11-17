mod support;

use enforce_check::{Target, check};
use support::{ENGINE, ENGINE_TEXT, Row, assert_rows, input, row};

fn static_method_rows() -> Vec<Row> {
    vec![
        row(
            "ord-007",
            "class ProbeS\n{\n\tstatic void F()\n\t{\n\t\tProbeS s = this;\n\t}\n}\n",
            &[
                (5, "'this' or 'super' inside static method is not valid"),
                (5, "Can't find variable 'this'"),
            ],
        ),
        row(
            "ord-008",
            "class ProbeB\n{\n\tvoid M()\n\t{\n\t}\n}\n\nclass ProbeS : ProbeB\n{\n\tstatic void F()\n\t{\n\t\tsuper.M();\n\t}\n}\n",
            &[(12, "Trying to call non-static function 'M' as static")],
        ),
        // A `void` parameter of an engine function takes `this` as it is.
        row("body-P113", "class PrA\n{\n\tstatic void S()\n\t{\n\t\tPrint(this);\n\t}\n}\n", &[]),
        row(
            "static-super-to-static",
            "class ProbeB\n{\n\tstatic void M()\n\t{\n\t}\n}\n\nclass ProbeS : ProbeB\n{\n\tstatic void F()\n\t{\n\t\tsuper.M();\n\t}\n}\n",
            &[],
        ),
    ]
}

#[test]
fn this_and_super_in_a_static_method_follow_the_game() {
    assert_rows(Target::RetailServer, &static_method_rows());
}

// body-P14: a class that is only declared ahead is no type a value can name.
#[test]
fn a_forward_declared_class_is_not_a_value() {
    assert_rows(
        Target::RetailServer,
        &[row(
            "body-P14",
            "class Fwd;\nvoid F()\n{\n\ttypename t = Fwd;\n}\n",
            &[(1, "Forward declarations are deprecated!"), (4, "Can't find variable 'Fwd'")],
        )],
    );
}

// syn-041 and syn-035: a line the parser only warned about is still compiled.
#[test]
fn a_line_the_parser_warned_about_still_has_its_body_checked() {
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "syn-041",
                "class PrA\n{\n\tvoid F()\n\t{\n\t\tint x = (int) \n ;\n\t}\n}\n",
                &[
                    (5, "Missing ';' at the end of line"),
                    (5, "Broken expression (missing ';'?)"),
                    (5, "Can't find variable 'int'"),
                ],
            ),
            row(
                "syn-035",
                "class PrA\n{\n\tvoid F()\n\t{\n\t\tvoid G() {}\n\t}\n}\n",
                &[(5, "Broken expression (missing ';'?)"), (5, "Can't find class void")],
            ),
        ],
    );
}

// What may reach a private or protected member, as the game showed it:
// private is the declaring class alone, except that a class with a template
// instance among its ancestors reads its inherited privates by their bare
// name; a protected field asks for kinship with the declaring class, a
// protected method for kinship with the class it is called on.
fn access_rows() -> Vec<Row> {
    vec![
        // Private script method of a template base, called from a subclass.
        row("asm-D24b", "class PrT<Class T>\n{\n\tprivate int P()\n\t{\n\t\treturn 1;\n\t}\n}\nclass PrU : PrT<int>\n{\n\tint M()\n\t{\n\t\treturn P();\n\t}\n}\n", &[]),
        // Protected method of the shared base, receiver is a sibling class.
        row("asm-D30a", "class PrA\n{\n\tprotected void P()\n\t{\n\t}\n}\nclass PrB : PrA\n{\n}\nclass PrC : PrA\n{\n\tvoid M(PrB b)\n\t{\n\t\tb.P();\n\t}\n}\n", &[(14, "Method 'P' is protected")]),
        // Protected method declared in a sibling class.
        row("asm-D30b", "class PrA\n{\n}\nclass PrB : PrA\n{\n\tprotected void P()\n\t{\n\t}\n}\nclass PrC : PrA\n{\n\tvoid M(PrB b)\n\t{\n\t\tb.P();\n\t}\n}\n", &[(14, "Method 'P' is protected")]),
        // Protected method, the two classes share only Managed.
        row("asm-D30c", "class PrB : Managed\n{\n\tprotected void P()\n\t{\n\t}\n}\nclass PrC : Managed\n{\n\tvoid M(PrB b)\n\t{\n\t\tb.P();\n\t}\n}\n", &[(11, "Method 'P' is protected")]),
        // Protected method, two classes with no base.
        row("asm-D30d", "class PrB\n{\n\tprotected void P()\n\t{\n\t}\n}\nclass PrC\n{\n\tvoid M(PrB b)\n\t{\n\t\tb.P();\n\t}\n}\n", &[(11, "Method 'P' is protected")]),
        // Protected method called on a parameter of the base class and of the caller's own class.
        row("asm-D30e", "class PrA\n{\n\tprotected void P()\n\t{\n\t}\n}\nclass PrC : PrA\n{\n\tvoid M(PrA a, PrC c)\n\t{\n\t\ta.P();\n\t\tc.P();\n\t\tP();\n\t}\n}\n", &[]),
        // Protected method called on a subclass of the caller, and from a base on its subclass's own protected method.
        row("asm-D30f", "class PrA\n{\n\tprotected void P()\n\t{\n\t}\n\tvoid M(PrC c)\n\t{\n\t\tc.P();\n\t\tc.Q();\n\t}\n}\nclass PrC : PrA\n{\n\tprotected void Q()\n\t{\n\t}\n}\n", &[]),
        // Protected field: sibling, base parameter, bare.
        row("asm-D30g", "class PrA\n{\n\tprotected int x;\n}\nclass PrB : PrA\n{\n}\nclass PrC : PrA\n{\n\tvoid M(PrA a, PrB b)\n\t{\n\t\tx = 1;\n\t\ta.x = 2;\n\t\tb.x = 3;\n\t}\n}\n", &[]),
        // Private field through obj.field from a subclass.
        row("asm-D35a", "class PrA\n{\n\tprivate int x;\n}\nclass PrB : PrA\n{\n\tvoid M(PrA a)\n\t{\n\t\ta.x = 1;\n\t}\n}\n", &[(9, "Variable 'x' is private"), (9, "Can't find variable 'x'")]),
        // Private field of a base read and written bare in a subclass.
        row("asm-D35b", "class PrA\n{\n\tprivate int x;\n}\nclass PrB : PrA\n{\n\tint M()\n\t{\n\t\tx = 2;\n\t\treturn x;\n\t}\n}\n", &[(9, "Variable 'x' is private"), (9, "Can't find variable 'x'"), (10, "Variable 'x' is private"), (10, "Can't find variable 'x'")]),
        // Private field of a base through this.x in a subclass.
        row("asm-D35c", "class PrA\n{\n\tprivate int x;\n}\nclass PrB : PrA\n{\n\tint M()\n\t{\n\t\treturn this.x;\n\t}\n}\n", &[(9, "Variable 'x' is private"), (9, "Can't find variable 'x'")]),
        // Private field of a class that was modded, read bare in a subclass.
        row("asm-D35d", "class PrA\n{\n\tprivate int x;\n}\nmodded class PrA\n{\n}\nclass PrB : PrA\n{\n\tint M()\n\t{\n\t\treturn x;\n\t}\n}\n", &[(12, "Variable 'x' is private"), (12, "Can't find variable 'x'")]),
        // Private field of a plain base, the subclass reaches it through a template instance.
        row("asm-D35e", "class PrA\n{\n\tprivate int x;\n}\nclass PrT<Class T> : PrA\n{\n}\nclass PrU : PrT<int>\n{\n\tint M()\n\t{\n\t\treturn x;\n\t}\n}\n", &[]),
        // Private field of a template base, read bare in a subclass.
        row("asm-D35f", "class PrT<Class T>\n{\n\tprivate int x;\n}\nclass PrU : PrT<int>\n{\n\tint M()\n\t{\n\t\treturn x;\n\t}\n}\n", &[]),
        // The vanilla shape: private members of a class built on a container, used in its subclass.
        row("asm-D35g", "class PrQ : array<int>\n{\n\tprivate int x;\n\tprivate void P()\n\t{\n\t}\n}\nclass PrR : PrQ\n{\n\tint M(PrQ other)\n\t{\n\t\tP();\n\t\tother.x = 2;\n\t\treturn x;\n\t}\n}\n", &[(13, "Variable 'x' is private"), (13, "Can't find variable 'x'")]),
        // Private member of a class built on a container, used from outside any class.
        row("asm-D35i", "class PrQ : array<int>\n{\n\tprivate int x;\n}\nvoid F(PrQ q)\n{\n\tq.x = 1;\n}\n", &[(7, "Variable 'x' is private"), (7, "Can't find variable 'x'")]),
        // A private engine method is private too, and a template instance in
        // the ancestry lets it through like a script one.
        row(
            "asm-D24c",
            "class PrBase
{
	private proto static bool S(Class a);
}
class PrC : PrBase
{
	void M()
	{
		S(this);
	}
}
",
            &[(9, "Method 'S' is private")],
        ),
        row(
            "asm-D24a",
            "class PrL<Class T>
{
	proto private native Class Get();
}
class PrLink : PrL<int>
{
	Class M()
	{
		return Get();
	}
}
",
            &[],
        ),
    ]
}

#[test]
fn private_and_protected_members_are_reached_as_the_game_allows() {
    assert_rows(Target::RetailServer, &access_rows());
}

// A returned value is held to the return type like a stored one, under the
// name `#return`.
fn return_rows() -> Vec<Row> {
    vec![
        // Return of an unrelated class.
        row("asm-D47a", "class PrA\n{\n}\nclass PrB\n{\n}\nPrB F(PrA a)\n{\n\treturn a;\n}\n", &[(9, "Types 'PrA' and 'PrB' are unrelated"), (9, "Incompatible parameter '#return'")]),
        // Return of a base where a derived class is declared.
        row("asm-D47b", "class PrA\n{\n}\nclass PrB : PrA\n{\n}\nPrB F(PrA a)\n{\n\treturn a;\n}\n", &[(9, "FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting")]),
        // String literal returned from an int function.
        row("asm-D48a", "int F()\n{\n\treturn \"a\";\n}\n", &[(3, "Incompatible parameter '#return'")]),
        // Number returned from a string function, and a string variable from an int one.
        row("asm-D48b", "string F()\n{\n\treturn 5;\n}\nint G(string s)\n{\n\treturn s;\n}\n", &[(3, "Incompatible parameter '5'"), (7, "Incompatible parameter '#return'")]),
        // Vanilla's `ParticleSource.Create`: an override keeps the return type
        // of the method it overrides, whatever narrower class it writes.
        row(
            "override-return",
            "class PrA
{
}
class PrB : PrA
{
}
class PrX
{
	static PrA Make()
	{
		return null;
	}
}
class PrY : PrX
{
	override static PrB Make()
	{
		PrA a;
		return a;
	}
}
",
            &[],
        ),
    ]
}

#[test]
fn a_returned_value_must_fit_the_return_type() {
    assert_rows(Target::RetailServer, &return_rows());
}

// A new object needs a strong reference wherever it is kept outside a
// function: a member, a static member or a global, assigned or initialised.
fn strong_reference_rows() -> Vec<Row> {
    vec![
        // New into a plain global with no ref.
        row("asm-D46a", "class PrP\n{\n}\nPrP g_PrP;\nvoid F()\n{\n\tg_PrP = new PrP();\n}\n", &[(7, "Variable 'g_PrP' is not strong ref (missing 'ref'?) !")]),
        // Member with no ref initialised with new where it is declared.
        row("asm-D46b", "class PrP\n{\n}\nclass PrA\n{\n\tPrP m = new PrP();\n}\n", &[(6, "Variable 'm' is not strong ref (missing 'ref'?) !")]),
        // New into a static member with no ref.
        row("asm-D46c", "class PrP\n{\n}\nclass PrA\n{\n\tstatic PrP s;\n\tstatic void S()\n\t{\n\t\ts = new PrP();\n\t}\n}\n", &[(9, "Variable 's' is not strong ref (missing 'ref'?) !")]),
        // Global with no ref initialised with new where it is declared.
        row("asm-D46d", "class PrP\n{\n}\nPrP g_PrP = new PrP();\n", &[(4, "Variable 'g_PrP' is not strong ref (missing 'ref'?) !"), (4, "Incompatible parameter 'PrP'")]),
        // New with no brackets into a global, a member and a static member, none with ref.
        row("asm-D46e", "class PrP\n{\n}\nPrP g_PrP;\nclass PrA\n{\n\tPrP m;\n\tstatic PrP s;\n\tvoid M()\n\t{\n\t\tm = new PrP;\n\t\ts = new PrP;\n\t\tg_PrP = new PrP;\n\t}\n}\n", &[(11, "Variable 'm' is not strong ref (missing 'ref'?) !"), (12, "Variable 's' is not strong ref (missing 'ref'?) !"), (13, "Variable 'g_PrP' is not strong ref (missing 'ref'?) !")]),
        // Member with no ref initialised with new and no brackets.
        row("asm-D46f", "class PrP\n{\n}\nclass PrA\n{\n\tPrP m = new PrP;\n}\n", &[(6, "Variable 'm' is not strong ref (missing 'ref'?) !")]),
        // Array members: a list into a static array member and into an array<int> with no ref.
        row("asm-D46g", "class PrA\n{\n\tfloat m_F[3];\n\tstatic float s_F[3];\n\tvoid M()\n\t{\n\t\tm_F = {1.0, 2.0, 3.0};\n\t\ts_F = {1.0, 2.0, 3.0};\n\t}\n}\n", &[]),
        // Member with no ref initialised with new, inside a modded class.
        row("asm-D46h", "class PrP\n{\n}\nclass PrA\n{\n}\nmodded class PrA\n{\n\tPrP m = new PrP;\n}\n", &[(9, "Variable 'm' is not strong ref (missing 'ref'?) !")]),
        // The vanilla shape with a plain class.
        row("asm-D46k", "class PrP\n{\n}\nPrP g_PrP;\nPrP PrMake()\n{\n\tg_PrP = new PrP;\n\treturn g_PrP;\n}\n", &[(7, "Variable 'g_PrP' is not strong ref (missing 'ref'?) !")]),
        // New container into a member with no ref.
        row("asm-D46l", "class PrA\n{\n\tarray<int> m;\n\tvoid M()\n\t{\n\t\tm = new array<int>;\n\t}\n}\n", &[(6, "Variable 'm' is not strong ref (missing 'ref'?) !")]),
        // Vanilla fills a vector member from a list; only a class is kept by
        // reference.
        row(
            "vector-from-list",
            "class PrA\n{\n\tprivate const vector V = { 0, 0, 0 };\n\tvector m_W;\n\tvoid M()\n\t{\n\t\tm_W = { 1, 2, 3 };\n\t}\n}\n",
            &[],
        ),
        // Vanilla builds its game object into a plain global.
        row(
            "asm-D46j",
            "class CGame
{
}
class PrG : CGame
{
}
PrG g_PrG;
PrG PrMake()
{
	g_PrG = new PrG;
	return g_PrG;
}
",
            &[],
        ),
    ]
}

#[test]
fn a_new_object_kept_outside_a_function_needs_a_strong_reference() {
    assert_rows(Target::RetailServer, &strong_reference_rows());
}

// lim-c17: a parameter is no strong reference either. A local is one, and an
// `out` parameter is filled this way all over vanilla.
#[test]
fn a_new_object_assigned_to_a_parameter_needs_a_strong_reference() {
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "lim-c17",
                "class PrP\n{\n}\nvoid F(PrP o)\n{\n\to = new PrP;\n}\n",
                &[(6, "Variable 'o' is not strong ref (missing 'ref'?) !")],
            ),
            row("out-parameter", "class PrP\n{\n}\nvoid F(out PrP o)\n{\n\to = new PrP;\n}\n", &[]),
            row("local", "class PrP\n{\n}\nvoid F()\n{\n\tPrP o;\n\to = new PrP;\n}\n", &[]),
        ],
    );
}

// asm-D46n, the Community Framework shape: a class declared in 1_Core is not
// held to the rule.
#[test]
fn a_class_declared_in_core_needs_no_strong_reference() {
    let files = [
        (ENGINE, ENGINE_TEXT),
        ("Scripts/1_Core/a.c", "class PrArgs
{
}
"),
        ("Scripts/3_Game/b.c", "class PrA
{
	PrArgs m = new PrArgs;
	PrArgs n;
	void M()
	{
		n = new PrArgs;
	}
}
"),
    ];
    assert_eq!(check(&input(Target::RetailServer, &files)).diagnostics, Vec::new());
}

// Operands that cannot stand beside each other: which of the two the game
// names depends on the operator and on which side holds the literal.
fn operator_rows() -> Vec<Row> {
    vec![
        // Operator rows, one per line.
        row("asm-D51a", "class PrA\n{\n}\nvoid F(PrA a, string s, vector v)\n{\n\tbool b1 = (1 == s);\n\tbool b2 = (s == 1);\n\tbool b3 = (1 == v);\n\tbool b4 = (v == 1);\n\tint i1 = a + 1;\n\tint i2 = 1 + a;\n\tstring s2 = 1 + s;\n\tvector v2 = v + 1.0;\n\tvector v3 = v / 2.0;\n\tfloat f1 = s * 2;\n}\n", &[(6, "Incompatible parameter '(null)'"), (7, "Incompatible parameter '1'"), (8, "Incompatible parameter '(null)'"), (9, "Incompatible parameter 'v'"), (10, "Incompatible parameter '1'"), (11, "Incompatible parameter 'a'"), (12, "Incompatible parameter '1'"), (13, "Incompatible parameter '1.0'"), (14, "Incompatible parameter '2.0'"), (15, "Incompatible parameter '2'")]),
        // Compound assignment rows, one per line.
        row("asm-D51b", "class PrA\n{\n}\nvoid F(PrA a, string s, vector v, bool b)\n{\n\ta += 1;\n\tv /= 2;\n\ts *= 2;\n\tb -= true;\n\ts += 1;\n}\n", &[(6, "Incompatible parameter '1'"), (7, "Incompatible parameter '@v00'"), (8, "Incompatible parameter '2'"), (10, "Incompatible parameter '1'")]),
    ]
}

#[test]
fn mismatched_operands_name_what_the_game_names() {
    assert_rows(Target::RetailServer, &operator_rows());
}

fn call_and_override_rows() -> Vec<Row> {
    vec![
        // This.Method(), this.field and super.field in a static method.
        row("stat-01", "class PrA\n{\n\tint x;\n\tvoid M()\n\t{\n\t}\n}\nclass PrB : PrA\n{\n\tint y;\n\tvoid N()\n\t{\n\t}\n\tstatic void S()\n\t{\n\t\tthis.N();\n\t\tint a = this.y;\n\t\tint b = super.x;\n\t\tPrB c = super;\n\t}\n}\n", &[(16, "Trying to call non-static function 'N' as static"), (17, "Variable 'y' is not static"), (18, "Variable 'x' is not static"), (19, "'this' or 'super' inside static method is not valid"), (19, "Can't find variable 'super'")]),
        // This passed to a script parameter and returned, in a static method.
        row("stat-02", "class PrB\n{\n\tstatic void G(PrB b)\n\t{\n\t}\n\tstatic PrB S()\n\t{\n\t\tG(this);\n\t\treturn this;\n\t}\n}\n", &[(8, "'this' or 'super' inside static method is not valid"), (8, "Can't find variable 'this'"), (9, "'this' or 'super' inside static method is not valid"), (9, "Can't find variable 'this'")]),
        // Private members through this. in a subclass of a class built on a container.
        row("asm-D35j", "class PrQ : array<int>\n{\n\tprivate int x;\n\tprivate void P()\n\t{\n\t}\n}\nclass PrR : PrQ\n{\n\tint M(PrQ other)\n\t{\n\t\tthis.P();\n\t\tother.P();\n\t\treturn this.x;\n\t}\n}\n", &[(13, "Method 'P' is private")]),
        // Base constructor called by name inside the subclass constructor.
        row("j14-01", "class PrA\n{\n\tvoid PrA()\n\t{\n\t}\n}\nclass PrB : PrA\n{\n\tvoid PrB()\n\t{\n\t\tPrA();\n\t}\n}\n", &[]),
        // Destructor called by name inside a method.
        row("j14-02", "class PrA\n{\n\tvoid ~PrA()\n\t{\n\t}\n\tvoid M()\n\t{\n\t\t~PrA();\n\t}\n}\n", &[(8, "Unknown type '~'"), (8, "Unknown type '~'"), (8, "Error in parameters")]),
        // Class name called like a function, with and without the right arguments.
        row("j14-03", "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = PrA(1);\n\tPrA b = PrA();\n}\n", &[(10, "Not enough parameters in function 'PrA'")]),
        // Own constructor called by name inside a method, and on an object.
        row("j14-06", "class PrA\n{\n\tvoid PrA()\n\t{\n\t}\n\tvoid M()\n\t{\n\t\tPrA();\n\t}\n}\n", &[]),
        // New with no argument against a one-parameter constructor.
        row("asm-D57a", "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA();\n}\n", &[(9, "Not enough parameters in function 'PrA'")]),
        // New of a subclass with no constructor: the base one takes one argument.
        row("asm-D57b", "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nclass PrB : PrA\n{\n}\nvoid F()\n{\n\tPrB b = new PrB();\n\tPrB c = new PrB(1);\n\tPrB d = new PrB(1, 2);\n}\n", &[(12, "Not enough parameters in function 'PrA'"), (14, "Too many parameters for 'PrA' method")]),
        // New with a wrong argument type.
        row("asm-D57c", "class PrA\n{\n\tvoid PrA(int a)\n\t{\n\t}\n}\nvoid F()\n{\n\tPrA a = new PrA(\"x\");\n}\n", &[(9, "Cannot convert 'string' to 'int' for argument '0' to method 'PrA'")]),
        // Unknown name as the argument of an overloaded call.
        row("asm-D31", "class PrC\n{\n\tvoid G(int a)\n\t{\n\t}\n\tvoid G(string a)\n\t{\n\t}\n\tvoid M()\n\t{\n\t\tG(nope);\n\t}\n}\n", &[(11, "Can't find variable 'nope'")]),
        // Enum value into an int parameter of an overloaded method.
        row("asm-D27", "enum PrEn\n{\n\tPRV\n}\nclass PrC\n{\n\tvoid G(int a)\n\t{\n\t}\n\tvoid G(string a)\n\t{\n\t}\n\tvoid M()\n\t{\n\t\tG(PrEn.PRV);\n\t}\n}\n", &[]),
        // Override changes bool to int.
        row("asm-D34a", "class PrA\n{\n\tbool M()\n\t{\n\t\treturn true;\n\t}\n}\nclass PrB : PrA\n{\n\toverride int M()\n\t{\n\t\treturn 1;\n\t}\n}\n", &[]),
        // Override changes int to float.
        row("asm-D34b", "class PrA\n{\n\tint M()\n\t{\n\t\treturn 1;\n\t}\n}\nclass PrB : PrA\n{\n\toverride float M()\n\t{\n\t\treturn 1.0;\n\t}\n}\n", &[(10, "Overloaded function 'M' not compatible")]),
        // Class and typedef of one name, overloads on both.
        row("asm-D25a", "class PrStr : string\n{\n\tstatic string Join(array<PrStr> a)\n\t{\n\t\treturn \"a\";\n\t}\n\tstatic string Join(array<string> a)\n\t{\n\t\treturn \"b\";\n\t}\n}\ntypedef string PrStr;\n", &[]),
        // Class and typedef of one name, which one a call picks.
        row("asm-D25b", "class PrStr : string\n{\n\tint Only()\n\t{\n\t\treturn 1;\n\t}\n}\ntypedef string PrStr;\nvoid F(PrStr s)\n{\n\tint a = s.Only();\n\tint b = s.Length();\n}\n", &[]),
        // Parameter default that is a sum.
        row("asm-D52", "void G(int a = 1 + 2)\n{\n}\n", &[]),
        // A type name is a value where a `Class` is wanted.
        row(
            "asm-D24c-typename",
            "class PrBase
{
	static bool S(Class type, Class from)
	{
		return true;
	}
}
class PrC : PrBase
{
	void M()
	{
		S(PrC, this);
	}
}
",
            &[],
        ),
    ]
}

#[test]
fn calls_constructors_and_overrides_print_what_the_game_prints() {
    assert_rows(Target::RetailServer, &call_and_override_rows());
}

// asm-D55a and asm-D55b: a method named as a callback.
#[test]
fn a_callback_names_one_method_the_caller_can_reach() {
    let overloaded = "class PrC
{
	void CB(int a)
	{
	}
	void CB(string a)
	{
	}
	void M(ScriptCallQueue q)
	{
		q.CallLater(CB, 100);
	}
}
";
    let no_instance = "class PrC
{
	void CB()
	{
	}
	static void S(ScriptCallQueue q)
	{
		q.CallLater(CB, 100);
	}
}
";
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "asm-D55a",
                overloaded,
                &[(11, "FIX-ME: Callbacks do not properly support overloaded methods 'CB'")],
            ),
            row(
                "asm-D55b",
                no_instance,
                &[
                    (8, "FIX-ME: Can't make callback from non-static function 'CB' without instance."),
                    (8, "Can't make callback from non-static function 'CB' without instance."),
                    (8, "FIX-ME: Failed to create callback"),
                    (8, "Can't find variable 'CB'"),
                ],
            ),
        ],
    );
}

// j14-04 and j14-05: a `proto` takes part in the duplicate rule of its class.
#[test]
fn a_proto_that_repeats_a_method_of_its_class_is_a_duplicate() {
    assert_rows(
        Target::RetailServer,
        &[
            row(
                "j14-04",
                "class PrA\n{\n\tproto native void P();\n\tproto native void P();\n}\n",
                &[(4, "Multiple declaration of function 'P'")],
            ),
            row(
                "j14-05",
                "class PrA\n{\n\tvoid P()\n\t{\n\t}\n\tproto native void P();\n}\n",
                &[(6, "Multiple declaration of function 'P'")],
            ),
            row(
                "proto-overloads",
                "class PrA\n{\n\tproto native void P();\n\tproto native void P(int a);\n\tproto void ~PrA();\n}\n",
                &[],
            ),
        ],
    );
}

// fwd-01: a parameter may take the name of a class that is only declared
// ahead, and a call on that name is then a call on the parameter.
#[test]
fn a_call_on_a_parameter_named_like_a_forward_declared_class_reaches_the_parameter() {
    assert_rows(
        Target::RetailServer,
        &[row(
            "fwd-01",
            "class PrFwd;\nclass PrHas\n{\n\tvoid M()\n\t{\n\t}\n}\nvoid F(PrHas PrFwd)\n{\n\tPrFwd.M();\n}\n",
            &[(1, "Forward declarations are deprecated!")],
        )],
    );
}

// asm-D46p to asm-D46s: the rule follows the class's ancestry. A class built on
// a plain 1_Core or 2_GameLib class is not held to it, wherever it is declared;
// one built on `Managed`, and a template, are.
#[test]
fn the_strong_reference_rule_follows_the_ancestry_of_the_class() {
    const MISSING: &str = "Variable 'm' is not strong ref (missing 'ref'?) !";
    let member = |type_: &str| support::text(&["class PrA\n{\n\t", type_, " m = new ", type_, ";\n}\n"]);
    let free: &[(u32, &str)] = &[];
    let held: &[(u32, &str)] = &[(3, MISSING)];
    let cases = [
        ("asm-D46p", "Scripts/1_Core/a.c", "class PrArgs\n{\n}\n", support::text(&["class PrArgs2 : PrArgs\n{\n}\n", member("PrArgs2")]), free),
        ("asm-D46q", "Scripts/2_GameLib/a.c", "class PrArgs\n{\n}\n", member("PrArgs"), free),
        ("asm-D46r", "Scripts/1_Core/a.c", "class PrArgs : Managed\n{\n}\n", member("PrArgs"), held),
        ("asm-D46s", "Scripts/1_Core/a.c", "class PrBox<Class T>\n{\n}\n", member("PrBox<int>"), held),
    ];
    for (id, early_path, early, game, expected) in cases {
        let files = [(ENGINE, ENGINE_TEXT), (early_path, early), ("Scripts/3_Game/b.c", game)];
        let result = check(&input(Target::RetailServer, &files));
        let got: Vec<(u32, &str)> = result.diagnostics.iter().map(|d| (d.line, d.message.as_str())).collect();
        assert_eq!(got, expected, "{id}");
    }
}

// lex-01 and num-01 to num-18: one digit before `.Name` is a name to the game,
// which no variable has; a longer digit run or a hex number takes the dot
// with it and is a broken expression unless digits or an exponent follow. A
// bracketed number and a string literal are values.
#[test]
fn a_method_call_on_a_bare_number_follows_the_game() {
    const NO_5: &str = "Can't find variable '5'";
    const BROKEN: &str = "Broken expression (missing ';'?)";
    assert_rows(
        Target::RetailServer,
        &[
            row("lex-01", "void F()\n{\n\tstring s = 5.ToString();\n}\n", &[(3, NO_5)]),
            row("num-01", "void F()\n{\n\tstring s = (5).ToString();\n}\n", &[]),
            row("num-02", "class PrJ\n{\n\tstatic const string BACKSPACE = (8).ToString();\n}\n", &[]),
            row(
                "num-03",
                "class PrJ\n{\n\tstatic const string BACKSPACE = 8.ToString();\n}\n",
                &[(3, "Can't find variable '8'"), (3, "Incompatible parameter '8'")],
            ),
            row("num-04", "void F()\n{\n\tint n = \"abc\".Length();\n}\n", &[]),
            row("num-05", "void F()\n{\n\tstring s;\n\ts = 5.ToString();\n}\n", &[(4, NO_5)]),
            row(
                "num-06",
                "void G(string s)\n{\n}\nvoid F()\n{\n\tPrint(5.ToString());\n\tG(5.ToString());\n}\n",
                &[(6, NO_5), (7, NO_5)],
            ),
            row("num-08", "void F()\n{\n\tint n = 5.ToString().Length();\n}\n", &[(3, NO_5)]),
            row("num-09", "string F()\n{\n\treturn 5.ToString();\n}\n", &[(3, NO_5)]),
            row(
                "num-10",
                "void F()\n{\n\t5.ToString();\n\tauto s = 5.ToString();\n}\n",
                &[(3, NO_5), (4, NO_5), (4, "Unknown type 'auto'"), (4, "Error in parameters")],
            ),
            row(
                "num-12",
                "void F()\n{\n\tarray<string> l = new array<string>;\n\tl.Insert(\"b:\" + 8632.ToString());\n}\n",
                &[(4, BROKEN)],
            ),
            row("num-13", "void F()\n{\n\tPrint(\"a\" + 5.ToString());\n}\n", &[(3, NO_5)]),
            row("num-14", "void F()\n{\n\tstring s = \"a\" + 5.ToString();\n}\n", &[(3, NO_5)]),
            row("num-15", "void F()\n{\n\tfloat f = 12.e3;\n\tNope();\n}\n", &[(4, "Undefined function 'Nope'")]),
            row("num-16", "void F()\n{\n\tstring s = 0x1F.ToString();\n\tNope();\n}\n", &[(3, BROKEN)]),
            row(
                "num-17",
                "void F()\n{\n\tfloat f = 1.;\n\tfloat g = 12.;\n\tNope();\n}\n",
                &[(3, "Syntax error"), (4, BROKEN)],
            ),
            row("num-18", "void F()\n{\n\tfloat f = 12.+3;\n\tNope();\n}\n", &[(3, BROKEN)]),
        ],
    );
}

// A local that took the name of a class with a body was refused where it was
// declared; the name still means the class (fwd-07 declares a variable of it,
// the call here is unobserved).
#[test]
fn a_refused_local_does_not_hide_the_class_it_is_named_after() {
    let text = "class PrFoo\n{\n\tstatic void S()\n\t{\n\t}\n}\nvoid F()\n{\n\tint PrFoo;\n\tPrFoo.S();\n}\n";
    let lines = support::lines(Target::RetailServer, text);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].0, 9, "{lines:?}");
}
