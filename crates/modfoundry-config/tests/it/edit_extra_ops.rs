//! Golden tests for the edit engine's `SetArray` (the
//! `[]` toggle behind MF-C350) and `RemoveAt` (occurrence-targeted removal
//! behind MF-C330).

use crate::edit_fixtures as fixtures;

use fixtures::*;
use modfoundry_config::edit::*;

fn set_array(class_path: &[&str], name: &str, array: bool) -> EditOp {
    EditOp::SetArray {
        target: prop(class_path, name),
        array,
    }
}

fn remove_at(class_path: &[&str], name: &str, occurrence: usize) -> EditOp {
    EditOp::RemoveAt {
        class: path(class_path),
        name: name.into(),
        occurrence,
    }
}

const SHAPES: &str = "class A
{
    braced = { \"a\" };
    list[] = \"b\";
    scalar = 1;
    already[] = { 2 };
};
";

#[test]
fn set_array_adds_the_brackets() {
    assert_eq!(
        edit(SHAPES, &set_array(&["A"], "braced", true)),
        SHAPES.replace("braced = { \"a\" };", "braced[] = { \"a\" };")
    );
}

#[test]
fn set_array_removes_the_brackets() {
    assert_eq!(
        edit(SHAPES, &set_array(&["A"], "list", false)),
        SHAPES.replace("list[] = \"b\";", "list = \"b\";")
    );
}

#[test]
fn set_array_is_a_no_op_when_the_key_already_matches() {
    assert_eq!(edit(SHAPES, &set_array(&["A"], "already", true)), SHAPES);
    assert_eq!(edit(SHAPES, &set_array(&["A"], "scalar", false)), SHAPES);
}

#[test]
fn set_array_keeps_the_value_bytes_and_the_rest_of_the_file() {
    let text = "class A\n{\n    x = { 1, 2 };\n};\nclass B\n{\n    y = 3;\n};\n";
    let out = edit(text, &set_array(&["A"], "x", true));
    assert_eq!(
        out,
        "class A\n{\n    x[] = { 1, 2 };\n};\nclass B\n{\n    y = 3;\n};\n"
    );
}

#[test]
fn set_array_works_on_a_one_line_class() {
    assert_eq!(
        edit("class A { x = { 1 }; };\n", &set_array(&["A"], "x", true)),
        "class A { x[] = { 1 }; };\n"
    );
}

#[test]
fn set_array_works_on_a_nested_class() {
    let text = "class P\n{\n    class C\n    {\n        x[] = 1;\n    };\n};\n";
    let out = edit(text, &set_array(&["P", "C"], "x", false));
    assert_eq!(
        out,
        "class P\n{\n    class C\n    {\n        x = 1;\n    };\n};\n"
    );
}

#[test]
fn set_array_works_with_crlf() {
    let text = crlf(SHAPES);
    assert_eq!(
        edit(&text, &set_array(&["A"], "braced", true)),
        text.replace("braced = { \"a\" };", "braced[] = { \"a\" };")
    );
}

#[test]
fn set_array_refuses_a_scalar_value_that_cannot_be_a_list() {
    let err = plan(SHAPES, &set_array(&["A"], "scalar", true)).unwrap_err();
    assert!(matches!(err, EditError::InvalidValue(_)), "{err:?}");
}

#[test]
fn set_array_refuses_a_list_value_that_cannot_drop_the_brackets() {
    let err = plan(SHAPES, &set_array(&["A"], "already", false)).unwrap_err();
    assert!(matches!(err, EditError::InvalidValue(_)), "{err:?}");
}

#[test]
fn set_array_needs_a_property() {
    let err = plan(SHAPES, &set_array(&["A"], "missing", true)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    let op = EditOp::SetArray {
        target: class(&["A"]),
        array: true,
    };
    assert!(matches!(
        plan(SHAPES, &op).unwrap_err(),
        EditError::InvalidValue(_)
    ));
}

const DUP_PROPS: &str = "class A
{
    x = 1;
    X = 2;
    y = 3;
};
";

#[test]
fn remove_at_removes_the_first_occurrence() {
    assert_eq!(
        edit(DUP_PROPS, &remove_at(&["A"], "x", 0)),
        "class A\n{\n    X = 2;\n    y = 3;\n};\n"
    );
}

#[test]
fn remove_at_removes_the_second_occurrence() {
    assert_eq!(
        edit(DUP_PROPS, &remove_at(&["A"], "x", 1)),
        "class A\n{\n    x = 1;\n    y = 3;\n};\n"
    );
}

#[test]
fn remove_at_on_a_one_line_class_keeps_the_surviving_member() {
    assert_eq!(
        edit("class A { x = 1; x = 2; };\n", &remove_at(&["A"], "x", 1)),
        "class A { x = 1; };\n"
    );
    assert_eq!(
        edit("class A { x = 1; x = 2; };\n", &remove_at(&["A"], "x", 0)),
        "class A { x = 2; };\n"
    );
}

#[test]
fn remove_at_does_not_count_delete_statements() {
    let text = "class A\n{\n    delete x;\n    x = 1;\n};\n";
    assert_eq!(
        edit(text, &remove_at(&["A"], "x", 0)),
        "class A\n{\n    delete x;\n};\n"
    );
}

#[test]
fn remove_at_can_remove_a_duplicate_class() {
    let text =
        "class P\n{\n    class C\n    {\n    };\n    class C\n    {\n        a = 1;\n    };\n};\n";
    assert_eq!(
        edit(text, &remove_at(&["P"], "C", 1)),
        "class P\n{\n    class C\n    {\n    };\n};\n"
    );
    assert_eq!(
        edit(text, &remove_at(&["P"], "C", 0)),
        "class P\n{\n    class C\n    {\n        a = 1;\n    };\n};\n"
    );
}

#[test]
fn remove_at_out_of_range_is_not_found() {
    let err = plan(DUP_PROPS, &remove_at(&["A"], "x", 2)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    let err = plan(DUP_PROPS, &remove_at(&["A"], "z", 0)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    let err = plan(DUP_PROPS, &remove_at(&["B"], "x", 0)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}
