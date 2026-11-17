mod support;

use enforce_check::{check, Diagnostic, Target};

const LIMIT_LINE: &str = "Too many instructions per function";

// The line the game names before it dies, if the function is too long.
fn overflow(text: &str) -> Option<u32> {
    named_line(&support::diagnostics(Target::RetailServer, text))
}

fn named_line(diagnostics: &[Diagnostic]) -> Option<u32> {
    let line = diagnostics.iter().find(|d| d.message == LIMIT_LINE).map(|d| d.line);
    assert_eq!(line.is_some(), diagnostics.iter().any(|d| d.game_crash), "{:?}", diagnostics.last());
    line
}

// lim-04 and lim-c00: a function holds 8,192 instructions, `a = a + 1;` is
// one, and the statement that passes the limit is the one named.
#[test]
fn a_function_holds_8192_instructions() {
    let function = |statements: usize| format!("void F(int a)\n{{\n{}}}\n", "\ta = a + 1;\n".repeat(statements));
    assert_eq!(overflow(&function(8192)), None);
    assert_eq!(overflow(&function(8193)), Some(8195));
    assert_eq!(overflow(&function(20000)), Some(8195));
}

// lim-c01 to lim-c22: 400 copies of one statement, then `a = a + 1;` until
// the function is full. The line the game named gives what the statement
// compiles to: (8,192 - (line - 430)) / 400.
#[test]
fn each_statement_costs_what_the_game_counted() {
    const HEADER: &str = "class PrA\n{\n\tint m_x;\n\tvoid M()\n\t{\n\t}\n\tvoid M1(int p)\n\t{\n\t}\n\tstatic void S()\n\t{\n\t}\n}\n\
        void G1(int p)\n{\n}\nvoid G2(int p, int q)\n{\n}\nvoid GS(string p)\n{\n}\nint R()\n{\n\treturn 1;\n}\n\
        void F(int a, int b, PrA o, string s, array<int> d)\n{\n\tint arr[4];\n";
    let rows: [(&str, u32); 22] = [
        ("a = 5;", 7822),
        ("a = b;", 7822),
        ("a = a + b + 1;", 7022),
        ("a++;", 7422),
        ("G1(a);", 7822),
        ("G2(a, 5);", 7422),
        ("a = R();", 7022),
        ("GS(\"abc\");", 7822),
        ("o.M();", 7822),
        ("o.M1(5);", 7822),
        ("PrA.S();", 7822),
        ("o.m_x = 5;", 7022),
        ("a = o.m_x;", 7022),
        ("if (a) a = 5;", 6622),
        ("if (a) a = 5; else a = 5;", 5422),
        ("if (a == 5) a = 5;", 6622),
        ("o = new PrA;", 7422),
        ("s = \"abc\";", 7822),
        ("while (a) a = 5;", 6222),
        ("a = arr[1];", 7022),
        ("d.Insert(5);", 7822),
        ("a = a + 1; a = a + 1;", 7822),
    ];
    let wrong = wrong_lines(HEADER, "\t", "}\n", &rows);
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// lim-d01 to lim-d56: the same inside a method, the copies starting at line
// 50: (8,192 - (line - 450)) / 400. `%d` is the number of the copy.
#[test]
fn each_statement_in_a_method_costs_what_the_game_counted() {
    const HEADER: &str = "enum PrE\n{\n\tV = 3\n}\nconst int PR_K = 7;\n\
        class PrA\n{\n\tint m_x;\n\tPrA m_o;\n\tstatic int s_x;\n\tvoid M()\n\t{\n\t}\n\tstatic void S()\n\t{\n\t}\n\
        \tPrA GetO()\n\t{\n\t\treturn m_o;\n\t}\n}\n\
        void G1(int p)\n{\n}\nvoid G3(int p, int q, int r)\n{\n}\nvoid G4(int p, int q, int r, int t)\n{\n}\n\
        void G5(int p, int q, int r, int t, int u)\n{\n}\nint R()\n{\n\treturn 1;\n}\n\
        class PrH\n{\n\tint m_x;\n\tvoid M()\n\t{\n\t}\n\
        \tvoid F(int a, int b, PrA o, string s, array<int> d, bool t, float f, vector v, PrA o2)\n\t{\n\
        \t\tint arr[4];\n\t\tint i;\n\t\tPrA l;\n\t\tarray<int> e;\n";
    let rows: [(&str, u32); 54] = [
        ("a = a - 1;", 8242),
        ("a = a + 2;", 7842),
        ("a = a + b;", 7842),
        ("a = b + 1;", 7842),
        ("a = 1 + a;", 8242),
        ("a = a * b;", 7842),
        ("a = a + b + b + b;", 6242),
        ("a += 1;", 7042),
        ("a += b;", 7042),
        ("a--;", 7442),
        ("++a;", 8242),
        ("a = -b;", 7842),
        ("t = !t;", 4642),
        ("t = a == 5;", 5442),
        ("t = a && b;", 4242),
        ("if (a && b) a = 5;", 5442),
        ("if (!a) a = 5;", 6642),
        ("if (o) a = 5;", 6642),
        ("if (a) a = 5; else if (b) a = 5;", 4242),
        ("G3(a, 5, b);", 7442),
        ("G4(a, 5, b, 5);", 7042),
        ("G5(a, 5, b, 5, a);", 7042),
        ("G1(R());", 7042),
        ("G1(a + 1);", 7042),
        ("M();", 7842),
        ("m_x = 5;", 7842),
        ("a = m_x;", 7842),
        ("this.m_x = 5;", 7042),
        ("o.m_o.m_x = 5;", 5442),
        ("o.GetO().M();", 6642),
        ("PrA.s_x = 5;", 7842),
        ("a = PR_K;", 7842),
        ("a = PrE.V;", 7842),
        ("f = a;", 7842),
        ("f = f + 1.5;", 7842),
        ("a = f;", 7842),
        ("s = s + \"x\";", 7842),
        ("v = \"1 2 3\";", 7842),
        ("o2 = PrA.Cast(o);", 5842),
        ("o = null;", 7842),
        ("l = new PrA;", 7442),
        ("e = {1, 2, 3};", 5042),
        ("a = arr[b];", 7842),
        ("arr[1] = 5;", 7842),
        ("a = d[1];", 7042),
        ("d[1] = 5;", 7842),
        ("a = d.Count();", 7442),
        ("Print(a);", 7842),
        ("for (i = 0; i < 2; i++) a = 5;", 3842),
        ("foreach (int w%d : d) a = 5;", 1842),
        ("switch (a) { case 1: a = 5; break; }", 4642),
        ("if (a) return;", 6242),
        ("while (a) { a = 5; break; }", 5842),
        ("while (a) { a = 5; continue; }", 5842),
    ];
    let wrong = wrong_lines(HEADER, "\t\t", "\t}\n}\n", &rows);
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// What the lim-e probes are compiled with: the engine methods they call, and
// the game's own `GetGame` in a file read before the probe.
const ENGINE_E: &str = "class Class\n{\n\tproto static Class Cast(Class from);\n\tproto static bool CastTo(out Class to, Class from);\n\
    \tproto native owned external string ClassName();\n}\n\
    class Managed\n{\n}\n\
    class array<Class T>\n{\n\tproto native int Count();\n\tproto T Get(int n);\n\tproto void Set(int n, T value);\n\tproto int Insert(T value);\n\
    \tproto int Find(T value);\n\tproto int InsertAt(T value, int index);\n}\n\
    class set<Class T>\n{\n}\nclass map<Class TKey,Class TValue>\n{\n}\n\
    class int\n{\n\tproto string ToString();\n}\nclass float\n{\n}\nclass bool\n{\n}\n\
    class string\n{\n\tproto native int Length();\n\tproto string Substring(int start, int len);\n}\n\
    class vector\n{\n\tproto native float Length();\n\tproto float Normalize();\n}\nclass typename\n{\n}\n\
    class Math\n{\n\tproto static float Clamp(float value, float min, float max);\n\tproto static float Max(float x, float y);\n}\n\
    proto native float GetWorldTime();\n";
const GAME_E: &str = "class CGame\n{\n}\nCGame g_Game;\nCGame GetGame()\n{\n\treturn g_Game;\n}\n";
const CLASSES_E: &str = "enum PrE\n{\n\tV = 3\n}\nconst int PR_K = 7;\n\
    class PrA\n{\n\tint m_x;\n\tPrA m_o;\n\tstatic int s_x;\n\tvoid M()\n\t{\n\t}\n\tstatic int SR()\n\t{\n\t\treturn 1;\n\t}\n\
    \tint MR()\n\t{\n\t\treturn 1;\n\t}\n\tPrA GetO()\n\t{\n\t\treturn m_o;\n\t}\n}\n\
    void G1(int p)\n{\n}\nvoid G2(int p, int q)\n{\n}\nvoid G4(int p, int q, int r, int t)\n{\n}\n\
    class PrB\n{\n\tint m_y;\n}\nint R()\n{\n\treturn 1;\n}\n\
    class PrH : PrB\n{\n\tint m_x;\n\tPrA m_o;\n\tvoid M()\n\t{\n\t}\n\tint MR2()\n\t{\n\t\treturn 1;\n\t}\n";
const LOCALS_E: &str = " F(int a, int b, PrA o, string s, array<int> d, bool t, float f, vector v, PrA o2)\n\t{\n\
    \t\tint arr[4];\n\t\tint i;\n\t\tPrA l;\n\t\tarray<int> e;\n\t\tCGame g;\n";

fn overflow_e(text: &str) -> Option<u32> {
    let files = [(support::ENGINE, ENGINE_E), ("Scripts/3_Game/game.c", GAME_E), (support::PROBE, text)];
    named_line(&check(&support::input(Target::RetailServer, &files)).diagnostics)
}

// The lines the game named for `copies` of each statement in the lim-e method,
// which starts them at line 62.
fn wrong_lines_e(returns: &str, close: &str, copies: usize, rows: &[(&str, u32)]) -> Vec<String> {
    let header = format!("{CLASSES_E}\t{returns}{LOCALS_E}");
    wrong_lines_of(overflow_e, &header, "\t\t", close, copies, rows)
}

// lim-e01 to lim-e66 (lim-e24 aside): the shapes the earlier runs left open,
// each (8,192 - (line - 462)) / 400, or with 40 copies (8,192 - (line - 102)) / 40.
#[test]
fn each_statement_in_the_second_method_costs_what_the_game_counted() {
    let rows = [
        ("a = a + 1;", 8254),
        ("a = arr[1];", 7054),
        ("arr[b] = 5;", 7854),
        ("--a;", 8254),
        ("a = a * 1;", 7854),
        ("t = !a;", 4654),
        ("t = a || b;", 4254),
        ("if (a || b) a = 5;", 5454),
        ("if (a > 5 && b < 6) a = 5;", 5454),
        ("G1(a == 5);", 4654),
        ("a = MR2();", 7054),
        ("a = o.MR();", 7054),
        ("a = PrA.SR();", 7054),
        ("o.m_o.M();", 6254),
        ("a = o.m_o.m_x;", 5454),
        ("G1(o.m_x);", 6254),
        ("a = o.m_x + 1;", 7054),
        ("e = {1};", 6654),
        ("e = {1, 2, 3, 4, 5, 6};", 2654),
        ("switch (a) { case 1: a = 5; break; case 2: a = 5; break; }", 1054),
        ("switch (a) { case 1: a = 5; }", 5054),
        ("switch (a) { default: a = 5; }", 7054),
        ("for (i = 0; i < 2; ++i) a = 5;", 4654),
        ("a = s.Length();", 7054),
        ("d.Set(1, 5);", 7854),
        ("a = d.Get(1);", 7054),
        ("PrA.Cast(o);", 6654),
        ("if (PrA.Cast(o)) a = 5;", 4654),
        ("Class.CastTo(o2, o);", 7054),
        ("this.M();", 7854),
        ("a = this.m_x;", 7054),
        ("m_o.m_x = 5;", 6254),
        ("a = R() + 1;", 7054),
        ("if (R()) a = 5;", 5854),
        ("a = d[b];", 7054),
        ("o.m_x = a + 1;", 7054),
        ("o.m_x = o2.m_x;", 6254),
        ("a = b++;", 6654),
        ("a = ~b;", 7054),
        ("o.m_x++;", 6654),
        ("a = PrA.s_x;", 7854),
        ("o.GetO().GetO().M();", 4254),
        ("G1(d.Count());", 7454),
        ("a = o.GetO().m_x;", 5854),
        ("G2(a, b);", 7454),
        ("G4(a, b, a, b);", 7054),
        ("G2(5, a);", 7454),
        ("G2(5, 5);", 7454),
        ("g = GetGame();", 7054),
        ("G1(7 + 1);", 6254),
        ("G1(PR_K + 1);", 6254),
        ("a = b + PR_K * 2;", 6254),
        ("if (true) a = 5;", 6654),
        ("f = v[1];", 7054),
        ("a = super.m_y;", 7054),
        ("e = {a, b, a};", 5054),
    ];
    let mut wrong = wrong_lines_e("void", "\t}\n}\n", 400, &rows);
    let returning = [("if (a) return 5;", 5454), ("if (a) return a + 1;", 5454)];
    wrong.extend(wrong_lines_e("int", "\t\treturn 0;\n\t}\n}\n", 400, &returning));
    // 400 variables kill the game, so what declares one is written 40 times.
    let declaring = [
        ("foreach (int k%d, int w%d : d) a = 5;", 7614),
        ("foreach (int w%d : arr) a = 5;", 7734),
        ("int w%d;", 8294),
        ("int w%d = 5;", 8214),
        ("int w%d = a;", 8214),
        ("int w%d = a + 1;", 8214),
        ("PrA w%d = new PrA;", 8174),
        ("int w%d = R();", 8134),
    ];
    wrong.extend(wrong_lines_e("void", "\t}\n}\n", 40, &declaring));
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// What the lim-f probes are compiled with: their method starts its copies at
// line 64.
const CLASSES_F: &str = "enum PrE\n{\n\tV = 3\n}\nconst int PR_K = 7;\n\
    class PrA\n{\n\tint m_x;\n\tstring m_s;\n\tPrA m_o;\n\tstatic int s_x;\n\tvoid M()\n\t{\n\t}\n\tvoid M2(int p, int q)\n\t{\n\t}\n\
    \tPrA GetO()\n\t{\n\t\treturn m_o;\n\t}\n}\n\
    void G1(int p)\n{\n}\nvoid GF(float p)\n{\n}\nvoid GO(PrA p)\n{\n}\nvoid G6(int p, int q, int r, int t, int u, int w)\n{\n}\n\
    class PrB\n{\n\tint m_y;\n}\nint R()\n{\n\treturn 1;\n}\n\
    class PrH : PrB\n{\n\tint m_x;\n\tPrA m_o;\n\tref array<int> m_d;\n\tint m_arr[4];\n\tvoid M()\n\t{\n\t}\n\tvoid M2(int p, int q)\n\t{\n\t}\n\
    \tvoid M4(int p, int q, int r, int t)\n\t{\n\t}\n";
const LOCALS_F: &str = " F(int a, int b, PrA o, string s, array<int> d, bool t, float f, vector v, PrA o2, map<int, int> mp, set<int> st)\n\t{\n\
    \t\tint arr[4];\n\t\tint i;\n\t\tPrA l;\n\t\tarray<int> e;\n";

fn wrong_lines_f(returns: &str, close: &str, copies: usize, rows: &[(&str, u32)]) -> Vec<String> {
    let header = format!("{CLASSES_F}\t{returns}{LOCALS_F}");
    wrong_lines_of(overflow_e, &header, "\t\t", close, copies, rows)
}

// lim-f01 to lim-f79 (lim-f40 aside): the neighbours of the shapes the run
// before left unmeasured, each (8,192 - (line - 464)) / 400, or with 40 copies
// (8,192 - (line - 104)) / 40. Five of them are refused line by line and
// counted all the same; `a = b = 5;` alone compiles to nothing.
#[test]
fn each_statement_in_the_third_method_costs_what_the_game_counted() {
    let rows = [
        ("a = a + 1;", 8256),
        ("M2(a, b);", 7456),
        ("o.M2(a, b);", 7456),
        ("M4(a, b, a, b);", 7056),
        ("s = o.ClassName();", 6256),
        ("f = v.Length();", 7056),
        ("f = GetWorldTime();", 7456),
        ("GetWorldTime();", 8256),
        ("f = Math.Clamp(f, 0, 5);", 6656),
        ("f = Math.Max(f, 5);", 7056),
        ("d.InsertAt(5, 1);", 7856),
        ("a = d.Find(5);", 7056),
        ("s = s.Substring(0, 1);", 6656),
        ("o.m_x = a;", 7056),
        ("o.m_x = PR_K;", 7056),
        ("o.m_s = \"abc\";", 7056),
        ("o.m_o = o2;", 7056),
        ("o.m_x = R();", 6256),
        ("o.m_o = new PrA;", 6656),
        ("m_x = a + 1;", 7856),
        ("s = \"x\" + s;", 7856),
        ("a = PrE.V + b;", 7856),
        ("a = PrA.s_x + b;", 7856),
        ("a = 5 + b;", 7856),
        ("a = 1 - a;", 7856),
        ("a = a / 1;", 7856),
        ("a = a << 1;", 7856),
        ("a = a | 1;", 7856),
        ("a = m_o.m_x;", 6256),
        ("m_o.M();", 7056),
        ("G1(m_x);", 7056),
        ("a = m_x + 1;", 7856),
        ("switch (a) { case PrE.V: a = 5; }", 5056),
        ("switch (a) { case 1: case 2: a = 5; }", 2656),
        ("switch (a) { case 1: a = 5; break; default: a = 5; }", 3856),
        ("switch (a) { }", 7856),
        ("o.GetO().GetO().GetO().M();", 1856),
        ("a = o.GetO().GetO().m_x;", 3456),
        ("o.GetO().m_o.M();", 5056),
        ("G1(d[1]);", 7056),
        ("G1(arr[1]);", 7056),
        ("G1(arr[b]);", 7056),
        ("a = d[1] + 1;", 7056),
        ("a = v[b];", 7856),
        ("a = m_arr[1];", 7056),
        ("a = -5;", 7856),
        ("G1(-5);", 7856),
        ("G1(-a);", 7056),
        ("G1(!a);", 4656),
        ("GF(1.5);", 7856),
        ("GO(null);", 7056),
        ("if (o.m_x) a = 5;", 5856),
        ("if (o.m_x == 5) a = 5;", 5856),
        ("if (5 == a) a = 5;", 6656),
        ("a = b = 5;", 8656),
        ("o.m_x += 1;", 6256),
        ("a = o.m_x + o2.m_x;", 6256),
        ("for (i = 0;;) { break; }", 6656),
        ("for (;; i++) { break; }", 6256),
        ("while (true) { break; }", 6656),
        ("a = 2 * b;", 7856),
        ("f = 0.5 * f;", 7856),
        ("a = PR_K + b;", 7856),
        ("G6(a, b, a, b, a, b);", 6656),
        ("v.Normalize();", 7856),
        ("s = a.ToString();", 7056),
        ("if (a) a = 5 + b;", 6656),
    ];
    let mut wrong = wrong_lines_f("void", "\t}\n}\n", 400, &rows);
    let returning = [
        ("if (a) return a;", 5456),
        ("if (a) return R();", 4656),
        ("if (a) return o.m_x;", 4656),
        ("if (a) return PR_K;", 5456),
    ];
    wrong.extend(wrong_lines_f("int", "\t\treturn 0;\n\t}\n}\n", 400, &returning));
    let declaring = [
        ("foreach (int w%d : m_d) a = 5;", 7496),
        ("foreach (int k%d, int w%d : arr) a = 5;", 7736),
        ("foreach (int k%d, int w%d : mp) a = 5;", 7216),
        ("foreach (int w%d : st) a = 5;", 7616),
        ("int w%d = b++;", 8096),
        ("PrA w%d;", 8296),
        ("int w%d[4];", 8296),
        ("int w%d, x%d;", 8296),
    ];
    wrong.extend(wrong_lines_f("void", "\t}\n}\n", 40, &declaring));
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// lim-e24: the game refuses each `for` with no test and still compiles three
// instructions of its line, so the function fills 1,200 lines sooner.
#[test]
fn a_refused_line_is_still_counted() {
    let text = format!("{CLASSES_E}\tvoid{LOCALS_E}{}{}\t}}\n}}\n", "\t\tfor (;;) { break; }\n".repeat(400), "\t\ta = a + 1;\n".repeat(9000));
    let files = [(support::ENGINE, ENGINE_E), ("Scripts/3_Game/game.c", GAME_E), (support::PROBE, text.as_str())];
    let diagnostics = check(&support::input(Target::RetailServer, &files)).diagnostics;
    let printed: Vec<(u32, &str)> = diagnostics.iter().map(|d| (d.line, d.message.as_str())).collect();
    let refused: Vec<(u32, &str)> = (62..462).map(|line| (line, "Syntax error")).collect();
    assert_eq!(printed[..400], refused[..]);
    assert_eq!(printed[400], (7454, LIMIT_LINE));
    assert!(diagnostics[401].game_crash && diagnostics.len() == 402);
    // Not measured: the test alone is what the game misses, whatever the
    // other two parts hold.
    let first_part = support::lines(Target::RetailServer, "void F(int a)\n{\n\tfor (a = 0;; a++) { break; }\n}\n");
    assert_eq!(first_part, [(3, "Syntax error".to_string())]);
}

// What the checker prints for `copies` of a statement in the lim-f method,
// then `a = a + 1;` 9,000 times.
fn printed_f(copies: usize, statement: &str) -> Vec<(u32, String)> {
    let copies: String = (0..copies).map(|copy| format!("\t\t{}\n", statement.replace("%d", &copy.to_string()))).collect();
    let text = format!("{CLASSES_F}\tvoid{LOCALS_F}{copies}{}\t}}\n}}\n", "\t\ta = a + 1;\n".repeat(9000));
    let files = [(support::ENGINE, ENGINE_E), ("Scripts/3_Game/game.c", GAME_E), (support::PROBE, text.as_str())];
    let diagnostics = check(&support::input(Target::RetailServer, &files)).diagnostics;
    diagnostics.iter().filter(|d| !d.game_crash).map(|d| (d.line, d.message.clone())).collect()
}

// lim-f40: a `foreach` over a static array that a member of the class holds
// is refused, under a name of the game's own, and only its body is compiled.
#[test]
fn a_foreach_over_a_member_static_array_is_refused() {
    let printed = printed_f(40, "foreach (int w%d : m_arr) a = 5;");
    let refused: Vec<(u32, String)> = (64..104).map(|line| (line, "'#array' is not array".to_string())).collect();
    assert_eq!(printed[..40], refused[..]);
    assert_eq!(printed[40..], [(8216, LIMIT_LINE.to_string())]);
    // Not measured: its variable is still declared, so a body that reads it
    // prints nothing more; a member the class inherits, or one named in a
    // static method, is left alone.
    const REFUSED: &str = "'#array' is not array";
    let class = |body: &str| format!("class PrB\n{{\n\tint m_far[4];\n}}\nclass PrH : PrB\n{{\n\tint m_arr[4];\n{body}}}\n");
    let read = class("\tvoid F(int a)\n\t{\n\t\tforeach (int w : m_arr)\n\t\t{\n\t\t\ta = w;\n\t\t}\n\t}\n");
    assert_eq!(support::lines(Target::RetailServer, &read), [(10, REFUSED.to_string())]);
    let inherited = class("\tvoid F(int a)\n\t{\n\t\tforeach (int w : m_far)\n\t\t{\n\t\t}\n\t}\n");
    assert_eq!(support::lines(Target::RetailServer, &inherited), []);
    let in_static = class("\tstatic void F(int a)\n\t{\n\t\tforeach (int w : m_arr)\n\t\t{\n\t\t}\n\t}\n");
    assert!(support::lines(Target::RetailServer, &in_static).iter().all(|(_, message)| message != REFUSED));
}

// lim-f52: an element of a `vector` read with a name for its index does not
// go into an `int`. The line is counted all the same.
#[test]
fn a_vector_element_read_with_a_name_is_no_int() {
    let printed = printed_f(400, "a = v[b];");
    let refused: Vec<(u32, String)> = (64..464).map(|line| (line, "Incompatible parameter 'a'".to_string())).collect();
    assert_eq!(printed[..400], refused[..]);
    assert_eq!(printed[400..], [(7856, LIMIT_LINE.to_string())]);
    // Not measured: a number for the index, or a `float` to keep it in, is
    // what vanilla writes; so is any index but a local's name.
    for statement in ["a = v[1];", "f = v[b];", "f = v[1];", "a = v[PR_K];", "a = v[1 + 1];", "a = v[-1];", "a = v[m_x];"] {
        assert!(printed_f(1, statement).iter().all(|(_, message)| message == LIMIT_LINE), "{statement}");
    }
}

// The rows where the checker names another line than the game: 400 copies
// of the statement after `header`, then `a = a + 1;` 9,000 times.
fn wrong_lines(header: &str, indent: &str, close: &str, rows: &[(&str, u32)]) -> Vec<String> {
    wrong_lines_of(overflow, header, indent, close, 400, rows)
}

fn wrong_lines_of(overflow: fn(&str) -> Option<u32>, header: &str, indent: &str, close: &str, copies: usize, rows: &[(&str, u32)]) -> Vec<String> {
    let filler = format!("{indent}a = a + 1;\n").repeat(9000);
    let got: Vec<Option<u32>> = std::thread::scope(|scope| {
        let checks: Vec<_> = rows
            .iter()
            .map(|(statement, _)| {
                let copies: String = (0..copies).map(|copy| format!("{indent}{}\n", statement.replace("%d", &copy.to_string()))).collect();
                let text = format!("{header}{copies}{filler}{close}");
                scope.spawn(move || overflow(&text))
            })
            .collect();
        checks.into_iter().map(|check| check.join().unwrap()).collect()
    });
    rows.iter()
        .zip(&got)
        .filter(|((_, line), got)| **got != Some(*line))
        .map(|((statement, line), got)| format!("`{statement}`: got {got:?}, the game named {line}"))
        .collect()
}

// What 400 copies of a statement in a method compile to, one copy's share:
// the rest of the function is filled with `a = a + 1;`, and the line the
// checker names says how many of those still fitted.
fn counted(statement: &str) -> u32 {
    const HEADER: &str = "enum PrE\n{\n\tV = 3\n}\nclass PrA\n{\n\tint m_x;\n\tPrA m_n;\n\tstatic int s_x;\n\tvoid M()\n\t{\n\t}\n\
        \tPrA GetO()\n\t{\n\t\treturn m_n;\n\t}\n}\n\
        void G4(int p, int q, int r, int t)\n{\n}\nvoid GO(PrA p)\n{\n}\n\
        PrA g_o;\nPrA RO()\n{\n\treturn g_o;\n}\nint R()\n{\n\treturn 1;\n}\n\
        class PrB\n{\n\tint m_y;\n\tvoid MB()\n\t{\n\t}\n}\n\
        class PrH : PrB\n{\n\tint m_x;\n\tPrA m_o;\n\tset<int> m_st;\n\tstatic int s_h;\n\
        \tvoid F(int a, int b, PrA o, array<int> d, map<int, int> mp)\n\t{\n\t\tint arr[4];\n\t\tPrA oa[2];\n";
    counted_in(HEADER, statement)
}

fn counted_in(header: &str, statement: &str) -> u32 {
    let first_filler = header.lines().count() as u32 + 400 + 1;
    let copies: String = (0..400).map(|copy| format!("\t\t{}\n", statement.replace("%d", &copy.to_string()))).collect();
    let text = format!("{header}{copies}{}\t}}\n}}\n", "\t\ta = a + 1;\n".repeat(9000));
    let line = overflow(&text).unwrap_or_else(|| panic!("`{statement}` never filled the function"));
    (8192 - (line - first_filler)) / 400
}

// Nothing here was measured in the game. Each is counted as the least it
// could be, so the checker never reports a function the game compiles.
#[test]
fn what_the_game_was_not_measured_on_is_counted_low() {
    let rows = [
        // Of two numbers under an operator the left one is loaded first
        // (measured in an argument); an enum value and a static are not.
        ("a = 1 + 2;", 4),
        ("G4(PrE.V + 1, a, b, a);", 6),
        ("a = PrA.s_x + 1;", 2),
        // A sign on a number is part of the number.
        ("G4(~1, a, b, a);", 4),
        // An element is two wherever it is read; measured as an argument,
        // under an operator and kept, not as what a method is called on.
        ("oa[1].M();", 4),
        // A member of this class is loaded before it is handed on; a static,
        // a global and a member the class inherits are taken for free.
        ("G4(s_h, a, b, a);", 4),
        ("G4(m_y, a, b, a);", 4),
        // Any assignment kept by another compiles to nothing, like the `=`
        // measured.
        ("a = b += 5;", 0),
        ("g_o.M();", 2),
        // A `map` walked with one variable, and a `set` that a member holds,
        // are the cheapest `foreach` measured.
        ("foreach (int w%d : mp) a = 5;", 14),
        ("foreach (int w%d : m_st) a = 5;", 14),
        // Only `null` was measured loaded: `NULL` is handed on as it is.
        ("GO(NULL);", 2),
        // What a call costs follows the type its receiver's names give.
        ("super.MB();", 2),
        ("RO().M();", 5),
        ("PrA.Cast(o).M();", 8),
        ("switch (R()) { case 1: a = 5; }", 11),
    ];
    let wrong: Vec<String> = rows
        .iter()
        .map(|(statement, cost)| (statement, *cost, counted(statement)))
        .filter(|(_, cost, got)| cost != got)
        .map(|(statement, cost, got)| format!("`{statement}`: counted {got}, want {cost}"))
        .collect();
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    // A member handed to an `out` parameter is written, not loaded, and an
    // element that is stepped or added to is no read.
    let unread = [("Class.CastTo(m_o, o);", 7056), ("arr[b]++;", 7456), ("d[1] += 5;", 7056)];
    assert_eq!(wrong_lines_f("void", "\t}\n}\n", 400, &unread), Vec::<String>::new());
    // A parameter hides the member and the constant of its name.
    const SHADOWS: &str = "const int PR_K = 7;\nclass PrA\n{\n\tint m_x;\n\tvoid M()\n\t{\n\t}\n}\n\
        class PrH\n{\n\tPrA m_o;\n\tarray<int> m_q;\n\tvoid F(int a, PrA m_o, PrA m_q, int PR_K)\n\t{\n";
    let shadowed = [("m_o.m_x = 5;", 4), ("m_q.M();", 2), ("a = PR_K + 1;", 2)];
    for (statement, cost) in shadowed {
        assert_eq!(counted_in(SHADOWS, statement), cost, "{statement}");
    }
    // The steps of a chain past the 16 the game takes are not compiled, so a
    // chain it refuses as too complex never fills the function.
    let chain = format!("class PrA\n{{\n\tint m_x;\n\tPrA m_n;\n}}\nvoid F(int a, PrA o)\n{{\n\ta = o{}.m_x;\n}}\n", ".m_n".repeat(5000));
    assert_eq!(overflow(&chain), None);
}

// lim-va, lim-vb and lim-vc: a function declares 256 variables, whatever
// their type and however many parameters it has. With one more the game still
// prints the function's lines and dies after them; with two more it dies at
// that declaration and prints nothing.
#[test]
fn a_function_declares_256_variables() {
    const NOPE: &str = "Undefined function 'Nope'";
    const DIES: &str = "A function of more than 256 variables";
    let shapes = [
        ("void F()", "int w%d;"),
        ("void F(int p0, int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8)", "int w%d;"),
        ("void F()", "vector w%d;"),
    ];
    for (signature, declaration) in shapes {
        let printed = |count: u32| -> Vec<(u32, String, bool)> {
            let declarations: String = (0..count).map(|copy| format!("\t{}\n", declaration.replace("%d", &copy.to_string()))).collect();
            let text = format!("class PrA\n{{\n}}\n{signature}\n{{\n{declarations}\tNope();\n}}\n");
            support::diagnostics(Target::RetailServer, &text).into_iter().map(|d| (d.line, d.message, d.game_crash)).collect()
        };
        assert_eq!(printed(256), [(262, NOPE.to_string(), false)], "{declaration}");
        assert_eq!(printed(257), [(263, NOPE.to_string(), false), (262, DIES.to_string(), true)], "{declaration}");
        assert_eq!(printed(258), [(263, DIES.to_string(), true)], "{declaration}");
        assert_eq!(printed(400), [(263, DIES.to_string(), true)], "{declaration}");
    }
    let printed = |text: &str| -> Vec<(u32, String, bool)> {
        support::diagnostics(Target::RetailServer, text).into_iter().map(|d| (d.line, d.message, d.game_crash)).collect()
    };
    let many = |count: u32, shape: &str| -> String { (0..count).map(|copy| shape.replace("%d", &copy.to_string())).collect() };
    // lim-w02, lim-w03 and lim-w05: a block gives its variables' room back
    // when it ends, and so does a `for`; a `foreach` takes none.
    let blocks = format!("void F(int a)\n{{\n{}}}\n", many(300, "\tif (a)\n\t{\n\t\tint w%d;\n\t}\n"));
    assert_eq!(printed(&blocks), []);
    let headers = format!("void F()\n{{\n{}}}\n", many(300, "\tfor (int i%d = 0; i%d < 1; i%d++)\n\t{\n\t}\n"));
    assert_eq!(printed(&headers), []);
    let walks = format!("void F(array<int> d)\n{{\n{}{}}}\n", many(200, "\tint w%d;\n"), many(100, "\tforeach (int v%d : d)\n\t{\n\t}\n"));
    assert_eq!(printed(&walks), []);
    // lim-w06: the 257th at once is one too many inside a block as well.
    let inside = format!("void F()\n{{\n{}\t{{\n\t\tint x;\n\t}}\n\tNope();\n}}\n", many(256, "\tint w%d;\n"));
    assert_eq!(printed(&inside), [(262, NOPE.to_string(), false), (260, DIES.to_string(), true)]);
    // lim-w10: with one too many the game reads on to the end of the module,
    // so a later function still prints its lines.
    let later = format!("void F()\n{{\n{}}}\nvoid G()\n{{\n\tNope();\n}}\n", many(257, "\tint w%d;\n"));
    assert_eq!(printed(&later), [(263, NOPE.to_string(), false), (259, DIES.to_string(), true)]);
    // lim-w08: each function has its own 256.
    let two = format!("void F()\n{{\n{}}}\nvoid G()\n{{\n{}}}\n", many(200, "\tint w%d;\n"), many(200, "\tint w%d;\n"));
    assert_eq!(printed(&two), []);
    // Not measured: a declaration that is the whole body of an `if` or a
    // loop gives its room back like a block.
    let bare = format!("void F(int a)\n{{\n{}}}\n", many(300, "\tif (a)\n\t\tint w%d;\n"));
    assert_eq!(printed(&bare), []);
    // Nothing is read past the declaration the game dies at.
    let started = std::time::Instant::now();
    let huge = format!("void F()\n{{\n{}}}\n", many(60_000, "\tint w%d;\n"));
    assert_eq!(printed(&huge), [(260, DIES.to_string(), true)]);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    // lim-w11 and lim-w12: a declaration the game refuses takes no room. 300
    // of one name, or of a type nothing declares, print their lines and no
    // more.
    for declaration in ["\tint w;\n", "\tPrNone w;\n"] {
        let refused = format!("void F()\n{{\n{}}}\n", declaration.repeat(300));
        assert!(!support::diagnostics(Target::RetailServer, &refused).iter().any(|d| d.game_crash), "{declaration}");
    }
}

// The count is each function's own: two long functions do not add up, and a
// template class compiled in the middle of a function leaves that function's
// count where it was.
#[test]
fn the_count_belongs_to_one_function() {
    let fill = |count: usize| "\ta = a + 1;\n".repeat(count);
    let two = format!("void F(int a)\n{{\n{}}}\nvoid G(int a)\n{{\n{}}}\n", fill(5000), fill(5000));
    assert_eq!(overflow(&two), None);
    // 4,000 before the instance and 4,193 after it: the last one is too many.
    let around = format!(
        "class PrT<Class T>\n{{\n\tvoid M(int a)\n\t{{\n{}\t}}\n}}\nvoid F(int a)\n{{\n{}\tPrT<int> t;\n{}}}\n",
        fill(100),
        fill(4000),
        fill(4193)
    );
    assert_eq!(overflow(&around), Some(100 + 8 + 4000 + 1 + 4193));
}

// par-13: each `else if` of a ladder is four, its jump and its own test, so
// 2,048 of them fill a function. Here in 16 ladders, short enough not to be
// a ladder the game dies of.
#[test]
fn an_else_if_costs_its_jump_and_its_test() {
    let ladder = format!("\tif (a) {{}}\n{}", "\telse if (a) {}\n".repeat(127));
    let function = |fillers: usize| format!("void F(int a)\n{{\n{}{}}}\n", ladder.repeat(16), "\ta = a + 1;\n".repeat(fillers));
    // A ladder is 128 tests and 127 jumps: 511.
    assert_eq!(overflow(&function(16)), None);
    assert_eq!(overflow(&function(17)), Some(2 + 16 * 128 + 17));
}
