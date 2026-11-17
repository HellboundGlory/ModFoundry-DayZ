use modfoundry_layout::parse::{parse, Pos, Property, Value, Widget};
use modfoundry_pbo::PboArchive;
use std::path::Path;

fn pos(line: u32, column: u32) -> Pos {
    Pos { line, column }
}

#[test]
fn reads_properties_of_each_value_kind() {
    let text = r#"TextWidgetClass "{00000000D19DE17F}" {
 position -5 0.1569
 text "Hi"
 "text valign" center_ref
 "src alpha" 1
 empty ""
 blend rover_sim_colorable
}
"#;
    let expected = vec![Widget {
        class: "TextWidgetClass".into(),
        name: Some("{00000000D19DE17F}".into()),
        pos: pos(1, 1),
        end_line: 8,
        properties: vec![
            Property {
                key: "position".into(),
                pos: pos(2, 2),
                values: vec![Value::Number("-5".into()), Value::Number("0.1569".into())],
            },
            Property {
                key: "text".into(),
                pos: pos(3, 2),
                values: vec![Value::Str("Hi".into())],
            },
            Property {
                key: "text valign".into(),
                pos: pos(4, 2),
                values: vec![Value::Word("center_ref".into())],
            },
            Property {
                key: "src alpha".into(),
                pos: pos(5, 2),
                values: vec![Value::Number("1".into())],
            },
            Property {
                key: "empty".into(),
                pos: pos(6, 2),
                values: vec![Value::Str(String::new())],
            },
            Property {
                key: "blend".into(),
                pos: pos(7, 2),
                values: vec![Value::Word("rover_sim_colorable".into())],
            },
        ],
        children: vec![],
    }];
    assert_eq!(parse(text).unwrap(), expected);
}

#[test]
fn collects_children_from_every_block_in_order() {
    let text = r#"FrameWidgetClass root {
 "size" 1 1
 {
  TextWidgetClass label {
   text "x"
  }
  ButtonWidgetClass "button" {
  }
 }
 {
  ScriptParamsClass {
   AlignChilds 1
  }
 }
}
"#;
    let expected = vec![Widget {
        class: "FrameWidgetClass".into(),
        name: Some("root".into()),
        pos: pos(1, 1),
        end_line: 15,
        properties: vec![Property {
            key: "size".into(),
            pos: pos(2, 2),
            values: vec![Value::Number("1".into()), Value::Number("1".into())],
        }],
        children: vec![
            Widget {
                class: "TextWidgetClass".into(),
                name: Some("label".into()),
                pos: pos(4, 3),
                end_line: 6,
                properties: vec![Property {
                    key: "text".into(),
                    pos: pos(5, 4),
                    values: vec![Value::Str("x".into())],
                }],
                children: vec![],
            },
            Widget {
                class: "ButtonWidgetClass".into(),
                name: Some("button".into()),
                pos: pos(7, 3),
                end_line: 8,
                properties: vec![],
                children: vec![],
            },
            Widget {
                class: "ScriptParamsClass".into(),
                name: None,
                pos: pos(11, 3),
                end_line: 13,
                properties: vec![Property {
                    key: "AlignChilds".into(),
                    pos: pos(12, 4),
                    values: vec![Value::Number("1".into())],
                }],
                children: vec![],
            },
        ],
    }];
    assert_eq!(parse(text).unwrap(), expected);
}

#[test]
fn unescapes_quotes_and_backslashes_only() {
    let cases = [
        (r#"Print(\"Hello World\");"#, r#"Print("Hello World");"#),
        (r"a\\b", r"a\b"),
        (r"C:\dir\x", r"C:\dir\x"),
        (r"ends\\", r"ends\"),
    ];
    for (source, expected) in cases {
        let text = format!("TextWidgetClass {{\n text \"{source}\"\n}}\n");
        let widgets = parse(&text).unwrap();
        assert_eq!(
            widgets[0].properties[0].values,
            vec![Value::Str(expected.into())],
            "source {source}"
        );
    }
}

#[test]
fn keeps_utf8_and_tabs_inside_strings() {
    let text = "TextWidgetClass {\n text \"附近\"\n label \"a\tb\"\n}\n";
    let widgets = parse(text).unwrap();
    assert_eq!(
        widgets[0].properties[0].values,
        vec![Value::Str("附近".into())]
    );
    assert_eq!(
        widgets[0].properties[1].values,
        vec![Value::Str("a\tb".into())]
    );
}

#[test]
fn crlf_and_tab_indent_give_the_same_tree() {
    let lf_spaces = "FrameWidgetClass root {\n size 1 1\n {\n  TextWidgetClass label {\n   text \"x\"\n  }\n }\n}\n";
    let crlf_tabs = "FrameWidgetClass root {\r\n\tsize 1 1\r\n\t{\r\n\t\tTextWidgetClass label {\r\n\t\t\ttext \"x\"\r\n\t\t}\r\n\t}\r\n}\r\n";
    assert_eq!(parse(crlf_tabs).unwrap(), parse(lf_spaces).unwrap());
}

#[test]
fn skips_a_leading_byte_order_mark() {
    let widgets = parse("\u{feff}TextWidgetClass label {\n}\n").unwrap();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].class, "TextWidgetClass");
    assert_eq!(widgets[0].pos, pos(1, 1));
}

#[test]
fn blank_text_has_no_widgets() {
    assert_eq!(parse("").unwrap(), vec![]);
    assert_eq!(parse(" \n\t\r\n\n").unwrap(), vec![]);
}

#[test]
fn reports_each_syntax_error_with_its_position() {
    let cases = [
        ("Foo a {\n size 1\n", "1:1: widget is not closed"),
        ("Foo {\n {\n  Bar {\n  }\n", "2:2: block is not closed"),
        ("Foo {\n}\n}\n", "3:1: stray }"),
        ("Foo {\n text \"abc\n}\n", "2:7: unterminated string"),
        ("Foo {\n \u{9644} \"x\n}\n", "2:4: unterminated string"),
        (
            "Foo {\n a } b\n}\n",
            "2:4: expected a property, a widget or a brace",
        ),
        ("Foo {\n}\nbar 1\n", "3:1: property outside any widget"),
        ("TextWidgetClass label\n", "1:1: widget header has no {"),
        ("Foo {\n bar\n}\n", "2:2: property has no value"),
        ("{\n", "1:1: block outside any widget"),
        (
            "Foo {\n TextWidgetClass label\n}\n",
            "2:2: widget header has no {",
        ),
        ("Foo {\n {\n Bar {\n }\n}\n", "1:1: widget is not closed"),
        ("Foo {\n Bar {\n}\n}\n", "2:2: widget outside a child block"),
        (
            "Foo {\n {\n x 1\n }\n}\n",
            "3:2: property outside any widget",
        ),
        (
            "Foo { } }\n",
            "1:5: expected a property, a widget or a brace",
        ),
        ("1 bar\n", "1:1: property outside any widget"),
    ];
    for (text, expected) in cases {
        let error = parse(text).expect_err(text);
        assert_eq!(error.to_string(), expected, "source {text:?}");
    }
}

fn widget_count(widgets: &[Widget]) -> usize {
    widgets.iter().map(|w| 1 + widget_count(&w.children)).sum()
}

fn without_positions(widgets: &[Widget]) -> Vec<Widget> {
    widgets
        .iter()
        .map(|widget| Widget {
            class: widget.class.clone(),
            name: widget.name.clone(),
            pos: pos(0, 0),
            end_line: 0,
            properties: widget
                .properties
                .iter()
                .map(|property| Property {
                    key: property.key.clone(),
                    pos: pos(0, 0),
                    values: property.values.clone(),
                })
                .collect(),
            children: without_positions(&widget.children),
        })
        .collect()
}

fn nested_layout(levels: usize) -> String {
    let mut text = String::new();
    for n in 1..=levels {
        text.push_str(&format!("W{n}Class w{n} {{\n{{\n"));
    }
    for _ in 0..levels {
        text.push_str("}\n}\n");
    }
    text
}

#[test]
fn a_lone_carriage_return_ends_a_line() {
    let lf = "Foo {\n text \"a\"\n}\n";
    let cr = "Foo {\r text \"a\"\r}\r";
    let widgets = parse(cr).unwrap();
    assert_eq!(widgets, parse(lf).unwrap());
    assert_eq!(widgets[0].properties[0].pos, pos(2, 2));
    assert_eq!(widgets[0].end_line, 3);

    let crlf = "Foo {\r\n text \"a\"\r\n}\r\n";
    assert_eq!(parse(crlf).unwrap(), parse(lf).unwrap());

    let error = parse("Foo {\r bar\r}\r").expect_err("property has no value");
    assert_eq!(error.to_string(), "2:2: property has no value");

    let mixed = parse("Foo {\n\r text \"a\"\n}\n").unwrap();
    assert_eq!(mixed[0].properties[0].pos, pos(3, 2));
}

#[test]
fn a_close_brace_ends_a_widget_on_its_own_header_line() {
    for (text, name) in [("Foo { }\n", None), ("Foo name { }\n", Some("name"))] {
        let widgets = parse(text).unwrap();
        assert_eq!(widgets.len(), 1, "{text:?}");
        assert_eq!(widgets[0].class, "Foo", "{text:?}");
        assert_eq!(widgets[0].name.as_deref(), name, "{text:?}");
        assert_eq!(widgets[0].pos, pos(1, 1), "{text:?}");
        assert_eq!(widgets[0].end_line, 1, "{text:?}");
        assert!(widgets[0].properties.is_empty(), "{text:?}");
        assert!(widgets[0].children.is_empty(), "{text:?}");
    }

    let widgets = parse("Foo {\n { }\n}\n").unwrap();
    assert_eq!(widgets[0].end_line, 3);
    assert!(widgets[0].children.is_empty());
}

#[test]
fn a_close_brace_after_a_value_closes_the_widget() {
    let widgets = parse("Foo {\n a 1 }\n").unwrap();
    assert_eq!(widgets[0].properties[0].key, "a");
    assert_eq!(
        widgets[0].properties[0].values,
        vec![Value::Number("1".into())]
    );
    assert_eq!(widgets[0].end_line, 2);

    let error = parse("Foo {\n a }\n").expect_err("property has no value");
    assert_eq!(error.to_string(), "2:2: property has no value");

    for (text, expected) in [
        ("Foo {\n a } b\n}\n", "2:4"),
        ("Foo {\n a } }\n}\n", "2:4"),
        ("Foo {\n } }\n", "2:2"),
    ] {
        let error = parse(text).expect_err(text);
        assert_eq!(
            error.to_string(),
            format!("{expected}: expected a property, a widget or a brace"),
            "{text:?}"
        );
    }
}

#[test]
fn properties_on_the_header_line_are_refused_with_a_hint() {
    for (text, expected) in [
        ("Foo { visible 1 }\n", "1:7"),
        ("TextWidgetClass t { visible 1 size 1 1 }\n", "1:21"),
    ] {
        let error = parse(text).expect_err(text);
        assert_eq!(
            error.to_string(),
            format!("{expected}: put each property on its own line after the {{"),
            "{text:?}"
        );
    }
}

#[test]
fn a_header_may_open_on_the_next_line() {
    let inline = without_positions(&parse("Foo bar {\n x 1\n}\n").unwrap());
    for (text, end_line) in [
        ("Foo bar\n{\n x 1\n}\n", 4),
        ("Foo bar\n\n\n{\n x 1\n}\n", 6),
        ("Foo \"bar\"\n{\n x 1\n}\n", 4),
    ] {
        let widgets = parse(text).unwrap();
        assert_eq!(without_positions(&widgets), inline, "{text:?}");
        assert_eq!(widgets[0].end_line, end_line, "{text:?}");
    }

    let widgets = parse("ScriptParamsClass\n{\n}\n").unwrap();
    assert_eq!(widgets[0].class, "ScriptParamsClass");
    assert_eq!(widgets[0].end_line, 3);

    let text = "Foo {\n {\n  Bar baz\n  {\n   y 2\n  }\n }\n}\n";
    let widgets = parse(text).unwrap();
    assert_eq!(widgets[0].children[0].class, "Bar");
    assert_eq!(widgets[0].children[0].name.as_deref(), Some("baz"));
    assert_eq!(widgets[0].children[0].end_line, 6);
    assert_eq!(widgets[0].children[0].properties[0].key, "y");
}

#[test]
fn a_header_with_no_brace_is_caught_for_any_class_name() {
    for (text, expected) in [
        ("Foo bar\n", "1:1"),
        ("Foo\n", "1:1"),
        ("Foo bar\nsize 1\n}\n", "1:1"),
        ("Foo bar", "1:1"),
        ("Foo {\n {\n  Bar baz\n }\n}\n", "3:3"),
    ] {
        let error = parse(text).expect_err(text);
        assert_eq!(
            error.to_string(),
            format!("{expected}: widget header has no {{"),
            "{text:?}"
        );
    }

    let error = parse("Foo {\n BarWidgetClass x\n}\n").expect_err("header in a body");
    assert_eq!(error.to_string(), "2:2: widget header has no {");

    let error = parse("Foo {\n BarWidgetClass x\n {\n }\n}\n").expect_err("header in a body");
    assert_eq!(error.to_string(), "2:2: widget outside a child block");

    let error = parse("Foo bar baz\n").expect_err("three words are not a header");
    assert_eq!(error.to_string(), "1:1: property outside any widget");
}

#[test]
fn a_two_word_line_in_a_widget_body_stays_a_property() {
    let widgets = parse("Foo {\n style blank\n fixaspect fixwidth\n}\n").unwrap();
    let keys: Vec<&str> = widgets[0]
        .properties
        .iter()
        .map(|property| property.key.as_str())
        .collect();
    assert_eq!(keys, ["style", "fixaspect"]);

    let widgets = parse("Foo {\n x 1\n {\n  Bar {\n  }\n }\n}\n").unwrap();
    assert_eq!(widgets[0].properties[0].key, "x");
    assert_eq!(widgets[0].children[0].class, "Bar");
    assert_eq!(widgets[0].children[0].name, None);
}

#[test]
fn nests_to_256_levels_and_stops_after() {
    let widgets = parse(&nested_layout(256)).unwrap();
    let mut deepest = &widgets[0];
    while let Some(child) = deepest.children.first() {
        deepest = child;
    }
    assert_eq!(deepest.class, "W256Class");

    for levels in [257, 300] {
        let error = parse(&nested_layout(levels)).expect_err("deeper than 256");
        assert_eq!(
            error.to_string(),
            "513:1: widgets nest more than 256 levels deep",
            "{levels} levels"
        );
    }
}

#[test]
#[ignore]
fn parses_every_vanilla_layout_in_gui_pbo() {
    let Some(install) = std::env::var_os("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let archive = PboArchive::open(&Path::new(&install).join("dta/gui.pbo")).unwrap();
    let mut files = 0;
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().into_owned();
        if !name.ends_with(".layout") {
            continue;
        }
        let text = String::from_utf8(archive.read_entry(index).unwrap()).unwrap();
        let widgets = parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(widgets.len(), 1, "{name}");
        println!("{name} widgets {}", widget_count(&widgets));
        files += 1;
    }
    assert!(files > 0, "no .layout entries in gui.pbo");
    println!("{files} layout files parsed");
}

#[test]
fn keeps_signed_words_without_digits_as_words() {
    let text = "TextWidgetClass {\n size -inf\n}\n";
    let widgets = parse(text).unwrap();
    assert_eq!(
        widgets[0].properties[0].values,
        vec![Value::Word("-inf".into())]
    );
}

#[test]
fn only_spaces_and_tabs_separate_words() {
    let widgets = parse("Foo {\n k\u{a0}v 1\n}\n").unwrap();
    let property = &widgets[0].properties[0];
    assert_eq!(property.key, "k\u{a0}v");
    assert_eq!(property.values, vec![Value::Number("1".into())]);
}
