use modfoundry_gamediff::layout::{
    layout_diff, layout_model, LayoutFileChange, WidgetChange, WidgetModel, WidgetRef,
};
use modfoundry_gamediff::ValueChange;
use std::collections::BTreeMap;

fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

const NAMED_PATH: &str = r#"FrameWidgetClass Root {
 position 0 0
 {
  FrameWidgetClass Child {
   position 1 0
  }
  TextWidgetClass {
   text "hello world"
  }
  TextWidgetClass {
   text "second"
  }
 }
}
"#;

#[test]
fn widgets_are_keyed_by_their_name_path() {
    let model = layout_model(&[("gui/layouts/a.layout".to_owned(), NAMED_PATH.to_owned())]);
    let file = &model.files["gui/layouts/a.layout"];
    assert_eq!(file.error, None);

    let keys: Vec<&str> = file.widgets.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "Root",
            "Root/Child",
            "Root/TextWidgetClass",
            "Root/TextWidgetClass#2"
        ]
    );
    assert_eq!(
        file.widgets["Root"],
        WidgetModel {
            path: "Root".to_owned(),
            class: "FrameWidgetClass".to_owned(),
            line: 1,
            properties: props(&[("position", "0 0")]),
        }
    );
    assert_eq!(file.widgets["Root/Child"].line, 4);
    assert_eq!(
        file.widgets["Root/TextWidgetClass"].properties,
        props(&[("text", "\"hello world\"")])
    );
    assert_eq!(
        file.widgets["Root/TextWidgetClass#2"].properties,
        props(&[("text", "\"second\"")])
    );
}

const BASE: &str = r#"FrameWidgetClass Root {
 position 0 0
 {
  ButtonWidgetClass Go {
   position 1 1
   text "Go"
  }
  TextWidgetClass Label {
   position 2 2
  }
 }
}
"#;

fn diff_of(old: &str, new: &str) -> Option<LayoutFileChange> {
    let old = layout_model(&[("gui/a.layout".to_owned(), old.to_owned())]);
    let new = layout_model(&[("gui/a.layout".to_owned(), new.to_owned())]);
    let mut diff = layout_diff(&old, &new);
    assert!(diff.added.is_empty() && diff.removed.is_empty());
    diff.changed.pop()
}

fn changed(
    path: &str,
    line: u32,
    class: Option<(&str, &str)>,
    properties: &[(&str, Option<&str>, Option<&str>)],
) -> WidgetChange {
    WidgetChange {
        path: path.to_owned(),
        line,
        class: class.map(|(old, new)| ValueChange {
            name: "class".to_owned(),
            old: Some(old.to_owned()),
            new: Some(new.to_owned()),
        }),
        properties: properties
            .iter()
            .map(|(name, old, new)| ValueChange {
                name: (*name).to_owned(),
                old: old.map(str::to_owned),
                new: new.map(str::to_owned),
            })
            .collect(),
    }
}

fn name_only(path: &str, class: &str, line: u32) -> WidgetRef {
    WidgetRef {
        path: path.to_owned(),
        class: class.to_owned(),
        line,
    }
}

#[test]
fn layout_diff_lists_each_kind_of_change() {
    let widget_added = BASE.replace(
        "  TextWidgetClass Label {\n   position 2 2\n  }\n",
        "  TextWidgetClass Label {\n   position 2 2\n  }\n  TextWidgetClass Extra {\n   position 3 3\n  }\n",
    );
    let widget_removed = BASE.replace("  TextWidgetClass Label {\n   position 2 2\n  }\n", "");
    let value_changed = BASE.replace("position 1 1", "position 5 1");
    let property_added = BASE.replace("   position 2 2\n", "   position 2 2\n   text \"hi\"\n");
    let class_changed = BASE.replace("ButtonWidgetClass Go", "TextWidgetClass Go");
    let spacing_changed = BASE.replace(" position 0 0", "   position 0 0");
    let moved_down = format!("\n\n{BASE}");

    let cases: [(&str, &str, Option<LayoutFileChange>); 7] = [
        (
            "widget added",
            &widget_added,
            Some(LayoutFileChange {
                path: "gui/a.layout".to_owned(),
                unparsed: false,
                widgets_added: vec![name_only("Root/Extra", "TextWidgetClass", 11)],
                widgets_removed: Vec::new(),
                widgets_changed: Vec::new(),
            }),
        ),
        (
            "widget removed",
            &widget_removed,
            Some(LayoutFileChange {
                path: "gui/a.layout".to_owned(),
                unparsed: false,
                widgets_added: Vec::new(),
                widgets_removed: vec![name_only("Root/Label", "TextWidgetClass", 8)],
                widgets_changed: Vec::new(),
            }),
        ),
        (
            "property value changed",
            &value_changed,
            Some(LayoutFileChange {
                path: "gui/a.layout".to_owned(),
                unparsed: false,
                widgets_added: Vec::new(),
                widgets_removed: Vec::new(),
                widgets_changed: vec![changed(
                    "Root/Go",
                    4,
                    None,
                    &[("position", Some("1 1"), Some("5 1"))],
                )],
            }),
        ),
        (
            "property added",
            &property_added,
            Some(LayoutFileChange {
                path: "gui/a.layout".to_owned(),
                unparsed: false,
                widgets_added: Vec::new(),
                widgets_removed: Vec::new(),
                widgets_changed: vec![changed(
                    "Root/Label",
                    8,
                    None,
                    &[("text", None, Some("\"hi\""))],
                )],
            }),
        ),
        (
            "class changed",
            &class_changed,
            Some(LayoutFileChange {
                path: "gui/a.layout".to_owned(),
                unparsed: false,
                widgets_added: Vec::new(),
                widgets_removed: Vec::new(),
                widgets_changed: vec![changed(
                    "Root/Go",
                    4,
                    Some(("ButtonWidgetClass", "TextWidgetClass")),
                    &[],
                )],
            }),
        ),
        ("only spacing changed", &spacing_changed, None),
        ("widget moved down two lines", &moved_down, None),
    ];

    for (name, new, expected) in cases {
        assert_eq!(diff_of(BASE, new), expected, "{name}");
    }

    let broken = BASE.replace(" position 0 0\n", " position\n");
    assert_eq!(
        diff_of(BASE, &broken),
        Some(LayoutFileChange {
            path: "gui/a.layout".to_owned(),
            unparsed: true,
            widgets_added: Vec::new(),
            widgets_removed: Vec::new(),
            widgets_changed: Vec::new(),
        }),
        "file stops parsing"
    );
}

#[test]
fn files_on_one_side_are_added_or_removed() {
    let old = layout_model(&[
        ("gui/layouts/a.layout".to_owned(), BASE.to_owned()),
        ("gui/layouts/gone.layout".to_owned(), BASE.to_owned()),
    ]);
    let new = layout_model(&[
        ("GUI\\Layouts\\A.layout".to_owned(), BASE.to_owned()),
        ("gui/layouts/new.layout".to_owned(), BASE.to_owned()),
    ]);
    let diff = layout_diff(&old, &new);
    assert_eq!(diff.added, ["gui/layouts/new.layout"]);
    assert_eq!(diff.removed, ["gui/layouts/gone.layout"]);
    assert!(diff.changed.is_empty());

    let model = layout_model(&[("GUI\\Layouts\\A.layout".to_owned(), BASE.to_owned())]);
    let file = &model.files["gui/layouts/a.layout"];
    assert_eq!(file.path, "GUI/Layouts/A.layout");
}

#[test]
fn the_model_round_trips_through_json() {
    let model = layout_model(&[("gui/layouts/a.layout".to_owned(), NAMED_PATH.to_owned())]);
    let json = serde_json::to_string(&model).unwrap();
    let back: modfoundry_gamediff::layout::LayoutModel = serde_json::from_str(&json).unwrap();
    assert_eq!(back, model);

    let old = layout_model(&[("gui/layouts/a.layout".to_owned(), BASE.to_owned())]);
    let new = layout_model(&[(
        "gui/layouts/a.layout".to_owned(),
        BASE.replace("position 1 1", "position 5 1"),
    )]);
    let diff = layout_diff(&old, &new);
    let back: modfoundry_gamediff::layout::LayoutDiff =
        serde_json::from_str(&serde_json::to_string(&diff).unwrap()).unwrap();
    assert_eq!(back, diff);
}

#[test]
fn a_changed_widget_reports_its_new_line() {
    let old = "FrameWidgetClass Root {\n position 0 0\n}\n";
    let new = "\n\nFrameWidgetClass Root {\n position 1 0\n}\n";
    let change = diff_of(old, new).expect("the position changed");
    assert_eq!(
        change.widgets_changed,
        [changed(
            "Root",
            3,
            None,
            &[("position", Some("0 0"), Some("1 0"))]
        )]
    );
}

#[test]
fn a_repeated_key_keeps_its_last_value_and_same_named_siblings_count_up() {
    let text = "FrameWidgetClass Root {\n color 1 1 1 1\n color 0 0 0 1\n {\n  TextWidgetClass T {\n  }\n  TextWidgetClass T {\n  }\n  TextWidgetClass T {\n  }\n }\n}\n";
    let model = layout_model(&[("gui/a.layout".to_owned(), text.to_owned())]);
    let file = &model.files["gui/a.layout"];
    assert_eq!(
        file.widgets["Root"].properties,
        props(&[("color", "0 0 0 1")])
    );
    let keys: Vec<&str> = file.widgets.keys().map(String::as_str).collect();
    assert_eq!(keys, ["Root", "Root/T", "Root/T#2", "Root/T#3"]);
}

#[test]
fn an_unchanged_file_that_does_not_parse_is_not_listed() {
    let broken = "FrameWidgetClass Root {\n";
    assert_eq!(diff_of(broken, broken), None);
}
