//! The XML edit engine, part 2 — insert, remove, wrap and style
//! inference. Each op is planned into byte splices over the original text.

use crate::common::fixtures::assert_op;
use modfoundry_ce::edit::{
    infer_style, plan, plan_all, render, EditError, NewElement, Pick, Position, Step, Style, XmlOp,
    XmlPath,
};
use modfoundry_ce::xml::parse;

fn nth(name: &str, index: usize) -> Step {
    Step {
        name: name.into(),
        pick: Pick::Nth(index),
    }
}

fn path(steps: Vec<Step>) -> XmlPath {
    XmlPath(steps)
}

fn el(name: &str, attrs: &[(&str, &str)]) -> NewElement {
    NewElement {
        name: name.into(),
        attrs: attrs
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect(),
        text: None,
        children: Vec::new(),
    }
}

fn text_el(name: &str, text: &str) -> NewElement {
    NewElement {
        name: name.into(),
        attrs: Vec::new(),
        text: Some(text.into()),
        children: Vec::new(),
    }
}

fn style(unit: &str, eol: &'static str, self_close: &'static str) -> Style {
    Style {
        unit: unit.into(),
        eol,
        self_close,
    }
}

fn insert(parent: XmlPath, at: Position, element: NewElement) -> XmlOp {
    XmlOp::InsertElement {
        parent,
        at,
        element,
    }
}

fn remove(element: XmlPath) -> XmlOp {
    XmlOp::RemoveElement { element }
}

fn wrap(name: &str) -> XmlOp {
    XmlOp::WrapTop { name: name.into() }
}

const SPACED_LF: &str = "<types>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n</types>\n";
const SPACED_CRLF: &str = "<types>\r\n\t<type name=\"A\"/>\r\n\t<type name=\"B\"/>\r\n</types>\r\n";

#[test]
fn infer_style_reads_the_files_own_style() {
    {
        let text = "<types>\n    <type name=\"A\">\n        <nominal>1</nominal>\n        <flags count_in_map=\"1\"/>\n    </type>\n</types>\n";
        assert_eq!(infer_style(text, &parse(text)), style("    ", "\n", "/>"));
    }
    {
        let text = "<types>\r\n\t<type name=\"A\">\r\n\t\t<flags count_in_map=\"1\" />\r\n\t</type>\r\n</types>\r\n";
        assert_eq!(infer_style(text, &parse(text)), style("\t", "\r\n", " />"));
    }
    {
        let text = "<a>\n  <b>\n    <c/>\n  </b>\n</a>\n";
        assert_eq!(infer_style(text, &parse(text)), style("  ", "\n", "/>"));
    }
    {
        assert_eq!(infer_style("", &parse("")), style("    ", "\n", "/>"));
    }
}

#[test]
fn infer_style_one_line_file() {
    assert_eq!(
        infer_style("<types><a/></types>", &parse("<types><a/></types>")),
        style("    ", "\n", "/>")
    );
    assert_eq!(
        infer_style("<types><a /></types>", &parse("<types><a /></types>")),
        style("    ", "\n", " />")
    );
}

#[test]
fn render_attributes_only() {
    assert_eq!(
        render(
            &el("type", &[("name", "A")]),
            "    ",
            &style("    ", "\n", "/>")
        )
        .unwrap(),
        "    <type name=\"A\"/>"
    );
    assert_eq!(
        render(
            &el("type", &[("name", "A")]),
            "    ",
            &style("    ", "\n", " />")
        )
        .unwrap(),
        "    <type name=\"A\" />"
    );
}

#[test]
fn render_text_only() {
    assert_eq!(
        render(
            &text_el("nominal", "12"),
            "        ",
            &style("    ", "\n", "/>")
        )
        .unwrap(),
        "        <nominal>12</nominal>"
    );
}

#[test]
fn render_two_levels_of_children() {
    let tree = NewElement {
        name: "type".into(),
        attrs: vec![("name".into(), "A".into())],
        text: None,
        children: vec![
            text_el("nominal", "12"),
            el("flags", &[("count_in_map", "1")]),
        ],
    };
    let want = "    <type name=\"A\">\n        <nominal>12</nominal>\n        <flags count_in_map=\"1\"/>\n    </type>";
    assert_eq!(
        render(&tree, "    ", &style("    ", "\n", "/>")).unwrap(),
        want
    );
}

#[test]
fn render_escapes_values_and_text() {
    let tree = NewElement {
        name: "t".into(),
        attrs: vec![("v".into(), "a\"&<b".into())],
        text: Some("x&<y>".into()),
        children: Vec::new(),
    };
    assert_eq!(
        render(&tree, "", &style("    ", "\n", "/>")).unwrap(),
        "<t v=\"a&quot;&amp;&lt;b\">x&amp;&lt;y&gt;</t>"
    );
}

#[test]
fn render_invalid_name() {
    let err = render(&el("1bad", &[]), "", &style("    ", "\n", "/>")).unwrap_err();
    assert_eq!(
        err,
        EditError::InvalidName("`1bad` is not a valid name.".into())
    );
}

#[test]
fn insert_last_into_spaced_parent_lf() {
    assert_op(
        SPACED_LF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Last,
            el("type", &[("name", "C")]),
        ),
        "<types>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n    <type name=\"C\"/>\n</types>\n",
    );
}

#[test]
fn insert_first_into_spaced_parent_lf() {
    assert_op(
        SPACED_LF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::First,
            el("type", &[("name", "C")]),
        ),
        "<types>\n    <type name=\"C\"/>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n</types>\n",
    );
}

#[test]
fn insert_index_into_spaced_parent_lf() {
    assert_op(
        SPACED_LF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Index(1),
            el("type", &[("name", "C")]),
        ),
        "<types>\n    <type name=\"A\"/>\n    <type name=\"C\"/>\n    <type name=\"B\"/>\n</types>\n",
    );
}

#[test]
fn insert_last_into_spaced_parent_crlf() {
    assert_op(
        SPACED_CRLF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Last,
            el("type", &[("name", "C")]),
        ),
        "<types>\r\n\t<type name=\"A\"/>\r\n\t<type name=\"B\"/>\r\n\t<type name=\"C\"/>\r\n</types>\r\n",
    );
}

#[test]
fn insert_first_into_spaced_parent_crlf() {
    assert_op(
        SPACED_CRLF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::First,
            el("type", &[("name", "C")]),
        ),
        "<types>\r\n\t<type name=\"C\"/>\r\n\t<type name=\"A\"/>\r\n\t<type name=\"B\"/>\r\n</types>\r\n",
    );
}

#[test]
fn insert_index_into_spaced_parent_crlf() {
    assert_op(
        SPACED_CRLF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Index(1),
            el("type", &[("name", "C")]),
        ),
        "<types>\r\n\t<type name=\"A\"/>\r\n\t<type name=\"C\"/>\r\n\t<type name=\"B\"/>\r\n</types>\r\n",
    );
}

#[test]
fn insert_index_goes_above_the_comment_block() {
    let text = "<types>\n    <type name=\"A\"/>\n    <!-- keep -->\n    <!-- with b -->\n    <type name=\"B\"/>\n</types>\n";
    let want = "<types>\n    <type name=\"A\"/>\n    <type name=\"C\"/>\n    <!-- keep -->\n    <!-- with b -->\n    <type name=\"B\"/>\n</types>\n";
    assert_op(
        text,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Index(1),
            el("type", &[("name", "C")]),
        ),
        want,
    );
}

#[test]
fn insert_multiline_element() {
    let tree = NewElement {
        name: "type".into(),
        attrs: vec![("name".into(), "C".into())],
        text: None,
        children: vec![
            text_el("nominal", "12"),
            el("flags", &[("count_in_map", "1")]),
        ],
    };
    let want = "<types>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n    <type name=\"C\">\n        <nominal>12</nominal>\n        <flags count_in_map=\"1\"/>\n    </type>\n</types>\n";
    assert_op(
        SPACED_LF,
        &insert(path(vec![nth("types", 0)]), Position::Last, tree),
        want,
    );
}

#[test]
fn insert_into_tags_on_different_lines() {
    let text = "<limits>\n</limits>\n";
    for at in [Position::First, Position::Last, Position::Index(0)] {
        assert_op(
            text,
            &insert(path(vec![nth("limits", 0)]), at, el("entry", &[])),
            "<limits>\n    <entry/>\n</limits>\n",
        );
    }
}

#[test]
fn insert_into_tags_on_different_lines_crlf() {
    let text = "<limits>\r\n</limits>\r\n";
    assert_op(
        text,
        &insert(
            path(vec![nth("limits", 0)]),
            Position::Last,
            el("entry", &[]),
        ),
        "<limits>\r\n    <entry/>\r\n</limits>\r\n",
    );
}

#[test]
fn insert_into_empty_parent_on_one_line() {
    assert_op(
        "<types></types>\n",
        &insert(
            path(vec![nth("types", 0)]),
            Position::Last,
            el("type", &[("name", "C")]),
        ),
        "<types>\n    <type name=\"C\"/>\n</types>\n",
    );
    assert_op(
        "<types></types>\r\n",
        &insert(
            path(vec![nth("types", 0)]),
            Position::Last,
            el("type", &[("name", "C")]),
        ),
        "<types>\r\n    <type name=\"C\"/>\r\n</types>\r\n",
    );
}

#[test]
fn insert_into_self_closing_parent() {
    let text = "<types>\n    <type name=\"X\"/>\n</types>\n";
    assert_op(
        text,
        &insert(
            path(vec![nth("types", 0), nth("type", 0)]),
            Position::Last,
            el("flags", &[("x", "1")]),
        ),
        "<types>\n    <type name=\"X\">\n        <flags x=\"1\"/>\n    </type>\n</types>\n",
    );
}

#[test]
fn insert_into_self_closing_parent_with_space() {
    let text = "<types>\n    <type name=\"X\" />\n</types>\n";
    assert_op(
        text,
        &insert(
            path(vec![nth("types", 0), nth("type", 0)]),
            Position::Last,
            el("flags", &[("x", "1")]),
        ),
        "<types>\n    <type name=\"X\">\n        <flags x=\"1\" />\n    </type>\n</types>\n",
    );
}

#[test]
fn insert_into_children_sharing_the_parent_line() {
    let text = "<a><b/><c/></a>";
    assert_op(
        text,
        &insert(path(vec![nth("a", 0)]), Position::First, el("x", &[])),
        "<a><x/><b/><c/></a>",
    );
    assert_op(
        text,
        &insert(path(vec![nth("a", 0)]), Position::Last, el("x", &[])),
        "<a><b/><c/><x/></a>",
    );
    assert_op(
        text,
        &insert(path(vec![nth("a", 0)]), Position::Index(1), el("x", &[])),
        "<a><b/><x/><c/></a>",
    );
}

#[test]
fn insert_at_the_top_level_of_a_fragment() {
    let text = "<type name=\"A\"/>\n<type name=\"B\"/>\n";
    assert_op(
        text,
        &insert(
            XmlPath::default(),
            Position::Last,
            el("type", &[("name", "C")]),
        ),
        "<type name=\"A\"/>\n<type name=\"B\"/>\n<type name=\"C\"/>\n",
    );
    assert_op(
        text,
        &insert(
            XmlPath::default(),
            Position::First,
            el("type", &[("name", "C")]),
        ),
        "<type name=\"C\"/>\n<type name=\"A\"/>\n<type name=\"B\"/>\n",
    );
    assert_op(
        text,
        &insert(
            XmlPath::default(),
            Position::Index(1),
            el("type", &[("name", "C")]),
        ),
        "<type name=\"A\"/>\n<type name=\"C\"/>\n<type name=\"B\"/>\n",
    );
}

#[test]
fn insert_index_equal_to_child_count_is_last() {
    assert_op(
        SPACED_LF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Index(2),
            el("type", &[("name", "C")]),
        ),
        "<types>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n    <type name=\"C\"/>\n</types>\n",
    );
}

#[test]
fn insert_index_past_child_count_is_out_of_range() {
    let err = plan(
        SPACED_LF,
        &insert(
            path(vec![nth("types", 0)]),
            Position::Index(3),
            el("type", &[("name", "C")]),
        ),
    )
    .unwrap_err();
    assert_eq!(err, EditError::IndexOutOfRange { len: 2, index: 3 });
}

#[test]
fn remove_alone_on_its_lines() {
    assert_op(
        "<types>\n    <type name=\"A\"/>\n    <type name=\"B\"/>\n</types>\n",
        &remove(path(vec![nth("types", 0), nth("type", 0)])),
        "<types>\n    <type name=\"B\"/>\n</types>\n",
    );
}

#[test]
fn remove_sharing_a_line() {
    assert_op(
        "<a><b/><c/></a>",
        &remove(path(vec![nth("a", 0), nth("b", 0)])),
        "<a><c/></a>",
    );
}

#[test]
fn remove_keeps_the_comment_above() {
    assert_op(
        "<types>\n    <!-- keep -->\n    <type name=\"A\"/>\n</types>\n",
        &remove(path(vec![nth("types", 0), nth("type", 0)])),
        "<types>\n    <!-- keep -->\n</types>\n",
    );
}

#[test]
fn remove_the_only_child() {
    assert_op(
        "<types>\n    <a/>\n</types>\n",
        &remove(path(vec![nth("types", 0), nth("a", 0)])),
        "<types>\n</types>\n",
    );
}

#[test]
fn remove_crlf_aisle() {
    assert_op(
        "<types>\r\n\t<type name=\"A\"/>\r\n\t<type name=\"B\"/>\r\n</types>\r\n",
        &remove(path(vec![nth("types", 0), nth("type", 0)])),
        "<types>\r\n\t<type name=\"B\"/>\r\n</types>\r\n",
    );
}

#[test]
fn wrap_top_fragment_with_declaration_and_bom() {
    let text = "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<type name=\"A\"/>\n<type name=\"B\"/>\n";
    let want = "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<types>\n<type name=\"A\"/>\n<type name=\"B\"/>\n</types>\n";
    assert_op(text, &wrap("types"), want);
}

#[test]
fn wrapping_a_file_with_a_bom_keeps_the_bom_first() {
    let text = "\u{FEFF}<type name=\"A\"/>\n";
    assert_op(
        text,
        &wrap("types"),
        "\u{FEFF}<types>\n<type name=\"A\"/>\n</types>\n",
    );
    assert_op(
        text,
        &insert(
            XmlPath::default(),
            Position::First,
            el("type", &[("name", "B")]),
        ),
        "\u{FEFF}<type name=\"B\"/>\n<type name=\"A\"/>\n",
    );
}

#[test]
fn wrap_top_without_final_newline() {
    assert_op(
        "<type name=\"A\"/>\n<type name=\"B\"/>",
        &wrap("types"),
        "<types>\n<type name=\"A\"/>\n<type name=\"B\"/>\n</types>",
    );
}

#[test]
fn wrap_top_exists() {
    let err = plan("<types>\n</types>\n", &wrap("types")).unwrap_err();
    assert_eq!(
        err,
        EditError::Exists("The file is already wrapped in `<types>`.".into())
    );
}

#[test]
fn insert_then_remove_restores_spaced_parents() {
    let cases = [
        (Position::Last, 2usize),
        (Position::First, 0),
        (Position::Index(1), 1),
    ];
    for (at, index) in cases {
        for text in [SPACED_LF, SPACED_CRLF] {
            let ins = insert(
                path(vec![nth("types", 0)]),
                at,
                el("type", &[("name", "C")]),
            );
            let rm = remove(path(vec![nth("types", 0), nth("type", index)]));
            assert_eq!(plan_all(text, &[ins, rm]).unwrap(), text, "at {at:?}");
        }
    }
}
