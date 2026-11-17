//! Golden before/after tests for every edit op in each supported file style.

use crate::edit_fixtures as fixtures;

use fixtures::*;
use modfoundry_config::edit::*;

struct Case {
    name: &'static str,
    op: EditOp,
    find: &'static str,
    replace: &'static str,
}

fn case(name: &'static str, op: EditOp, find: &'static str, replace: &'static str) -> Case {
    Case {
        name,
        op,
        find,
        replace,
    }
}

fn expect(base: &str, c: &Case, adjust: &dyn Fn(&str) -> String) -> String {
    let find = adjust(c.find);
    let replace = adjust(c.replace);
    if find.is_empty() {
        return format!("{base}{replace}");
    }
    assert_eq!(
        base.matches(&find).count(),
        1,
        "{}: find must be unique",
        c.name
    );
    base.replacen(&find, &replace, 1)
}

fn run(label: &str, base: &str, cases: &[Case], adjust: &dyn Fn(&str) -> String) {
    for c in cases {
        let got = edit(base, &c.op);
        assert_eq!(got, expect(base, c, adjust), "{label}/{}", c.name);
    }
}

const SUB: &str = "        class Sub\n        {\n            x = 1;\n        };\n";

fn canon_cases() -> Vec<Case> {
    vec![
        case(
            "set string",
            set(&ITEM, "displayName", s("Canteen")),
            "displayName = \"Item\";",
            "displayName = \"Canteen\";",
        ),
        case("set number", set(&ITEM, "scope", n("1")), "scope = 2;", "scope = 1;"),
        case("set ident", set(&ITEM, "scope", id("public")), "scope = 2;", "scope = public;"),
        case(
            "set array",
            set(&ITEM, "itemSize", arr(vec![n("3"), n("4")])),
            "{ 1, 2 }",
            "{ 3, 4 }",
        ),
        case(
            "set empty array",
            set(&ITEM, "itemSize", arr(vec![])),
            "itemSize[] = { 1, 2 };",
            "itemSize[] = {};",
        ),
        case(
            "add last",
            add(&ITEM, "weight", n("450"), Position::Last),
            SUB,
            "        class Sub\n        {\n            x = 1;\n        };\n        weight = 450;\n",
        ),
        case(
            "add first",
            add(&ITEM, "weight", n("450"), Position::First),
            "    {\n        // display\n",
            "    {\n        weight = 450;\n        // display\n",
        ),
        case(
            "add after",
            add(&ITEM, "weight", n("450"), Position::After("scope".into())),
            "        scope = 2;\n",
            "        scope = 2;\n        weight = 450;\n",
        ),
        case(
            "add before keeps the leading comment attached",
            add(&ITEM, "weight", n("450"), Position::Before("DISPLAYNAME".into())),
            "        // display\n",
            "        weight = 450;\n        // display\n",
        ),
        case(
            "add array",
            add(&ITEM, "hiddenSelections", arr(vec![s("zbytek")]), Position::Last),
            SUB,
            "        class Sub\n        {\n            x = 1;\n        };\n        hiddenSelections[] = { \"zbytek\" };\n",
        ),
        case(
            "add empty array",
            add(&ITEM, "hiddenSelections", arr(vec![]), Position::After("scope".into())),
            "        scope = 2;\n",
            "        scope = 2;\n        hiddenSelections[] = {};\n",
        ),
        case("remove property", remove(prop(&ITEM, "scope")), "        scope = 2;\n", ""),
        case(
            "remove keeps the comment above",
            remove(prop(&ITEM, "displayName")),
            "        displayName = \"Item\";\n",
            "",
        ),
        case("remove class", remove(class(&["CfgVehicles", "Item", "Sub"])), SUB, ""),
        case(
            "rename property",
            rename(prop(&ITEM, "displayName"), "label"),
            "displayName = \"Item\";",
            "label = \"Item\";",
        ),
        case(
            "rename class",
            rename(class(&["CfgVehicles", "Item", "Sub"]), "Inner"),
            "        class Sub\n",
            "        class Inner\n",
        ),
        case(
            "rename to a different case of itself",
            rename(prop(&ITEM, "scope"), "Scope"),
            "scope = 2;",
            "Scope = 2;",
        ),
        case(
            "add class with a base",
            add_class(&ITEM, "Extra", Some("Base"), false, Position::Last),
            SUB,
            "        class Sub\n        {\n            x = 1;\n        };\n        class Extra: Base\n        {\n        };\n",
        ),
        case(
            "add class before",
            add_class(&ITEM, "Extra", None, false, Position::Before("Sub".into())),
            "        class Sub\n",
            "        class Extra\n        {\n        };\n        class Sub\n",
        ),
        case(
            "add forward class last",
            add_class(&["CfgVehicles"], "Other", None, true, Position::Last),
            "        };\n    };\n};\n",
            "        };\n    };\n    class Other;\n};\n",
        ),
        case(
            "add forward class with base after",
            add_class(&["CfgVehicles"], "Mid", Some("Base"), true, Position::After("Base".into())),
            "    class Base;\n",
            "    class Base;\n    class Mid: Base;\n",
        ),
        case(
            "add class at the file root",
            add_class(&[], "CfgExtra", None, false, Position::Last),
            "",
            "class CfgExtra\n{\n};\n",
        ),
        case(
            "add class first at the file root",
            add_class(&[], "CfgFirst", None, false, Position::First),
            "class CfgPatches\n{\n    class MyMod",
            "class CfgFirst\n{\n};\nclass CfgPatches\n{\n    class MyMod",
        ),
        case(
            "set base replaces",
            set_base(&ITEM, Some("Other")),
            "class Item: Base\n",
            "class Item: Other\n",
        ),
        case("set base removes", set_base(&ITEM, None), "class Item: Base\n", "class Item\n"),
        case(
            "set base inserts the file's colon style",
            set_base(&["CfgVehicles", "Item", "Sub"], Some("Base")),
            "        class Sub\n",
            "        class Sub: Base\n",
        ),
        case(
            "array insert middle",
            insert(&ITEM, "itemSize", 1, n("9")),
            "{ 1, 2 }",
            "{ 1, 9, 2 }",
        ),
        case("array insert first", insert(&ITEM, "itemSize", 0, n("9")), "{ 1, 2 }", "{ 9, 1, 2 }"),
        case("array append", insert(&ITEM, "itemSize", 2, n("9")), "{ 1, 2 }", "{ 1, 2, 9 }"),
        case(
            "array insert into empty",
            insert(&MYMOD, "units", 0, s("u")),
            "units[] = {};",
            "units[] = { \"u\" };",
        ),
        case("array remove first", remove_item(&ITEM, "itemSize", 0), "{ 1, 2 }", "{ 2 }"),
        case("array remove last", remove_item(&ITEM, "itemSize", 1), "{ 1, 2 }", "{ 1 }"),
        case("array remove only item", remove_item(&MYMOD, "requiredAddons", 0), "{ \"DZ_Data\" }", "{}"),
        case("array set", set_item(&ITEM, "itemSize", 1, n("7")), "{ 1, 2 }", "{ 1, 7 }"),
        case("array move", move_item(&ITEM, "itemSize", 0, 1), "{ 1, 2 }", "{ 2, 1 }"),
        case(
            "array insert string",
            insert(&MYMOD, "requiredAddons", 1, s("DZ_Gear")),
            "{ \"DZ_Data\" }",
            "{ \"DZ_Data\", \"DZ_Gear\" }",
        ),
    ]
}

#[test]
fn canonical_allman_in_every_indent_and_eol_style() {
    let styles: [(&str, &str, bool); 5] = [
        ("spaces4", "    ", false),
        ("tabs", "\t", false),
        ("spaces2", "  ", false),
        ("crlf", "    ", true),
        ("tabs+crlf", "\t", true),
    ];
    for (label, indent, use_crlf) in styles {
        let base = restyle(CANON, indent, use_crlf);
        let adjust = move |t: &str| restyle(t, indent, use_crlf);
        run(label, &base, &canon_cases(), &adjust);
    }
}

#[test]
fn knr_braces() {
    let cases = [
        case(
            "add class",
            add_class(&ITEM, "Extra", Some("Base"), false, Position::Last),
            "        scope = 2;\n",
            "        scope = 2;\n        class Extra: Base {\n        };\n",
        ),
        case(
            "add forward",
            add_class(&ITEM, "Extra", None, true, Position::Last),
            "        scope = 2;\n",
            "        scope = 2;\n        class Extra;\n",
        ),
        case(
            "add property first",
            add(&ITEM, "weight", n("1"), Position::First),
            "class Item: Base {\n",
            "class Item: Base {\n        weight = 1;\n",
        ),
        case(
            "remove property",
            remove(prop(&ITEM, "scope")),
            "        scope = 2;\n",
            "",
        ),
        case(
            "remove class",
            remove(class(&ITEM)),
            "    class Item: Base {\n        scope = 2;\n    };\n",
            "",
        ),
        case(
            "rename",
            rename(class(&ITEM), "Thing"),
            "class Item:",
            "class Thing:",
        ),
        case(
            "set base none",
            set_base(&ITEM, None),
            "class Item: Base {",
            "class Item {",
        ),
    ];
    run("knr", KNR, &cases, &|t| t.to_owned());
}

#[test]
fn one_line_classes_reflow_only_when_something_is_inserted() {
    let cases = [
        case(
            "set keeps layout",
            set(&["A"], "x", n("5")),
            "x = 1;",
            "x = 5;",
        ),
        case(
            "set array keeps layout",
            set(&["A"], "y", arr(vec![n("3"), n("4")])),
            "{ 1, 2 }",
            "{ 3, 4 }",
        ),
        case(
            "add last",
            add(&["A"], "z", n("3"), Position::Last),
            "class A { x = 1; y[] = { 1, 2 }; };\n",
            "class A {\n    x = 1;\n    y[] = { 1, 2 };\n    z = 3;\n};\n",
        ),
        case(
            "add into an empty body",
            add(&["B"], "w", n("1"), Position::First),
            "class B { };\n",
            "class B {\n    w = 1;\n};\n",
        ),
        case(
            "add into a tight empty body",
            add(&["C"], "w", n("1"), Position::Last),
            "class C {};\n",
            "class C {\n    w = 1;\n};\n",
        ),
        case(
            "add class first",
            add_class(&["A"], "Inner", None, false, Position::First),
            "class A { x = 1; y[] = { 1, 2 }; };\n",
            "class A {\n    class Inner {\n    };\n    x = 1;\n    y[] = { 1, 2 };\n};\n",
        ),
        case(
            "remove property",
            remove(prop(&["A"], "x")),
            "class A { x = 1; y[]",
            "class A { y[]",
        ),
        case("remove class", remove(class(&["B"])), "class B { };\n", ""),
        case(
            "array insert",
            insert(&["A"], "y", 1, n("9")),
            "{ 1, 2 }",
            "{ 1, 9, 2 }",
        ),
        case(
            "add class at the root",
            add_class(&[], "D", None, false, Position::Last),
            "",
            "class D {\n};\n",
        ),
    ];
    run("one-line", ONE_LINE, &cases, &|t| t.to_owned());
}

fn vanilla_cases() -> Vec<Case> {
    vec![
        case(
            "set scalar",
            set(&ITEM, "scope", n("1")),
            "scope=2;",
            "scope=1;",
        ),
        case(
            "set multi-line array",
            set(&MYMOD, "requiredAddons", arr(vec![s("A"), s("B"), s("C")])),
            "\t\t{\n\t\t\t\"DZ_Data\",\n\t\t\t\"DZ_Gear\"\n\t\t}",
            "\t\t{\n\t\t\t\"A\",\n\t\t\t\"B\",\n\t\t\t\"C\"\n\t\t}",
        ),
        case(
            "set tight array",
            set(&ITEM, "itemSize", arr(vec![n("3"), n("4")])),
            "{2,2}",
            "{3, 4}",
        ),
        case(
            "add scalar",
            add(&ITEM, "weight", n("450"), Position::Last),
            "\t\titemSize[]={2,2};\n",
            "\t\titemSize[]={2,2};\n\t\tweight=450;\n",
        ),
        case(
            "add multi-line array",
            add(
                &ITEM,
                "hiddenSelections",
                arr(vec![s("zbytek")]),
                Position::Last,
            ),
            "\t\titemSize[]={2,2};\n",
            "\t\titemSize[]={2,2};\n\t\thiddenSelections[]=\n\t\t{\n\t\t\t\"zbytek\"\n\t\t};\n",
        ),
        case(
            "add class",
            add_class(
                &["CfgPatches"],
                "Other",
                Some("Base"),
                false,
                Position::Last,
            ),
            "\t};\n};\nclass CfgVehicles",
            "\t};\n\tclass Other: Base\n\t{\n\t};\n};\nclass CfgVehicles",
        ),
        case(
            "insert middle",
            insert(&MYMOD, "requiredAddons", 1, s("DZ_Mid")),
            "\t\t\t\"DZ_Data\",\n",
            "\t\t\t\"DZ_Data\",\n\t\t\t\"DZ_Mid\",\n",
        ),
        case(
            "append",
            insert(&MYMOD, "requiredAddons", 2, s("DZ_New")),
            "\t\t\t\"DZ_Gear\"\n",
            "\t\t\t\"DZ_Gear\",\n\t\t\t\"DZ_New\"\n",
        ),
        case(
            "remove first",
            remove_item(&MYMOD, "requiredAddons", 0),
            "\t\t\t\"DZ_Data\",\n",
            "",
        ),
        case(
            "remove last drops the comma before it",
            remove_item(&MYMOD, "requiredAddons", 1),
            "\t\t\t\"DZ_Data\",\n\t\t\t\"DZ_Gear\"\n",
            "\t\t\t\"DZ_Data\"\n",
        ),
        case(
            "move",
            move_item(&MYMOD, "requiredAddons", 0, 1),
            "\t\t\t\"DZ_Data\",\n\t\t\t\"DZ_Gear\"\n",
            "\t\t\t\"DZ_Gear\",\n\t\t\t\"DZ_Data\"\n",
        ),
        case(
            "set item",
            set_item(&MYMOD, "requiredAddons", 0, s("X")),
            "\"DZ_Data\",",
            "\"X\",",
        ),
        case(
            "insert into an empty array",
            insert(&MYMOD, "units", 0, s("u")),
            "units[]={};",
            "units[]={\n\t\t\t\"u\"\n\t\t};",
        ),
        case(
            "rename",
            rename(prop(&MYMOD, "requiredVersion"), "ver"),
            "requiredVersion=",
            "ver=",
        ),
        case("remove", remove(prop(&ITEM, "scope")), "\t\tscope=2;\n", ""),
        case(
            "set base replaces",
            set_base(&ITEM, Some("Other")),
            "class Item: Base",
            "class Item: Other",
        ),
        case(
            "set base on a forward declaration",
            set_base(&["CfgVehicles", "Base"], Some("X")),
            "class Base;",
            "class Base: X;",
        ),
    ]
}

#[test]
fn derapified_vanilla_style() {
    let base = crlf(VANILLA_LF);
    run("vanilla", &base, &vanilla_cases(), &crlf);
}

#[test]
fn scaffold_style() {
    let base = scaffold_config();
    let mods = ["CfgPatches", "MyMod_Scripts"];
    let cases = [
        case(
            "add property",
            add(&mods, "extra", s("x"), Position::Last),
            "        requiredAddons[] = { \"DZ_Data\" };\n",
            "        requiredAddons[] = { \"DZ_Data\" };\n        extra = \"x\";\n",
        ),
        case(
            "array insert",
            insert(&mods, "requiredAddons", 1, s("DZ_Gear")),
            "{ \"DZ_Data\" }",
            "{ \"DZ_Data\", \"DZ_Gear\" }",
        ),
        case(
            "add class",
            add_class(
                &["CfgPatches"],
                "Second",
                Some("MyMod_Scripts"),
                false,
                Position::Last,
            ),
            "    };\n};\n\nclass CfgMods",
            "    };\n    class Second: MyMod_Scripts\n    {\n    };\n};\n\nclass CfgMods",
        ),
        case(
            "set nested property",
            set(&["CfgMods", "MyMod"], "type", s("servermod")),
            "type = \"mod\";",
            "type = \"servermod\";",
        ),
        case(
            "remove",
            remove(prop(&mods, "weapons")),
            "        weapons[] = {};\n",
            "",
        ),
    ];
    run("scaffold", &base, &cases, &|t| t.to_owned());
}

#[test]
fn comments_and_preprocessor_lines_survive() {
    let a = ["A"];
    let cases = [
        case(
            "set beside a comment",
            set(&a, "a", n("5")),
            "a = 1; // one",
            "a = 5; // one",
        ),
        case(
            "remove beside a comment",
            remove(prop(&a, "a")),
            "    a = 1; // one\n",
            "    // one\n",
        ),
        case(
            "remove after a block comment",
            remove(prop(&a, "b")),
            "    b = 2;\n",
            "",
        ),
        case(
            "add last goes after trailing members",
            add(&a, "d", n("1"), Position::Last),
            "    };\n};\n",
            "    };\n    d = 1;\n};\n",
        ),
        case(
            "add after a member with a trailing comment",
            add(&a, "d", n("1"), Position::After("a".into())),
            "    a = 1; // one\n",
            "    a = 1; // one\n    d = 1;\n",
        ),
        case(
            "add before a block-commented member",
            add(&a, "d", n("1"), Position::Before("b".into())),
            "    b = 2;\n",
            "    d = 1;\n    b = 2;\n",
        ),
        case(
            "array insert between commented items",
            insert(&a, "c", 1, s("z")),
            "        \"x\", // first\n",
            "        \"x\", // first\n        \"z\",\n",
        ),
        case(
            "array append",
            insert(&a, "c", 2, s("z")),
            "        \"y\"\n",
            "        \"y\",\n        \"z\"\n",
        ),
        case(
            "array remove first drops its comment line",
            remove_item(&a, "c", 0),
            "        \"x\", // first\n",
            "",
        ),
        case(
            "array remove last drops the comma before it",
            remove_item(&a, "c", 1),
            "        \"x\", // first\n        \"y\"\n",
            "        \"x\" // first\n",
        ),
        case(
            "rename",
            rename(prop(&a, "c"), "d"),
            "    c[] = {",
            "    d[] = {",
        ),
        case(
            "add class first goes above preprocessor lines",
            add_class(&a, "New", None, false, Position::First),
            "{\n    #ifdef X",
            "{\n    class New\n    {\n    };\n    #ifdef X",
        ),
    ];
    run("comments", COMMENTS, &cases, &|t| t.to_owned());
}

#[test]
fn array_edge_layouts() {
    let a = ["A"];
    let cases = [
        case(
            "trailing comma append",
            insert(&a, "trail", 2, s("c")),
            "{ \"a\", \"b\", }",
            "{ \"a\", \"b\", \"c\", }",
        ),
        case(
            "trailing comma remove last",
            remove_item(&a, "trail", 1),
            "{ \"a\", \"b\", }",
            "{ \"a\", }",
        ),
        case(
            "trailing comma remove first",
            remove_item(&a, "trail", 0),
            "{ \"a\", \"b\", }",
            "{ \"b\", }",
        ),
        case(
            "trailing comma move",
            move_item(&a, "trail", 0, 1),
            "{ \"a\", \"b\", }",
            "{ \"b\", \"a\", }",
        ),
        case(
            "tight insert",
            insert(&a, "tight", 1, n("9")),
            "{1,2}",
            "{1,9,2}",
        ),
        case(
            "tight append",
            insert(&a, "tight", 2, n("9")),
            "{1,2}",
            "{1,2,9}",
        ),
        case(
            "plus-equals insert",
            insert(&a, "plus", 2, n("3")),
            "plus[] += { 1, 2 };",
            "plus[] += { 1, 2, 3 };",
        ),
        case(
            "nested insert",
            insert(&a, "nested", 1, arr(vec![n("5")])),
            "{ { 1, 2 }, { 3 } }",
            "{ { 1, 2 }, { 5 }, { 3 } }",
        ),
        case(
            "nested set as a whole",
            set(&a, "nested", arr(vec![arr(vec![n("1")])])),
            "{ { 1, 2 }, { 3 } }",
            "{ { 1 } }",
        ),
        case(
            "multi-line append keeps the trailing comma",
            insert(&a, "lines", 2, s("c")),
            "        \"b\",\n    };",
            "        \"b\",\n        \"c\",\n    };",
        ),
        case(
            "multi-line remove last keeps the trailing comma",
            remove_item(&a, "lines", 1),
            "        \"a\",\n        \"b\",\n",
            "        \"a\",\n",
        ),
        case(
            "multi-line remove first",
            remove_item(&a, "lines", 0),
            "        \"a\",\n        \"b\",\n",
            "        \"b\",\n",
        ),
        case(
            "multi-line insert first",
            insert(&a, "lines", 0, s("z")),
            "lines[] = {\n        \"a\",",
            "lines[] = {\n        \"z\",\n        \"a\",",
        ),
        case(
            "multi-line move",
            move_item(&a, "lines", 0, 1),
            "        \"a\",\n        \"b\",\n",
            "        \"b\",\n        \"a\",\n",
        ),
        case(
            "insert into empty",
            insert(&a, "none", 0, s("x")),
            "none[] = {};",
            "none[] = { \"x\" };",
        ),
    ];
    run("arrays", ARRAYS, &cases, &|t| t.to_owned());
}

#[test]
fn plan_all_applies_ops_in_order() {
    let ops = [
        add(&ITEM, "weight", n("450"), Position::Last),
        set(&ITEM, "weight", n("500")),
        rename(prop(&ITEM, "weight"), "mass"),
    ];
    let out = plan_all(CANON, &ops).unwrap();
    assert!(out.contains("        mass = 500;\n"));
    assert!(!out.contains("weight"));
    assert!(plan_all(
        CANON,
        &[remove(prop(&ITEM, "scope")), set(&ITEM, "scope", n("1"))]
    )
    .is_err());
}

#[test]
fn edits_are_sorted_and_non_overlapping() {
    let edits = plan(ARRAYS, &remove_item(&["A"], "lines", 1)).unwrap();
    assert_eq!(edits.len(), 1);
    let edits = plan(ARRAYS, &move_item(&["A"], "lines", 0, 1)).unwrap();
    assert!(edits
        .windows(2)
        .all(|w| w[0].offset + w[0].len <= w[1].offset));
}
