//! Mod configs and layout names checked against a config and layout compare.

use modfoundry_config::ast::ConfigFile;
use modfoundry_gamediff::config::{config_diff, config_model, ConfigModel};
use modfoundry_gamediff::hits::{HitKind, Level, ModConfig, ModInput};
use modfoundry_gamediff::hits_data::{config_hits, layout_hits};
use modfoundry_gamediff::layout::{layout_diff, layout_model};

const LAYOUT: &str = "FrameWidgetClass Root {\n position 0 0\n size 100 100\n}\n";
const CHANGED_LAYOUT: &str = "FrameWidgetClass Root {\n position 0 0\n size 200 100\n}\n";

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

fn mod_input(configs: &[(&str, &str)]) -> ModInput {
    ModInput {
        name: "TestMod".to_owned(),
        scripts: Vec::new(),
        configs: configs
            .iter()
            .map(|(path, text)| ModConfig {
                path: (*path).to_owned(),
                file: parse(text),
                text: Some((*text).to_owned()),
            })
            .collect(),
    }
}

#[test]
fn config_patches_and_children_of_removed_classes() {
    let old = model(&[("cfg", "class CfgVehicles { class Old { scope = 2; }; };")]);
    let new = model(&[("cfg", "class CfgVehicles { };")]);
    let diff = config_diff(&old, &new);
    let config = "class CfgVehicles {\n    class Old {\n        scope = 2;\n    };\n    class Mine: Old {\n    };\n};";
    let input = mod_input(&[("config.cpp", config)]);

    let hits = config_hits(&old, &diff, &input);

    assert_eq!(hits.len(), 2, "{hits:#?}");
    let patch = hits
        .iter()
        .find(|hit| hit.kind == HitKind::ConfigPatchRemoved)
        .expect("a patch of the removed class");
    assert_eq!(patch.level, Level::Check);
    assert_eq!(patch.place.file, "config.cpp");
    assert_eq!(patch.place.line, 2);
    assert_eq!(patch.vanilla, "CfgVehicles/Old");
    let child = hits
        .iter()
        .find(|hit| hit.kind == HitKind::ConfigParentRemoved)
        .expect("a child of the removed class");
    assert_eq!(child.level, Level::Breaks);
    assert_eq!(child.place.line, 5);
    assert_eq!(child.vanilla, "CfgVehicles/Old");
    assert!(hits
        .iter()
        .all(|hit| hit.mod_name == "TestMod" && !hit.maybe && !hit.diag_only));
}

#[test]
fn changed_classes_hit_only_where_the_mod_is_affected() {
    const OLD: &str = "class CfgVehicles { class Car { mass = 10; speed = 5; }; };";
    const PROPERTY_CHANGED: &str = "class CfgVehicles { class Car { mass = 12; speed = 5; }; };";
    const PARENT_CHANGED: &str =
        "class CfgVehicles { class Car: Other { mass = 10; speed = 5; }; };";

    struct Case {
        name: &'static str,
        new: &'static str,
        config: &'static str,
        expected: Vec<(HitKind, &'static str)>,
    }

    let cases = [
        Case {
            name: "a patch setting the changed property",
            new: PROPERTY_CHANGED,
            config: "class CfgVehicles { class Car { mass = 12; }; };",
            expected: vec![(HitKind::ConfigPatchChanged, "mass")],
        },
        Case {
            name: "a patch setting only other properties",
            new: PROPERTY_CHANGED,
            config: "class CfgVehicles { class Car { speed = 4; }; };",
            expected: vec![],
        },
        Case {
            name: "a child setting every changed property",
            new: PROPERTY_CHANGED,
            config: "class CfgVehicles { class Mine: Car { mass = 12; }; };",
            expected: vec![],
        },
        Case {
            name: "a child inheriting a changed property",
            new: PROPERTY_CHANGED,
            config: "class CfgVehicles { class Mine: Car { speed = 4; }; };",
            expected: vec![(HitKind::ConfigParentChanged, "mass")],
        },
        Case {
            name: "a changed vanilla parent of a patch and a child",
            new: PARENT_CHANGED,
            config: "class CfgVehicles { class Car { mass = 10; }; class Mine: Car {}; };",
            expected: vec![
                (HitKind::ConfigParentChanged, ""),
                (HitKind::ConfigPatchChanged, ""),
            ],
        },
    ];

    for case in &cases {
        let old = model(&[("cfg", OLD)]);
        let new = model(&[("cfg", case.new)]);
        let diff = config_diff(&old, &new);
        let input = mod_input(&[("config.cpp", case.config)]);

        let hits = config_hits(&old, &diff, &input);

        let kinds: Vec<HitKind> = hits.iter().map(|hit| hit.kind).collect();
        let expected: Vec<HitKind> = case.expected.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, expected, "{}", case.name);
        for (kind, detail) in &case.expected {
            if detail.is_empty() {
                continue;
            }
            let found = hits.iter().find(|hit| hit.kind == *kind).unwrap();
            assert!(found.detail.contains(detail), "{}: {found:#?}", case.name);
        }
        assert!(
            hits.iter().all(|hit| hit.vanilla == "CfgVehicles/Car"),
            "{}: {hits:#?}",
            case.name
        );
    }
}

#[test]
fn a_parent_is_found_through_scopes_and_mod_classes() {
    let old = model(&[(
        "cfg",
        "class CfgVehicles { class Car { mass = 10; }; class Other { mass = 1; }; };",
    )]);
    let new = model(&[(
        "cfg",
        "class CfgVehicles { class Car { mass = 12; }; class Other { mass = 1; }; };",
    )]);
    let diff = config_diff(&old, &new);
    let config = "class CfgVehicles {\n    class MyBase: Car {\n    };\n    class Mine: MyBase {\n    };\n};";
    let input = mod_input(&[("config.cpp", config)]);

    let hits = config_hits(&old, &diff, &input);

    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert!(hits
        .iter()
        .all(|hit| hit.kind == HitKind::ConfigParentChanged));
    assert_eq!(
        hits.iter().map(|hit| hit.place.line).collect::<Vec<_>>(),
        vec![2, 4]
    );
    assert!(hits.iter().all(|hit| hit.vanilla == "CfgVehicles/Car"));
}

#[test]
fn a_config_without_text_is_placed_at_line_zero() {
    let old = model(&[("cfg", "class CfgVehicles { class Car { mass = 10; }; };")]);
    let new = model(&[(
        "cfg",
        "class CfgVehicles { class Car: Other { mass = 10; }; };",
    )]);
    let diff = config_diff(&old, &new);
    let input = ModInput {
        name: "TestMod".to_owned(),
        scripts: Vec::new(),
        configs: vec![ModConfig {
            path: "addons/data/config.bin".to_owned(),
            file: parse("class CfgVehicles { class Mine: Car {}; };"),
            text: None,
        }],
    };

    let hits = config_hits(&old, &diff, &input);

    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].kind, HitKind::ConfigParentChanged);
    assert_eq!(hits[0].place.file, "addons/data/config.bin");
    assert_eq!(hits[0].place.line, 0);
}

#[test]
fn layout_names_in_mod_strings() {
    let old = layout_model(&[
        ("gui/layouts/old.layout".to_owned(), LAYOUT.to_owned()),
        ("gui/layouts/changed.layout".to_owned(), LAYOUT.to_owned()),
        ("gui/layouts/kept.layout".to_owned(), LAYOUT.to_owned()),
    ]);
    let new = layout_model(&[
        (
            "gui/layouts/changed.layout".to_owned(),
            CHANGED_LAYOUT.to_owned(),
        ),
        ("gui/layouts/kept.layout".to_owned(), LAYOUT.to_owned()),
    ]);
    let diff = layout_diff(&old, &new);
    let script = "\
class MyMenu {
    string m_Path = \"/gui/layouts/changed.layout\";

    void Open() {
        Widget w = GetGame().GetWorkspace().CreateWidgets(\"Gui\\\\Layouts\\\\old.layout\");
        Widget k = GetGame().GetWorkspace().CreateWidgets(\"gui/layouts/kept.layout\");
    }
}
";
    let input = ModInput {
        name: "TestMod".to_owned(),
        scripts: vec![("Scripts/5_Mission/MyMenu.c".to_owned(), script.to_owned())],
        configs: Vec::new(),
    };

    let hits = layout_hits(&diff, &input, &input.outlines());

    assert_eq!(hits.len(), 2, "{hits:#?}");
    let changed = hits
        .iter()
        .find(|hit| hit.kind == HitKind::LayoutChanged)
        .expect("the changed layout");
    assert_eq!(changed.level, Level::Check);
    assert_eq!(changed.place.line, 2);
    assert_eq!(changed.vanilla, "gui/layouts/changed.layout");
    assert_eq!(
        changed.detail, "0 widgets removed, 0 added, 1 changed",
        "{changed:#?}"
    );
    let removed = hits
        .iter()
        .find(|hit| hit.kind == HitKind::LayoutRemoved)
        .expect("the removed layout");
    assert_eq!(removed.level, Level::Breaks);
    assert_eq!(removed.place.line, 5);
    assert_eq!(removed.vanilla, "gui/layouts/old.layout");
    assert!(hits
        .iter()
        .all(|hit| hit.place.file == "Scripts/5_Mission/MyMenu.c"));
}

#[test]
fn a_change_is_inherited_through_the_whole_parent_chain() {
    let old = model(&[(
        "cfg",
        "class CfgVehicles { class Car { mass = 10; fuel = 1; }; class Sub: Car { fuel = 2; }; };",
    )]);
    let new = model(&[(
        "cfg",
        "class CfgVehicles { class Car { mass = 12; fuel = 5; }; class Sub: Car { fuel = 2; }; };",
    )]);
    let diff = config_diff(&old, &new);
    let config = "class CfgVehicles {\n    class Mine: Sub {\n    };\n    class MyBase: Car {\n        mass = 3;\n        fuel = 3;\n    };\n    class Quiet: MyBase {\n    };\n};";
    let input = mod_input(&[("config.cpp", config)]);

    let hits = config_hits(&old, &diff, &input);

    let found: Vec<(usize, &str, &str)> = hits
        .iter()
        .map(|hit| (hit.place.line, hit.vanilla.as_str(), hit.detail.as_str()))
        .collect();
    assert_eq!(found, [(2, "CfgVehicles/Car", "inherits mass")]);
}

#[test]
fn a_mod_class_takes_its_parent_from_any_of_its_declarations() {
    let old = model(&[(
        "cfg",
        "class CfgVehicles { class Car { mass = 10; }; class Old {}; };",
    )]);
    let new = model(&[("cfg", "class CfgVehicles { class Car { mass = 12; }; };")]);
    let diff = config_diff(&old, &new);
    let input = mod_input(&[
        ("a.cpp", "class CfgVehicles {\n    class MyBase {\n        z = 1;\n    };\n};"),
        (
            "b.cpp",
            "class CfgVehicles {\n    class MyBase: Car {};\n    class Mine: MyBase {};\n    class Fwd: Old;\n    class Fwd {\n        y = 1;\n    };\n};",
        ),
    ]);

    let hits = config_hits(&old, &diff, &input);

    let found: Vec<(&str, usize, HitKind)> = hits
        .iter()
        .map(|hit| (hit.place.file.as_str(), hit.place.line, hit.kind))
        .collect();
    assert_eq!(
        found,
        [
            ("a.cpp", 2, HitKind::ConfigParentChanged),
            ("b.cpp", 2, HitKind::ConfigParentChanged),
            ("b.cpp", 3, HitKind::ConfigParentChanged),
            ("b.cpp", 5, HitKind::ConfigParentRemoved),
        ]
    );
}

#[test]
fn a_deleted_property_is_not_inherited() {
    let old = model(&[("cfg", "class CfgVehicles { class Car { mass = 10; }; };")]);
    let new = model(&[("cfg", "class CfgVehicles { class Car { mass = 12; }; };")]);
    let diff = config_diff(&old, &new);
    let config = "class CfgVehicles {\n    class Mine: Car {\n        delete mass;\n    };\n};";

    let hits = config_hits(&old, &diff, &mod_input(&[("config.cpp", config)]));

    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn layout_names_in_defaults_and_array_values() {
    let old = layout_model(&[("gui/layouts/old.layout".to_owned(), LAYOUT.to_owned())]);
    let new = layout_model(&[]);
    let diff = layout_diff(&old, &new);
    let script = "class Menu\n{\n\tstatic const string PATHS[] = {\"gui/layouts/old.layout\"};\n\tvoid Open(string path = \"gui//layouts/old.layout\") {}\n}\n";
    let input = ModInput {
        name: "TestMod".to_owned(),
        scripts: vec![("Scripts/5_Mission/Menu.c".to_owned(), script.to_owned())],
        configs: Vec::new(),
    };

    let hits = layout_hits(&diff, &input, &input.outlines());

    let lines: Vec<(usize, HitKind)> = hits.iter().map(|hit| (hit.place.line, hit.kind)).collect();
    assert_eq!(
        lines,
        [(3, HitKind::LayoutRemoved), (4, HitKind::LayoutRemoved)]
    );
}
