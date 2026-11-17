use modfoundry_config::ast::ConfigFile;
use modfoundry_gamediff::config::{
    config_diff, config_model, ClassChange, ClassRef, ConfigDiff, ConfigModel,
};
use modfoundry_gamediff::ValueChange;

fn parse(text: &str) -> ConfigFile {
    let parsed = modfoundry_config::parser::parse(text);
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    parsed.file
}

fn model(sources: &[(&str, &str)]) -> ConfigModel {
    let sources: Vec<(String, ConfigFile)> = sources
        .iter()
        .map(|(name, text)| ((*name).to_owned(), parse(text)))
        .collect();
    config_model(&sources)
}

/// One class split over two sources, the second overriding a property.
const TWO_FILES: &[(&str, &str)] = &[
    (
        "a",
        "class CfgVehicles { class Rifle_Base; class AKM: Rifle_Base { mass = 10; }; };",
    ),
    (
        "b",
        "class CfgVehicles { class AKM { mass = 12; displayName = \"AKM\"; }; };",
    ),
];

#[test]
fn a_class_defined_in_two_files_takes_the_later_value_and_keeps_its_parent() {
    let model = model(TWO_FILES);

    let akm = &model.classes["cfgvehicles/akm"];
    assert_eq!(akm.path, vec!["CfgVehicles".to_owned(), "AKM".to_owned()]);
    assert_eq!(akm.parent.as_deref(), Some("Rifle_Base"));
    assert_eq!(akm.sources, vec!["a".to_owned(), "b".to_owned()]);

    let mass = &akm.properties["mass"];
    assert_eq!(mass.value, "12");
    assert_eq!(mass.source, "b");
    assert!(!mass.append);

    let display_name = &akm.properties["displayname"];
    assert_eq!(display_name.name, "displayName");
    assert_eq!(display_name.value, "\"AKM\"");

    assert!(!model.classes.contains_key("cfgvehicles/rifle_base"));
    assert!(model.classes.contains_key("cfgvehicles"));
}

fn empty_diff() -> ConfigDiff {
    ConfigDiff {
        added: vec![],
        removed: vec![],
        changed: vec![],
    }
}

#[test]
fn config_diff_lists_each_kind_of_change() {
    fn class(path: &[&str]) -> ClassRef {
        ClassRef {
            path: path.iter().map(|name| (*name).to_owned()).collect(),
            sources: vec!["x".to_owned()],
        }
    }

    struct Case {
        name: &'static str,
        old: &'static str,
        new: &'static str,
        expected: ConfigDiff,
    }

    let cases = [
        Case {
            name: "class removed",
            old: "class Cfg { class Gone { p = 1; }; };",
            new: "class Cfg { };",
            expected: ConfigDiff {
                removed: vec![class(&["Cfg", "Gone"])],
                ..empty_diff()
            },
        },
        Case {
            name: "class added",
            old: "class Cfg { };",
            new: "class Cfg { class Fresh { p = 1; }; };",
            expected: ConfigDiff {
                added: vec![class(&["Cfg", "Fresh"])],
                ..empty_diff()
            },
        },
        Case {
            name: "parent changed",
            old: "class Cfg { class A: Before { }; };",
            new: "class Cfg { class A: After { }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: Some(ValueChange {
                        name: "parent".to_owned(),
                        old: Some("Before".to_owned()),
                        new: Some("After".to_owned()),
                    }),
                    properties: vec![],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "property value changed",
            old: "class Cfg { class A { p = 1; }; };",
            new: "class Cfg { class A { p = 2; }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: None,
                    properties: vec![ValueChange {
                        name: "p".to_owned(),
                        old: Some("1".to_owned()),
                        new: Some("2".to_owned()),
                    }],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "property removed",
            old: "class Cfg { class A { p = 1; }; };",
            new: "class Cfg { class A { }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: None,
                    properties: vec![ValueChange {
                        name: "p".to_owned(),
                        old: Some("1".to_owned()),
                        new: None,
                    }],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "property added",
            old: "class Cfg { class A { }; };",
            new: "class Cfg { class A { p = 2; }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: None,
                    properties: vec![ValueChange {
                        name: "p".to_owned(),
                        old: None,
                        new: Some("2".to_owned()),
                    }],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "array appended",
            old: "class Cfg { class A { items[] = {\"a\"}; }; };",
            new: "class Cfg { class A { items[] += {\"a\"}; }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: None,
                    properties: vec![ValueChange {
                        name: "items".to_owned(),
                        old: Some("{\"a\"}".to_owned()),
                        new: Some("+= {\"a\"}".to_owned()),
                    }],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "declared array without a value",
            old: "class Cfg { class A { items[] = {\"a\"}; }; };",
            new: "class Cfg { class A { items[]; }; };",
            expected: ConfigDiff {
                changed: vec![ClassChange {
                    path: vec!["Cfg".to_owned(), "A".to_owned()],
                    parent: None,
                    properties: vec![ValueChange {
                        name: "items".to_owned(),
                        old: Some("{\"a\"}".to_owned()),
                        new: Some(String::new()),
                    }],
                }],
                ..empty_diff()
            },
        },
        Case {
            name: "parent differs only in case",
            old: "class Cfg { class A: Base { }; };",
            new: "class Cfg { class A: base { }; };",
            expected: empty_diff(),
        },
    ];

    for case in &cases {
        let old = model(&[("x", case.old)]);
        let new = model(&[("x", case.new)]);
        assert_eq!(config_diff(&old, &new), case.expected, "{}", case.name);
    }
}

#[test]
fn a_binarized_copy_compares_equal_to_its_text() {
    let text = r#"class Cfg {
    class Item {
        count = 12;
        ratio = 1.0;
        half = 0.5;
        label = "Item";
        type = mod;
        nums[] = { 1, 2, 3 };
        names[] = { "a", "b" };
    };
};"#;
    let parsed = parse(text);
    let binary =
        modfoundry_config::read_rap(&modfoundry_config::write_rap(&parsed).unwrap()).unwrap();

    let from_text = model(&[("cfg", text)]);
    let from_binary = config_model(&[("cfg".to_owned(), binary)]);

    assert_eq!(config_diff(&from_text, &from_binary), empty_diff());
    assert_eq!(config_diff(&from_binary, &from_text), empty_diff());

    let item = &from_text.classes["cfg/item"];
    assert_eq!(item.properties["count"].value, "12");
    assert_eq!(item.properties["ratio"].value, "1");
    assert_eq!(item.properties["half"].value, "0.5");
    assert_eq!(item.properties["type"].value, "\"mod\"");
    assert_eq!(item.properties["nums"].value, "{1,2,3}");
    assert_eq!(item.properties["names"].value, "{\"a\",\"b\"}");
}

#[test]
fn delete_removes_the_class_and_everything_under_it() {
    let model = model(&[
        ("a", "class Cfg { class X { class Y { p = 1; }; }; };"),
        ("b", "class Cfg { delete X; };"),
    ]);

    assert!(model.classes.contains_key("cfg"));
    assert!(!model.classes.contains_key("cfg/x"));
    assert!(!model.classes.contains_key("cfg/x/y"));
}

#[test]
fn the_model_round_trips_through_json() {
    let model = model(TWO_FILES);
    let json = serde_json::to_string(&model).unwrap();
    let back: ConfigModel = serde_json::from_str(&json).unwrap();
    assert_eq!(back, model);
}

#[test]
fn a_class_that_only_moved_between_sources_is_unchanged() {
    let old = model(&[("a", "class Cfg { class Item: Base { p = 1; }; };")]);
    let new = model(&[("b", "class Cfg { class ITEM: BASE { P = 1; }; };")]);

    assert_ne!(
        old.classes["cfg/item"].sources,
        new.classes["cfg/item"].sources
    );
    assert_eq!(
        new.classes["cfg/item"].path,
        vec!["Cfg".to_owned(), "ITEM".to_owned()]
    );
    assert_eq!(config_diff(&old, &new), empty_diff());
}

#[test]
fn an_append_adds_to_the_array_an_earlier_file_set() {
    let merged = model(&[
        ("a", "class Cfg { class X { items[] = {\"a\"}; }; };"),
        ("b", "class Cfg { class X { items[] += {\"b\"}; }; };"),
        ("c", "class Cfg { class X { items[] += {\"c\"}; }; };"),
    ]);
    let items = &merged.classes["cfg/x"].properties["items"];
    assert_eq!(items.value, "{\"a\",\"b\",\"c\"}");
    assert!(!items.append);
    assert_eq!(items.source, "c");
}

#[test]
fn a_forward_declaration_gives_its_parent_to_a_class_without_one() {
    for sources in [
        [
            ("a", "class Cfg { class D: B; };"),
            ("b", "class Cfg { class D { z = 1; }; };"),
        ],
        [
            ("a", "class Cfg { class D { z = 1; }; };"),
            ("b", "class Cfg { class D: B; };"),
        ],
    ] {
        let merged = model(&sources);
        assert_eq!(
            merged.classes["cfg/d"].parent.as_deref(),
            Some("B"),
            "{sources:?}"
        );
    }
}

#[test]
fn delete_removes_a_property() {
    let merged = model(&[
        ("a", "class Cfg { class X { p = 1; q = 2; }; };"),
        ("b", "class Cfg { class X { delete p; }; };"),
    ]);
    let keys: Vec<&str> = merged.classes["cfg/x"]
        .properties
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["q"]);
}
