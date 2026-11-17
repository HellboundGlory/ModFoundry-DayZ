//! The XML edit engine, part 1 — paths, attributes, text and rename. Each
//! op is planned into byte splices over the original text; the tree is only read.

use crate::common::fixtures::{assert_op, edit_run};
use modfoundry_ce::edit::{
    apply, path_to, plan, plan_all, resolve, EditError, Pick, Step, TextEdit, XmlOp, XmlPath,
};
use modfoundry_ce::xml::{parse, NodeKind};
use serde_json::json;

fn nth(name: &str, index: usize) -> Step {
    Step {
        name: name.into(),
        pick: Pick::Nth(index),
    }
}

fn by_attr(name: &str, attr: &str, value: &str) -> Step {
    Step {
        name: name.into(),
        pick: Pick::Attr {
            name: attr.into(),
            value: value.into(),
        },
    }
}

fn path(steps: Vec<Step>) -> XmlPath {
    XmlPath(steps)
}

fn set_attr(element: XmlPath, name: &str, value: &str) -> XmlOp {
    XmlOp::SetAttr {
        element,
        name: name.into(),
        value: value.into(),
    }
}

fn set_text(element: XmlPath, text: &str) -> XmlOp {
    XmlOp::SetText {
        element,
        text: text.into(),
    }
}

fn rename(element: XmlPath, name: &str) -> XmlOp {
    XmlOp::RenameElement {
        element,
        name: name.into(),
    }
}

#[test]
fn set_attr_quotes_an_unquoted_value() {
    assert_op(
        "<x a=1/>",
        &set_attr(path(vec![nth("x", 0)]), "a", "two words"),
        "<x a=\"two words\"/>",
    );
}

#[test]
fn set_attr_replaces_only_the_value() {
    let text = "<types>\n    <type name=\"FieldCanteen\">\n        <nominal>12</nominal>\n    </type>\n</types>\n";
    let want =
        "<types>\n    <type name=\"Canteen\">\n        <nominal>12</nominal>\n    </type>\n</types>\n";
    assert_op(
        text,
        &set_attr(
            path(vec![nth("types", 0), nth("type", 0)]),
            "name",
            "Canteen",
        ),
        want,
    );
}

#[test]
fn set_attr_new_on_element_without_attributes() {
    assert_op(
        "<types/>",
        &set_attr(path(vec![nth("types", 0)]), "count", "2"),
        "<types count=\"2\"/>",
    );
}

#[test]
fn set_attr_new_keeps_self_close_style() {
    assert_op(
        "<types/>",
        &set_attr(path(vec![nth("types", 0)]), "count", "2"),
        "<types count=\"2\"/>",
    );
    assert_op(
        "<types />",
        &set_attr(path(vec![nth("types", 0)]), "count", "2"),
        "<types count=\"2\" />",
    );
}

#[test]
fn set_attr_new_goes_after_the_last_attribute() {
    assert_op(
        "<type name=\"A\" nominal=\"1\"/>",
        &set_attr(path(vec![nth("type", 0)]), "min", "2"),
        "<type name=\"A\" nominal=\"1\" min=\"2\"/>",
    );
    assert_op(
        "<type name = \"A\" />",
        &set_attr(path(vec![nth("type", 0)]), "min", "2"),
        "<type name = \"A\" min=\"2\" />",
    );
}

#[test]
fn set_attr_keeps_the_quote_character() {
    assert_op(
        "<type name='A'/>",
        &set_attr(path(vec![nth("type", 0)]), "name", "B"),
        "<type name='B'/>",
    );
    assert_op(
        "<type name='A'/>",
        &set_attr(path(vec![nth("type", 0)]), "name", "B's"),
        "<type name='B&apos;s'/>",
    );
}

#[test]
fn set_attr_crlf_tabs() {
    let text = "<types>\r\n\t<type name=\"A\">\r\n\t\t<flags count_in_map=\"1\"/>\r\n\t</type>\r\n</types>";
    let want =
        "<types>\r\n\t<type name=\"A\">\r\n\t\t<flags count_in_map=\"0\"/>\r\n\t</type>\r\n</types>";
    assert_op(
        text,
        &set_attr(
            path(vec![nth("types", 0), nth("type", 0), nth("flags", 0)]),
            "count_in_map",
            "0",
        ),
        want,
    );
}

#[test]
fn set_attr_bom() {
    let text = "\u{FEFF}<types name=\"A\"/>";
    let want = "\u{FEFF}<types name=\"B\"/>";
    assert_op(
        text,
        &set_attr(path(vec![nth("types", 0)]), "name", "B"),
        want,
    );
}

#[test]
fn set_attr_comments_around_the_edit_stay() {
    let text = "<!--before-->\n<type name=\"A\">\n    <!--inside-->\n    <nominal>1</nominal>\n</type>\n<!--after-->\n";
    let want = "<!--before-->\n<type name=\"A\">\n    <!--inside-->\n    <nominal value=\"2\">1</nominal>\n</type>\n<!--after-->\n";
    assert_op(
        text,
        &set_attr(path(vec![nth("type", 0), nth("nominal", 0)]), "value", "2"),
        want,
    );
}

#[test]
fn set_attr_value_round_trips_through_parse() {
    for (text, quote_value) in [("<a b=\"1\"/>", "x&<\"y"), ("<a b='1'/>", "x&<'y")] {
        let out = edit_run(text, &set_attr(path(vec![nth("a", 0)]), "b", quote_value));
        let doc = parse(&out);
        assert_eq!(
            doc.attr(doc.root().unwrap(), "b").unwrap().value,
            quote_value
        );
    }
}

#[test]
fn set_attr_invalid_name_is_refused() {
    for bad in ["", "1bad", "a b", "a<b"] {
        let err = plan("<a/>", &set_attr(path(vec![nth("a", 0)]), bad, "v")).unwrap_err();
        assert_eq!(
            err,
            EditError::InvalidName(format!("`{bad}` is not a valid name."))
        );
    }
}

#[test]
fn remove_attr_first_drops_its_whitespace() {
    assert_op(
        "<a b=\"1\" c=\"2\"/>",
        &XmlOp::RemoveAttr {
            element: path(vec![nth("a", 0)]),
            name: "b".into(),
        },
        "<a c=\"2\"/>",
    );
}

#[test]
fn remove_attr_last() {
    assert_op(
        "<a b=\"1\" c=\"2\"/>",
        &XmlOp::RemoveAttr {
            element: path(vec![nth("a", 0)]),
            name: "c".into(),
        },
        "<a b=\"1\"/>",
    );
}

#[test]
fn remove_attr_only_one() {
    assert_op(
        "<a b=\"1\"/>",
        &XmlOp::RemoveAttr {
            element: path(vec![nth("a", 0)]),
            name: "b".into(),
        },
        "<a/>",
    );
}

#[test]
fn remove_attr_crlf_tabs() {
    let text = "<types>\r\n\t<type name = 'A' nominal='1' />\r\n</types>";
    let want = "<types>\r\n\t<type nominal='1' />\r\n</types>";
    assert_op(
        text,
        &XmlOp::RemoveAttr {
            element: path(vec![nth("types", 0), nth("type", 0)]),
            name: "name".into(),
        },
        want,
    );
}

#[test]
fn remove_attr_absent_is_not_found() {
    let err = plan(
        "<a b=\"1\"/>",
        &XmlOp::RemoveAttr {
            element: path(vec![nth("a", 0)]),
            name: "c".into(),
        },
    )
    .unwrap_err();
    match err {
        EditError::NotFound(message) => assert!(message.contains('c'), "{message}"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn set_text_replaces_the_trimmed_text() {
    let text = "<a>\n    <nominal>10</nominal>\n</a>\n";
    let want = "<a>\n    <nominal>12</nominal>\n</a>\n";
    assert_op(
        text,
        &set_text(path(vec![nth("a", 0), nth("nominal", 0)]), "12"),
        want,
    );
}

#[test]
fn set_text_on_empty_element() {
    assert_op(
        "<a></a>",
        &set_text(path(vec![nth("a", 0)]), "x"),
        "<a>x</a>",
    );
}

#[test]
fn set_text_on_whitespace_only_element() {
    assert_op(
        "<a>\n  </a>",
        &set_text(path(vec![nth("a", 0)]), "x"),
        "<a>\n  x</a>",
    );
}

#[test]
fn set_text_on_self_closing_keeps_attributes() {
    assert_op(
        "<a b=\"1\"/>",
        &set_text(path(vec![nth("a", 0)]), "hi"),
        "<a b=\"1\">hi</a>",
    );
    assert_op(
        "<a b=\"1\" />",
        &set_text(path(vec![nth("a", 0)]), "hi"),
        "<a b=\"1\">hi</a>",
    );
}

#[test]
fn set_text_escapes_and_round_trips() {
    let out = edit_run("<a>old</a>", &set_text(path(vec![nth("a", 0)]), "a&b<c"));
    assert_eq!(out, "<a>a&amp;b&lt;c</a>");
    let doc = parse(&out);
    assert_eq!(doc.text_of(doc.root().unwrap()), "a&b<c");
}

#[test]
fn set_text_empty_removes_the_text() {
    assert_op(
        "<a>old</a>",
        &set_text(path(vec![nth("a", 0)]), ""),
        "<a></a>",
    );
    assert_op("<a/>", &set_text(path(vec![nth("a", 0)]), ""), "<a></a>");
}

#[test]
fn set_text_on_a_parent_is_has_children() {
    let err = plan(
        "<nominal><x/></nominal>",
        &set_text(path(vec![nth("nominal", 0)]), "1"),
    )
    .unwrap_err();
    assert_eq!(
        err,
        EditError::HasChildren("`<nominal>` has child elements, so its text can't be set.".into())
    );
}

#[test]
fn set_text_crlf_tabs_and_comments() {
    let text = "<type name=\"A\"><!--c-->\r\n\t<nominal>1</nominal>\r\n</type>";
    let want = "<type name=\"A\"><!--c-->\r\n\t<nominal>2</nominal>\r\n</type>";
    assert_op(
        text,
        &set_text(path(vec![nth("type", 0), nth("nominal", 0)]), "2"),
        want,
    );
}

#[test]
fn set_text_replaces_the_value_beside_a_comment() {
    for (name, text, want) in [
        (
            "nominal",
            "<nominal>10<!-- was 5 --></nominal>",
            "<nominal>20<!-- was 5 --></nominal>",
        ),
        (
            "lifetime",
            "<lifetime>\n 3600 <!-- 1h -->\n</lifetime>",
            "<lifetime>\n 20 <!-- 1h -->\n</lifetime>",
        ),
    ] {
        assert_op(text, &set_text(path(vec![nth(name, 0)]), "20"), want);
    }
}

#[test]
fn set_text_refuses_a_value_split_by_a_comment() {
    let err = plan(
        "<n>1<!-- c -->2</n>",
        &set_text(path(vec![nth("n", 0)]), "3"),
    )
    .unwrap_err();
    assert_eq!(
        err,
        EditError::Unsupported(
            "`<n>` mixes its value with other content; edit it in Source.".into()
        )
    );
}

#[test]
fn set_text_bom() {
    let text = "\u{FEFF}<a>old</a>";
    let want = "\u{FEFF}<a>new</a>";
    assert_op(text, &set_text(path(vec![nth("a", 0)]), "new"), want);
}

#[test]
fn rename_self_closing() {
    assert_op(
        "<type name=\"A\"/>",
        &rename(path(vec![nth("type", 0)]), "thing"),
        "<thing name=\"A\"/>",
    );
}

#[test]
fn rename_with_end_tag() {
    assert_op(
        "<type name=\"A\"></type>",
        &rename(path(vec![nth("type", 0)]), "thing"),
        "<thing name=\"A\"></thing>",
    );
}

#[test]
fn rename_with_children() {
    assert_op(
        "<type><a/></type>",
        &rename(path(vec![nth("type", 0)]), "thing"),
        "<thing><a/></thing>",
    );
}

#[test]
fn rename_crlf_tabs_bom() {
    let text = "\u{FEFF}<types>\r\n\t<type name=\"A\">\r\n\t</type>\r\n</types>";
    let want = "\u{FEFF}<types>\r\n\t<entry name=\"A\">\r\n\t</entry>\r\n</types>";
    assert_op(
        text,
        &rename(path(vec![nth("types", 0), nth("type", 0)]), "entry"),
        want,
    );
}

#[test]
fn rename_invalid_name_is_refused() {
    let err = plan("<type/>", &rename(path(vec![nth("type", 0)]), "1bad")).unwrap_err();
    assert_eq!(
        err,
        EditError::InvalidName("`1bad` is not a valid name.".into())
    );
}

#[test]
fn golden_across_styles_and_comments() {
    let cases: Vec<(&str, &str, XmlOp, &str)> = vec![
        (
            "lf four spaces slash",
            "<a>\n    <b name=\"x\"/>\n</a>",
            set_attr(path(vec![nth("a", 0), nth("b", 0)]), "name", "y"),
            "<a>\n    <b name=\"y\"/>\n</a>",
        ),
        (
            "crlf tabs slash space",
            "<a>\r\n\t<b name=\"x\" />\r\n</a>",
            set_attr(path(vec![nth("a", 0), nth("b", 0)]), "name", "y"),
            "<a>\r\n\t<b name=\"y\" />\r\n</a>",
        ),
        (
            "single quoted",
            "<a><b name='x'/></a>",
            set_attr(path(vec![nth("a", 0), nth("b", 0)]), "name", "y"),
            "<a><b name='y'/></a>",
        ),
        (
            "bom",
            "\u{FEFF}<a><b name=\"x\"/></a>",
            set_attr(path(vec![nth("a", 0), nth("b", 0)]), "name", "y"),
            "\u{FEFF}<a><b name=\"y\"/></a>",
        ),
        (
            "remove attr with comments",
            "<!--p--><a><!--c--><b x=\"1\" y=\"2\"/></a><!--q-->",
            XmlOp::RemoveAttr {
                element: path(vec![nth("a", 0), nth("b", 0)]),
                name: "y".into(),
            },
            "<!--p--><a><!--c--><b x=\"1\"/></a><!--q-->",
        ),
        (
            "rename with comments",
            "<!--p--><a><!--c--><b/></a><!--q-->",
            XmlOp::RenameElement {
                element: path(vec![nth("a", 0), nth("b", 0)]),
                name: "c".into(),
            },
            "<!--p--><a><!--c--><c/></a><!--q-->",
        ),
        (
            "set text with comments",
            "<!--p--><a><!--c--><nominal>1</nominal></a><!--q-->",
            set_text(path(vec![nth("a", 0), nth("nominal", 0)]), "2"),
            "<!--p--><a><!--c--><nominal>2</nominal></a><!--q-->",
        ),
        (
            "top level fragment",
            "<type name=\"A\"/><type name=\"B\"/>",
            set_attr(path(vec![nth("type", 1)]), "name", "C"),
            "<type name=\"A\"/><type name=\"C\"/>",
        ),
    ];
    for (label, before, op, after) in cases {
        assert_eq!(edit_run(before, &op), after, "{label}");
    }
}

#[test]
fn error_display_texts() {
    assert_eq!(
        EditError::NotFound("No `<type>` named `FieldCanteen` here.".into()).to_string(),
        "No `<type>` named `FieldCanteen` here."
    );
    assert_eq!(
        EditError::Ambiguous("Two `<type>` elements match `FieldCanteen`.".into()).to_string(),
        "Two `<type>` elements match `FieldCanteen`."
    );
    assert_eq!(
        EditError::HasChildren("`<nominal>` has child elements, so its text can't be set.".into())
            .to_string(),
        "`<nominal>` has child elements, so its text can't be set."
    );
    assert_eq!(
        EditError::Unparseable.to_string(),
        "Fix the syntax errors first."
    );
    assert_eq!(
        EditError::InvalidName("`1bad` is not a valid name.".into()).to_string(),
        "`1bad` is not a valid name."
    );
    assert_eq!(
        EditError::IndexOutOfRange { len: 1, index: 5 }.to_string(),
        "Only 1 match, so index 5 is out of range."
    );
    assert_eq!(EditError::Exists("x".into()).to_string(), "x");
    assert_eq!(EditError::InvalidValue("y".into()).to_string(), "y");
    assert_eq!(EditError::Unsupported("z".into()).to_string(), "z");
}

#[test]
fn attr_pick_that_matches_nothing_is_not_found() {
    let text = "<types><type name=\"A\"/></types>";
    let err = plan(
        text,
        &set_attr(
            path(vec![
                nth("types", 0),
                by_attr("type", "name", "FieldCanteen"),
            ]),
            "a",
            "1",
        ),
    )
    .unwrap_err();
    assert_eq!(
        err,
        EditError::NotFound("No `<type>` named `FieldCanteen` here.".into())
    );
}

#[test]
fn attr_pick_with_two_matches_is_ambiguous() {
    let text = "<types><type name=\"FieldCanteen\"/><type name=\"FieldCanteen\"/></types>";
    let err = plan(
        text,
        &set_attr(
            path(vec![
                nth("types", 0),
                by_attr("type", "name", "FieldCanteen"),
            ]),
            "a",
            "1",
        ),
    )
    .unwrap_err();
    assert_eq!(
        err,
        EditError::Ambiguous("Two `<type>` elements match `FieldCanteen`.".into())
    );
}

#[test]
fn unparseable_text_is_refused() {
    assert_eq!(
        plan("<a>", &set_text(path(vec![nth("a", 0)]), "x")),
        Err(EditError::Unparseable)
    );
}

#[test]
fn nth_out_of_range_is_index_out_of_range() {
    let err = resolve(&parse("<a/>"), &path(vec![nth("a", 5)])).unwrap_err();
    assert_eq!(err, EditError::IndexOutOfRange { len: 1, index: 5 });
}

#[test]
fn nth_with_no_match_is_not_found() {
    let err = resolve(&parse("<a/>"), &path(vec![nth("b", 0)])).unwrap_err();
    assert_eq!(err, EditError::NotFound("No `<b>` here.".into()));
}

const FIXTURE: &str = "<?xml version=\"1.0\"?>\n<!-- top -->\n<types>\n    <type name=\"A\">\n        <nominal>1</nominal>\n        <flags count_in_map=\"1\"/>\n    </type>\n    <type name=\"B\"/>\n</types>\n<!-- tail -->\n";

#[test]
fn path_to_round_trips_every_element() {
    let doc = parse(FIXTURE);
    let mut elements = 0;
    for (id, node) in doc.nodes.iter().enumerate() {
        if matches!(node.kind, NodeKind::Element(_)) {
            elements += 1;
            assert_eq!(resolve(&doc, &path_to(&doc, id)), Ok(id));
        }
    }
    assert_eq!(elements, 5);
}

#[test]
fn attr_pick_is_case_insensitive() {
    let text = "<types><type name=\"FieldCanteen\"/></types>";
    let doc = parse(text);
    let id = resolve(
        &doc,
        &path(vec![
            nth("types", 0),
            by_attr("type", "name", "fieldcanteen"),
        ]),
    )
    .unwrap();
    assert_eq!(doc.element(id).unwrap().name, "type");
    assert_eq!(doc.attr(id, "name").unwrap().value, "FieldCanteen");
}

#[test]
fn path_to_uses_nth_among_same_name_siblings() {
    let doc = parse("<types><type name=\"A\"/><type name=\"B\"/></types>");
    let second = doc.children_named(doc.root().unwrap(), "type")[1];
    assert_eq!(
        path_to(&doc, second),
        path(vec![nth("types", 0), nth("type", 1)])
    );
}

#[test]
fn rename_plans_the_name_and_the_close_in_order() {
    let edits = plan(
        "<type></type>",
        &rename(path(vec![nth("type", 0)]), "thing"),
    )
    .unwrap();
    assert_eq!(edits.len(), 2);
    assert!(edits[0].offset < edits[1].offset);
    assert_eq!(edits[0].text, "thing");
    assert_eq!(edits[1].text, "thing");
}

#[test]
fn apply_runs_edits_back_to_front() {
    let edits = vec![
        TextEdit {
            offset: 0,
            len: 1,
            text: "X".into(),
        },
        TextEdit {
            offset: 2,
            len: 1,
            text: "Y".into(),
        },
    ];
    assert_eq!(apply("abc", &edits), "XbY");
}

#[test]
fn plan_all_reparses_between_ops() {
    let text = "<types><type name=\"A\"><nominal>1</nominal></type></types>";
    let ops = vec![
        set_attr(path(vec![nth("types", 0), nth("type", 0)]), "name", "B"),
        set_text(
            path(vec![nth("types", 0), nth("type", 0), nth("nominal", 0)]),
            "2",
        ),
    ];
    assert_eq!(
        plan_all(text, &ops).unwrap(),
        "<types><type name=\"B\"><nominal>2</nominal></type></types>"
    );
}

#[test]
fn xml_op_json_is_externally_tagged_snake_case() {
    let op = set_attr(path(vec![nth("types", 0)]), "a", "1");
    let value = serde_json::to_value(&op).unwrap();
    assert_eq!(
        value,
        json!({"set_attr":{"element":[{"name":"types","pick":{"nth":0}}],"name":"a","value":"1"}})
    );
    assert_eq!(serde_json::from_value::<XmlOp>(value).unwrap(), op);
}

#[test]
fn attr_pick_json_shape_round_trips() {
    let op = XmlOp::RemoveAttr {
        element: path(vec![by_attr("type", "name", "FieldCanteen")]),
        name: "nominal".into(),
    };
    let value = serde_json::to_value(&op).unwrap();
    assert_eq!(
        value,
        json!({"remove_attr":{"element":[{"name":"type","pick":{"attr":{"name":"name","value":"FieldCanteen"}}}],"name":"nominal"}})
    );
    assert_eq!(serde_json::from_value::<XmlOp>(value).unwrap(), op);
    assert_eq!(serde_json::to_value(XmlPath::default()).unwrap(), json!([]));
}
