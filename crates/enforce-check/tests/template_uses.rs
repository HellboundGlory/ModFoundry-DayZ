mod support;

use enforce_check::Target;
use support::{Row, assert_rows, row, text};

const ONE: &str = "class PrT<Class T>\n{\n}\n";
const TWO: &str = "class PrT<Class T, Class U>\n{\n}\n";
const MANY: &str = "FIX-ME: Too many template parameters for 'PrT' type";
const FEW: &str = "FIX-ME: Not enough parameters for template class 'PrT'";
const WRONG: &str = "Wrong number of template parameters";

// A template class named with the wrong number of arguments: what the game
// prints depends on where the type is written.
fn use_site_rows() -> Vec<Row> {
    vec![
        row("tpl-11", text(&[ONE, "void F()\n{\n\tPrT<int,int> t;\n}\n"]), &[(6, MANY), (6, MANY)]),
        row("tpl-12", text(&[ONE, "void F(PrT<int,int> t)\n{\n}\n"]), &[(4, MANY), (4, MANY)]),
        row("tpl-13", text(&[ONE, "PrT<int,int> F()\n{\n\treturn null;\n}\n"]), &[(4, MANY), (4, MANY)]),
        row("tpl-14", text(&[ONE, "class PrU : PrT<int,int>\n{\n}\n"]), &[(4, MANY), (4, MANY), (4, MANY)]),
        row(
            "tpl-15",
            text(&[TWO, "void F()\n{\n\tPrT<int> t;\n}\n"]),
            &[(6, FEW), (6, WRONG), (6, FEW), (6, WRONG), (6, "Error in parameters")],
        ),
        row(
            "tpl-16",
            text(&[TWO, "void F(PrT<int> t)\n{\n}\n"]),
            &[(4, FEW), (4, WRONG), (4, "Error in parameters")],
        ),
        row("tpl-17", text(&[TWO, "PrT<int> F()\n{\n\treturn null;\n}\n"]), &[(4, FEW), (4, WRONG)]),
        row(
            "tpl-18",
            text(&[TWO, "class PrU : PrT<int>\n{\n}\n"]),
            &[
                (4, FEW),
                (4, FEW),
                (4, "Wrong number of parameters"),
                (4, FEW),
                (4, WRONG),
                (4, "Can't inherit class 'PrU'"),
            ],
        ),
        // Arguments on a class that takes none are let through; a template
        // named with none is short of them.
        row(
            "tpl-19",
            "class PrN\n{\n}\nclass PrT<Class T>\n{\n}\nvoid F()\n{\n\tPrN<int> a;\n\tPrT b;\n}\n",
            &[(10, FEW), (10, WRONG), (10, FEW), (10, WRONG), (10, "Error in parameters")],
        ),
    ]
}

#[test]
fn a_template_named_with_the_wrong_number_of_arguments_is_reported_where_it_is_written() {
    assert_rows(Target::RetailServer, &use_site_rows());
}

// tpl-14, tpl-18, tpl-36 and tpl-37 on the Diag server: a base with the wrong
// number of arguments prints one line there, as an error, and the module
// fails even when the arguments are only extra ones. A second wrong base
// prints its line too; a body after them is never compiled.
#[test]
fn a_base_with_the_wrong_number_of_arguments_fails_the_module_on_the_diag_server() {
    const MANY_ERROR: &str = "Too many template parameters for 'PrT' type";
    let rows = [
        row("tpl-14", text(&[ONE, "class PrU : PrT<int,int>\n{\n}\n"]), &[(4, MANY_ERROR)]),
        row("tpl-18", text(&[TWO, "class PrU : PrT<int>\n{\n}\n"]), &[(4, "Not enough parameters for template class 'PrT'")]),
        row("tpl-36", text(&[ONE, "class PrU : PrT<int,int>\n{\n}\nclass PrV : PrT<int,int>\n{\n}\n"]), &[(4, MANY_ERROR), (7, MANY_ERROR)]),
        row("tpl-37", text(&[ONE, "class PrU : PrT<int,int>\n{\n}\nvoid F()\n{\n\tNope();\n}\n"]), &[(4, MANY_ERROR)]),
    ];
    assert_rows(Target::DiagServer, &rows);
    for row in &rows {
        let files = [(support::ENGINE, support::ENGINE_TEXT), (support::PROBE, row.text)];
        let got = enforce_check::check(&support::input(Target::DiagServer, &files));
        assert_eq!(got.failed_modules.len(), 1, "{}", row.id);
    }
}

// tpl-11 to tpl-13, tpl-15 to tpl-17 and tpl-19 on the Diag server: anywhere
// but a base it prints every line the retail server does, as errors.
#[test]
fn the_diag_server_prints_every_line_for_a_use_that_is_no_base() {
    let rows: Vec<Row> = use_site_rows()
        .into_iter()
        .filter(|row| row.id != "tpl-14" && row.id != "tpl-18")
        .map(|found| {
            let expected: Vec<(u32, &'static str)> =
                found.expected.iter().map(|(line, message)| (*line, message.trim_start_matches("FIX-ME: "))).collect();
            row(found.id, found.text, Box::leak(expected.into_boxed_slice()))
        })
        .collect();
    assert_eq!(rows.len(), 7);
    assert_rows(Target::DiagServer, &rows);
}
