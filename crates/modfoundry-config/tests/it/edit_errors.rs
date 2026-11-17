//! Every `EditError` variant, with the exact text users see.

use crate::edit_fixtures as fixtures;

use fixtures::*;
use modfoundry_config::edit::*;

fn err(text: &str, op: &EditOp) -> EditError {
    plan(text, op).expect_err("the edit should be refused")
}

#[test]
fn unparseable_text() {
    for text in [
        "class A { x = ",
        "class A { x = 1 }",
        "class A { s = \"open; };",
    ] {
        let e = err(text, &set(&["A"], "x", n("1")));
        assert_eq!(e, EditError::Unparseable);
    }
    assert_eq!(
        EditError::Unparseable.to_string(),
        "The file has syntax errors. Fix them in Source before editing here."
    );
}

#[test]
fn not_found() {
    let e = err(CANON, &set(&["CfgVehicles", "Nope"], "x", n("1")));
    assert_eq!(e, EditError::NotFound("Class `CfgVehicles/Nope`".into()));
    assert_eq!(e.to_string(), "Class `CfgVehicles/Nope` was not found");

    let e = err(CANON, &set(&ITEM, "nope", n("1")));
    assert_eq!(e.to_string(), "Property `nope` was not found");

    let e = err(
        CANON,
        &add(&ITEM, "w", n("1"), Position::After("zzz".into())),
    );
    assert_eq!(e.to_string(), "Member `zzz` was not found");

    let e = err(CANON, &set_base(&[], None));
    assert!(matches!(e, EditError::NotFound(_)));

    let e = err(CANON, &remove(class(&["CfgVehicles", "Base", "Nope"])));
    assert!(matches!(e, EditError::NotFound(_)));
}

#[test]
fn ambiguous_siblings() {
    let e = err("class A { x = 1; x = 2; };", &set(&["A"], "x", n("3")));
    assert_eq!(e, EditError::Ambiguous("Property `x`".into()));
    assert_eq!(
        e.to_string(),
        "Property `x` is declared more than once here"
    );

    let e = err(
        "class A {};\nclass a {};\n",
        &add(&["A"], "x", n("1"), Position::Last),
    );
    assert_eq!(e.to_string(), "Class `A` is declared more than once here");

    let e = err(
        "class A { x = 1; class X {}; };",
        &add(&["A"], "y", n("1"), Position::Before("x".into())),
    );
    assert!(matches!(e, EditError::Ambiguous(_)));
}

#[test]
fn exists() {
    let e = err(CANON, &add(&ITEM, "SCOPE", n("1"), Position::Last));
    assert_eq!(e, EditError::Exists("`SCOPE`".into()));
    assert_eq!(e.to_string(), "`SCOPE` already exists here");

    let e = err(CANON, &rename(prop(&ITEM, "displayName"), "Scope"));
    assert!(matches!(e, EditError::Exists(_)));

    let e = err(CANON, &add_class(&ITEM, "sub", None, false, Position::Last));
    assert!(matches!(e, EditError::Exists(_)));

    let e = err(
        CANON,
        &rename(class(&["CfgVehicles", "Item", "Sub"]), "scope"),
    );
    assert!(
        matches!(e, EditError::Exists(_)),
        "classes and properties share one namespace"
    );
}

#[test]
fn invalid_names() {
    for bad in ["", "1bad", "a b", "a-b", "class", "delete", "external", "ä"] {
        let e = err(CANON, &add(&ITEM, bad, n("1"), Position::Last));
        assert_eq!(e, EditError::InvalidName(bad.into()), "{bad:?}");
    }
    assert_eq!(
        EditError::InvalidName("a b".into()).to_string(),
        "`a b` isn't a valid name"
    );
    let e = err(CANON, &rename(prop(&ITEM, "scope"), "no good"));
    assert!(matches!(e, EditError::InvalidName(_)));
    let e = err(CANON, &add_class(&ITEM, "x-y", None, false, Position::Last));
    assert!(matches!(e, EditError::InvalidName(_)));
    let e = err(
        CANON,
        &add_class(&ITEM, "Ok", Some("bad name"), false, Position::Last),
    );
    assert_eq!(e, EditError::InvalidName("bad name".into()));
    let e = err(CANON, &set_base(&ITEM, Some("1x")));
    assert!(matches!(e, EditError::InvalidName(_)));
}

#[test]
fn invalid_values() {
    let e = err(CANON, &set(&ITEM, "displayName", s("a\"b")));
    assert_eq!(
        e,
        EditError::InvalidValue("Strings can't contain \"".into())
    );
    assert_eq!(e.to_string(), "Strings can't contain \"");

    let e = err(CANON, &set(&ITEM, "displayName", s("a\nb")));
    assert_eq!(e.to_string(), "Strings can't contain a line break");

    let e = err(CANON, &set(&ITEM, "scope", n("abc")));
    assert_eq!(e.to_string(), "`abc` isn't a valid number");

    let e = err(CANON, &set(&ITEM, "scope", id("a b")));
    assert_eq!(e.to_string(), "`a b` isn't a valid identifier");

    let e = err(
        CANON,
        &add(&ITEM, "w", arr(vec![s("a\"b")]), Position::Last),
    );
    assert!(matches!(e, EditError::InvalidValue(_)));

    let e = err(CANON, &set(&ITEM, "itemSize", n("1")));
    assert_eq!(
        e.to_string(),
        "`itemSize[]` is an array, so it needs a list"
    );

    let e = err(CANON, &set(&ITEM, "scope", arr(vec![n("1")])));
    assert_eq!(
        e.to_string(),
        "`scope` isn't declared as an array, so it can't take a list"
    );

    let e = err(
        CANON,
        &EditOp::SetValue {
            property: class(&ITEM),
            value: n("1"),
        },
    );
    assert_eq!(e.to_string(), "This edit needs a property, not a class");
}

#[test]
fn not_an_array() {
    let e = err(CANON, &insert(&ITEM, "scope", 0, n("1")));
    assert_eq!(e, EditError::NotAnArray("scope".into()));
    assert_eq!(e.to_string(), "`scope` isn't an array");
    assert!(matches!(
        err(CANON, &remove_item(&ITEM, "scope", 0)),
        EditError::NotAnArray(_)
    ));
    assert!(matches!(
        err(CANON, &move_item(&ITEM, "scope", 0, 1)),
        EditError::NotAnArray(_)
    ));
}

#[test]
fn index_out_of_range() {
    let e = err(CANON, &insert(&ITEM, "itemSize", 3, n("1")));
    assert_eq!(e, EditError::IndexOutOfRange { len: 2, index: 3 });
    assert_eq!(e.to_string(), "Index 3 is out of range for an array of 2");
    assert_eq!(
        err(CANON, &remove_item(&ITEM, "itemSize", 2)),
        EditError::IndexOutOfRange { len: 2, index: 2 }
    );
    assert_eq!(
        err(CANON, &set_item(&ITEM, "itemSize", 2, n("1"))),
        EditError::IndexOutOfRange { len: 2, index: 2 }
    );
    assert_eq!(
        err(CANON, &move_item(&ITEM, "itemSize", 0, 2)),
        EditError::IndexOutOfRange { len: 2, index: 2 }
    );
    assert_eq!(
        err(CANON, &move_item(&ITEM, "itemSize", 5, 0)),
        EditError::IndexOutOfRange { len: 2, index: 5 }
    );
}

#[test]
fn read_only() {
    let e = err(ARRAYS, &set_item(&["A"], "nested", 0, n("1")));
    assert_eq!(
        e,
        EditError::ReadOnly("Nested lists and expressions can only be replaced as a whole".into())
    );
    assert_eq!(
        e.to_string(),
        "Nested lists and expressions can only be replaced as a whole"
    );

    let e = err("class A { delete X; };", &remove(prop(&["A"], "X")));
    assert_eq!(e.to_string(), "`delete X;` statements are edited in Source");

    let e = err(
        CANON,
        &add(&["CfgVehicles", "Base"], "x", n("1"), Position::Last),
    );
    assert_eq!(
        e.to_string(),
        "Class `CfgVehicles/Base` is a forward declaration and has no body"
    );

    let e = err(
        "class A { x[] = { 1 + 2 }; };\n",
        &set_item(&["A"], "x", 0, n("3")),
    );
    assert!(
        matches!(e, EditError::ReadOnly(_)),
        "an expression item can only be set as a whole"
    );
}

#[test]
fn no_op_edits_plan_nothing() {
    assert!(plan(CANON, &set(&ITEM, "scope", n("2")))
        .unwrap()
        .is_empty());
    assert!(plan(CANON, &rename(prop(&ITEM, "scope"), "scope"))
        .unwrap()
        .is_empty());
    assert!(plan(CANON, &set_base(&ITEM, Some("Base")))
        .unwrap()
        .is_empty());
    assert!(plan(CANON, &set_base(&["CfgVehicles", "Base"], None))
        .unwrap()
        .is_empty());
    assert!(plan(CANON, &move_item(&ITEM, "itemSize", 1, 1))
        .unwrap()
        .is_empty());
    assert!(
        plan(CANON, &set(&ITEM, "itemSize", arr(vec![n("1"), n("2")])))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn paths_match_case_insensitively() {
    let out = edit(CANON, &set(&["cfgvehicles", "ITEM"], "SCOPE", n("1")));
    assert!(out.contains("scope = 1;"));
}
