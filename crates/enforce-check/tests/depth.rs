mod support;

use enforce_check::Target;
use support::lines;

fn nested_blocks(count: usize) -> String {
    format!("void F()\n{{\n{}Nope();\n{}}}\n", "{\n".repeat(count), "}\n".repeat(count))
}

#[test]
fn blocks_nested_as_deep_as_the_game_reads_them_are_compiled() {
    // par-08, par-09
    for count in [150, 3000] {
        assert_eq!(
            lines(Target::RetailServer, &nested_blocks(count)),
            [(count as u32 + 3, "Undefined function 'Nope'".to_string())],
            "{count} blocks"
        );
    }
}

#[test]
fn a_sum_as_long_as_the_game_reads_is_compiled() {
    // par-12
    let text = format!("void F()\n{{\n\tint x = 1{};\n\tNope();\n}}\n", " + 1".repeat(2999));
    assert_eq!(
        lines(Target::RetailServer, &text),
        [(4, "Undefined function 'Nope'".to_string())]
    );
}

const TOO_COMPLEX: &str = "Formula too complex";
const NO_NOPE: &str = "Undefined function 'Nope'";
// Nine lines, so the body of `F` starts on line 12.
const CHAIN: &str = "class PrN\n{\n\tPrN n;\n\tint v;\n\tPrN G()\n\t{\n\t\treturn this;\n\t}\n}\n";

fn in_f(statements: &[String]) -> String {
    format!("{CHAIN}void F(PrN p)\n{{\n\t{}\n\tNope();\n}}\n", statements.join("\n\t"))
}

fn printed(text: &str) -> Vec<(u32, String)> {
    lines(Target::RetailServer, text)
}

fn expect(counts: &[(u32, usize)], nope: u32) -> Vec<(u32, String)> {
    let mut expected = Vec::new();
    for (line, count) in counts {
        expected.extend(vec![(*line, TOO_COMPLEX.to_string()); *count]);
    }
    expected.push((nope, NO_NOPE.to_string()));
    expected
}

#[test]
fn member_steps_past_the_sixteenth_are_too_complex() {
    let n = |steps: usize| ".n".repeat(steps);
    // fc-05: an argument, an assignment target and a condition take 16 steps.
    let text = in_f(&[
        format!("Print(p{});", n(20)),
        format!("p{} = null;", n(20)),
        format!("if (p{})\n\t{{\n\t}}", n(20)),
    ]);
    assert_eq!(printed(&text), expect(&[(12, 4), (13, 4), (14, 4)], 17), "fc-05");
    // fc-08: so does an operand.
    let text = in_f(&[format!("int x = p{}.v + 1;", n(20))]);
    assert_eq!(printed(&text), expect(&[(12, 5)], 13), "fc-08");
}

#[test]
fn a_chain_that_is_a_whole_initialiser_takes_one_step_fewer() {
    // fc-01, fc-02, par-10
    for (steps, complaints) in [(15, 0), (16, 1), (600, 585)] {
        let text = in_f(&[format!("PrN q = p{};", ".n".repeat(steps))]);
        assert_eq!(printed(&text), expect(&[(12, complaints)], 13), "{steps} steps");
    }
    // fc-06: `this` and a bare member start a chain like a variable does. The
    // function is compiled before the class.
    let text = format!(
        "class PrN\n{{\n\tPrN n;\n\tvoid M()\n\t{{\n\t\tPrN a = this{};\n\t\tPrN b = this{};\n\t\tPrN c = n{};\n\t}}\n}}\nvoid F()\n{{\n\tNope();\n}}\n",
        ".n".repeat(15),
        ".n".repeat(16),
        ".n".repeat(15)
    );
    assert_eq!(printed(&text), [(13, NO_NOPE.to_string()), (7, TOO_COMPLEX.to_string())], "fc-06");
}

#[test]
fn a_call_between_member_steps_neither_counts_nor_starts_the_count_again() {
    // fc-04
    let n = |steps: usize| ".n".repeat(steps);
    let text = in_f(&[
        format!("PrN q = p{}.G(){};", n(8), n(10)),
        format!("PrN r = p{}.G(){};", n(10), n(16)),
    ]);
    assert_eq!(printed(&text), expect(&[(12, 3), (13, 11)], 14));
}

#[test]
fn calls_nested_too_deep_are_too_complex_once() {
    // fc-03: each call on the result of the one before.
    let text = in_f(&[format!("PrN q = p{};", ".G()".repeat(20))]);
    assert_eq!(printed(&text), expect(&[(12, 1)], 13), "fc-03");
    // dp-03: each call in the argument of the one before.
    let text = format!(
        "int G(int a)\n{{\n\treturn a;\n}}\nvoid F()\n{{\n\tint x = {}1{};\n\tNope();\n}}\n",
        "G(".repeat(200),
        ")".repeat(200)
    );
    assert_eq!(printed(&text), expect(&[(7, 1)], 8), "dp-03");
}

#[test]
fn nesting_the_game_reads_without_complaint() {
    let texts = [
        // fc-07
        ("sums in brackets", 13, format!("int x = {}1 + 1{};", "1 + (".repeat(60), ")".repeat(60))),
        // dp-02
        ("not operators", 13, format!("bool b = {}true;", "!".repeat(200))),
        // dp-04
        ("an index in its own index", 14, format!("int a[4];\n\tint x = {}0{};", "a[".repeat(200), "]".repeat(200))),
    ];
    for (name, nope, statement) in texts {
        assert_eq!(printed(&in_f(&[statement])), [(nope, NO_NOPE.to_string())], "{name}");
    }
}

#[test]
fn nesting_at_the_checker_s_own_limit_does_not_overflow_the_stack() {
    // The shapes that take the most stack per level, each at the deepest the
    // parser still reads it.
    let wrap = |open: &str, close: &str, count: usize| {
        format!("class A\n{{\n\tvoid F(int x)\n\t{{\n{}{}\t}}\n}}\n", open.repeat(count), close.repeat(count))
    };
    // That many `if`s or `switch`es are also more than a function holds,
    // which is all the checker says about them.
    let shapes = [
        (wrap("{\n", "}\n", 8100), false),
        (wrap("if (x) {\n", "}\n", 4090), true),
        (wrap("if (x) {} else {\n", "}\n", 4090), true),
        (wrap("for (;x;) {\n", "}\n", 4090), true),
        (wrap("switch (x) {\ncase 1:\n", "}\n", 4090), true),
    ];
    for (text, too_long) in &shapes {
        let printed: Vec<String> = lines(Target::RetailServer, text).into_iter().map(|(_, message)| message).collect();
        let expected: &[&str] = if *too_long { &["Too many instructions per function", "A function of more than 8192 instructions"] } else { &[] };
        assert_eq!(printed, expected, "{}", &text[30..60]);
    }
}

// The later stages' work on a type grows much faster than its depth, so a
// type nested past anything a script writes still ends the parse.
#[test]
fn a_type_nested_thousands_deep_ends_the_parse() {
    let text = format!("void F()\n{{\n\t{}int{} x;\n}}\n", "array<".repeat(4000), ">".repeat(4000));
    let started = std::time::Instant::now();
    let printed = printed(&text);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    assert!(printed.iter().any(|(_, message)| message == "Too many errors"), "{:?}", &printed[..printed.len().min(3)]);
}

// Each `<` looks ahead for the `>` that would make it a type list. It must not
// read the rest of a long line again at every `<`, with no `>` on the line or
// with one followed by a long tail.
#[test]
fn a_long_run_of_less_than_signs_is_read_in_linear_time() {
    let run = " < a".repeat(8000);
    for tail in [String::new(), format!(" > a{}", " + 1".repeat(25_000))] {
        let text = format!("void F(int a)\n{{\n\tbool b = a{run}{tail};\n}}\n");
        let started = std::time::Instant::now();
        printed(&text);
        assert!(started.elapsed().as_millis() < 1500, "{:?}", started.elapsed());
    }
}
