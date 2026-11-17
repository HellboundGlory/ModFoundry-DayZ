use std::collections::{BTreeMap, BTreeSet};

use modfoundry_layout::parse::{parse, Pos, Property, Value, Widget};
use modfoundry_layout::vocab::{pattern, Shape, Vocabulary, MAX_WORDS};
use modfoundry_pbo::PboArchive;
use std::path::Path;

fn shape(patterns: &[&str], words: Option<&[&str]>) -> Shape {
    Shape {
        patterns: patterns.iter().map(|p| p.to_string()).collect(),
        words: words.map(|ws| ws.iter().map(|w| w.to_string()).collect()),
    }
}

fn class(properties: &[(&str, Shape)]) -> BTreeMap<String, Shape> {
    properties
        .iter()
        .map(|(key, shape)| (key.to_string(), shape.clone()))
        .collect()
}

#[test]
fn learn_records_every_class_with_its_property_shapes() {
    let text = "\
PanelWidgetClass root {
 color 1 0 0 1
 halign center_ref
 {
  ScriptParamsClass {
   AlignChilds 1
  }
  TextWidgetClass label {
   text \"hi\"
   halign center_ref
  }
  SpacerWidgetClass gap {
  }
 }
}
";
    let mut vocab = Vocabulary::default();
    vocab.learn(&parse(text).unwrap());
    let expected = BTreeMap::from([
        (
            "PanelWidgetClass".to_string(),
            class(&[
                ("color", shape(&["nnnn"], Some(&[]))),
                ("halign", shape(&["w"], Some(&["center_ref"]))),
            ]),
        ),
        (
            "ScriptParamsClass".to_string(),
            class(&[("AlignChilds", shape(&["n"], Some(&[])))]),
        ),
        (
            "TextWidgetClass".to_string(),
            class(&[
                ("text", shape(&["s"], Some(&[]))),
                ("halign", shape(&["w"], Some(&["center_ref"]))),
            ]),
        ),
        ("SpacerWidgetClass".to_string(), BTreeMap::new()),
    ]);
    assert_eq!(vocab.classes, expected);
}

#[test]
fn pattern_has_one_letter_per_value_kind() {
    let values = [
        Value::Number("0.5".into()),
        Value::Word("center".into()),
        Value::Str("x".into()),
    ];
    assert_eq!(pattern(&values), "nws");
    assert_eq!(pattern(&[]), "");
}

#[test]
fn a_property_with_no_values_learns_nothing() {
    let widget = Widget {
        class: "TextWidgetClass".into(),
        name: None,
        pos: Pos { line: 1, column: 1 },
        end_line: 1,
        properties: vec![
            Property {
                key: "empty".into(),
                pos: Pos { line: 2, column: 1 },
                values: Vec::new(),
            },
            Property {
                key: "text".into(),
                pos: Pos { line: 3, column: 1 },
                values: vec![Value::Str("hi".into())],
            },
        ],
        children: Vec::new(),
    };
    let mut vocab = Vocabulary::default();
    vocab.learn(&[widget]);
    let keys: Vec<&str> = vocab.classes["TextWidgetClass"]
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["text"]);
    assert!(
        vocab
            .classes
            .values()
            .flat_map(BTreeMap::values)
            .all(|shape| !shape.patterns.contains("")),
        "{:?}",
        vocab.classes
    );
}

#[test]
fn a_new_shape_starts_with_no_words_seen() {
    let shape = Shape::default();
    assert_eq!(shape.words, Some(Default::default()));
}

#[test]
fn words_stay_unknown_once_the_limit_is_passed() {
    let words: String = (0..MAX_WORDS).map(|i| format!(" word{i}")).collect();
    let mut vocab = Vocabulary::default();
    vocab.learn(&parse(&format!("TextWidgetClass {{\n tag{words}\n}}\n")).unwrap());
    assert_eq!(
        vocab.classes["TextWidgetClass"]["tag"]
            .words
            .as_ref()
            .map(|w| w.len()),
        Some(MAX_WORDS)
    );

    vocab.learn(&parse("TextWidgetClass {\n tag word_extra\n}\n").unwrap());
    assert_eq!(vocab.classes["TextWidgetClass"]["tag"].words, None);

    vocab.learn(&parse("TextWidgetClass {\n tag word0\n}\n").unwrap());
    assert_eq!(vocab.classes["TextWidgetClass"]["tag"].words, None);
}

#[test]
fn script_widgets_are_the_classes_that_derive_from_widget() {
    type Case<'a> = (&'a [(&'a str, Option<&'a str>)], &'a [&'a str]);
    let cases: [Case; 5] = [
        (
            &[
                ("RenderTargetWidget", Some("Widget")),
                ("GridSpacerWidget", Some("SpacerWidget")),
                ("SpacerWidget", Some("Widget")),
            ],
            &[
                "RenderTargetWidgetClass",
                "GridSpacerWidgetClass",
                "SpacerWidgetClass",
            ],
        ),
        (
            &[
                ("Widget", None),
                ("ScriptedWidgetEventHandler", Some("Managed")),
                ("Managed", None),
            ],
            &[],
        ),
        (
            &[
                ("Loop", Some("Other")),
                ("Other", Some("Loop")),
                ("Button", Some("Widget")),
            ],
            &["ButtonClass"],
        ),
        (&[("Orphan", Some("Unknown"))], &[]),
        (
            &[("TextWidget", Some("Widget")), ("TextWidget", None)],
            &["TextWidgetClass"],
        ),
    ];
    for (classes, expected) in cases {
        let classes: Vec<(String, Option<String>)> = classes
            .iter()
            .map(|(name, base)| (name.to_string(), base.map(str::to_string)))
            .collect();
        let mut vocab = Vocabulary::default();
        vocab.add_script_widgets(&classes);
        let expected: BTreeSet<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(vocab.script_widgets, expected, "{classes:?}");
    }
}

#[test]
fn shape_anywhere_merges_the_shapes_of_every_class() {
    let mut vocab = Vocabulary::default();
    vocab.learn(
        &parse("TextWidgetClass {\n halign center_ref\n size 1 1\n}\nImageWidgetClass {\n halign left\n size fill\n}\n")
            .unwrap(),
    );
    assert_eq!(
        vocab.shape_anywhere("size"),
        Some(shape(&["nn", "w"], Some(&["fill"])))
    );
    assert_eq!(
        vocab.shape_anywhere("halign"),
        Some(shape(&["w"], Some(&["center_ref", "left"])))
    );
    assert_eq!(vocab.shape_anywhere("missing"), None);
}

#[test]
fn shape_anywhere_drops_words_once_the_merged_list_is_too_long() {
    let first: String = (0..MAX_WORDS).map(|i| format!(" a{i}")).collect();
    let second: String = (0..MAX_WORDS).map(|i| format!(" b{i}")).collect();
    let mut vocab = Vocabulary::default();
    vocab.learn(
        &parse(&format!(
            "TextWidgetClass {{\n tag{first}\n}}\nImageWidgetClass {{\n tag{second}\n}}\n"
        ))
        .unwrap(),
    );
    assert_eq!(vocab.shape_anywhere("tag").unwrap().words, None);
}

#[test]
fn knows_learned_and_script_classes_only() {
    let mut vocab = Vocabulary::default();
    vocab.learn(&parse("TextWidgetClass {\n text \"x\"\n}\n").unwrap());
    vocab.add_script_widgets(&[("RenderTargetWidget".into(), Some("Widget".into()))]);
    assert!(vocab.knows_class("TextWidgetClass"));
    assert!(vocab.knows_class("RenderTargetWidgetClass"));
    assert!(!vocab.knows_class("ImageWidgetClass"));
}

#[test]
#[ignore]
fn learns_the_vanilla_gui_layouts() {
    let Some(install) = std::env::var_os("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let archive = PboArchive::open(&Path::new(&install).join("dta/gui.pbo")).unwrap();
    let mut vocab = Vocabulary::default();
    let mut files = 0;
    let mut failures = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        if !entry.name_str().ends_with(".layout") {
            continue;
        }
        let name = entry.name_str().replace('\\', "/");
        let text = String::from_utf8_lossy(&archive.read_entry(index).unwrap()).into_owned();
        match parse(&text) {
            Ok(widgets) => {
                vocab.learn(&widgets);
                files += 1;
            }
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }

    let properties: usize = vocab.classes.values().map(|props| props.len()).sum();
    println!(
        "{files} layout files, {} classes, {properties} properties",
        vocab.classes.len()
    );
    let mut ranked: Vec<(usize, String)> = vocab
        .classes
        .iter()
        .flat_map(|(class, props)| {
            props
                .iter()
                .map(move |(key, shape)| (shape.patterns.len(), format!("{class}.{key}")))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (count, name) in ranked.iter().take(3) {
        println!("{name}: {count} patterns");
    }
    assert!(vocab.classes["TextWidgetClass"].contains_key("text"));
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn shape_anywhere_has_no_word_list_when_one_class_accepts_any_word() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "ImageWidgetClass".into(),
        class(&[("tag", shape(&["w"], Some(&["a"])))]),
    );
    vocab.classes.insert(
        "TextWidgetClass".into(),
        class(&[("tag", shape(&["w"], None))]),
    );
    assert_eq!(vocab.shape_anywhere("tag").unwrap().words, None);
}
