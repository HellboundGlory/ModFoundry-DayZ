//! `conflict_data`'s load order, prefix checks and shared config entries.

use modfoundry_commands::conflict_data::{
    data_conflicts, load_order, read_packing, DataConflicts, DataMod, Finding, FindingPlace,
    PackedAddon, Packing,
};
use modfoundry_core::Severity;
use modfoundry_gamediff::hits::ModConfig;

use crate::common::{pack, tmp, write};

fn cfg(path: &str, text: &str) -> ModConfig {
    let parsed = modfoundry_config::parser::parse(text);
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    ModConfig {
        path: path.to_string(),
        file: parsed.file,
        text: Some(text.to_string()),
    }
}

fn bin(path: &str, text: &str) -> ModConfig {
    let parsed = modfoundry_config::parser::parse(text);
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    ModConfig {
        path: path.to_string(),
        file: parsed.file,
        text: None,
    }
}

fn packing(addons: &[(&str, &str, &[&str])]) -> Packing {
    Packing {
        addons: addons
            .iter()
            .map(|(prefix, base, files)| PackedAddon {
                prefix: (*prefix).to_string(),
                base: (*base).to_string(),
                files: files.iter().map(|f| (*f).to_string()).collect(),
            })
            .collect(),
        warnings: Vec::new(),
    }
}

#[derive(Default)]
struct Mods {
    names: Vec<String>,
    configs: Vec<Vec<ModConfig>>,
    packings: Vec<Packing>,
}

impl Mods {
    fn add(&mut self, name: &str, configs: Vec<ModConfig>) -> &mut Self {
        self.names.push(name.to_string());
        self.configs.push(configs);
        self.packings.push(Packing::default());
        self
    }

    fn add_packed(&mut self, name: &str, configs: Vec<ModConfig>, packing: Packing) -> &mut Self {
        self.add(name, configs);
        *self.packings.last_mut().unwrap() = packing;
        self
    }

    fn data(&self) -> Vec<DataMod<'_>> {
        (0..self.names.len())
            .map(|i| DataMod {
                name: &self.names[i],
                configs: &self.configs[i],
                packing: &self.packings[i],
            })
            .collect()
    }

    fn order(&self) -> Vec<usize> {
        load_order(&self.data())
    }

    fn conflicts(&self, order: &[usize]) -> DataConflicts {
        data_conflicts(&self.data(), order)
    }
}

fn patches(patch: &str, requires: &[&str]) -> Vec<ModConfig> {
    let required = requires
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    vec![cfg(
        "config.cpp",
        &format!(
            "class CfgPatches {{ class {patch} {{ requiredAddons[] = {{ {required} }}; }}; }};"
        ),
    )]
}

fn with_patches(cases: &[(&str, Option<&[&str]>)]) -> Vec<Vec<ModConfig>> {
    cases
        .iter()
        .map(|(name, req)| match req {
            Some(requires) => patches(name, requires),
            None => Vec::new(),
        })
        .collect()
}

fn order_of(cases: &[(&str, Option<&[&str]>)]) -> Vec<usize> {
    let mut mods = Mods::default();
    for (name, configs) in cases.iter().zip(with_patches(cases)) {
        mods.add(name.0, configs);
    }
    mods.order()
}

#[test]
fn load_order_is_a_stable_topological_sort_of_required_addons() {
    type Case<'a> = (&'a str, Vec<(&'a str, Option<&'a [&'a str]>)>, Vec<usize>);
    let cases: Vec<Case> = vec![
        (
            "no dependency keeps the given order",
            vec![("A", Some(&[][..])), ("B", Some(&[])), ("C", Some(&[]))],
            vec![0, 1, 2],
        ),
        (
            "a required patch loads first",
            vec![("A", Some(&["B"][..])), ("B", Some(&[])), ("C", Some(&[]))],
            vec![1, 0, 2],
        ),
        (
            "a chain across three loads the deepest first",
            vec![
                ("A", Some(&["B"][..])),
                ("B", Some(&["C"])),
                ("C", Some(&[])),
            ],
            vec![2, 1, 0],
        ),
        (
            "a name no mod in the set declares is ignored",
            vec![("A", Some(&["DZ_Data"][..])), ("B", Some(&[]))],
            vec![0, 1],
        ),
        (
            "a required name matches in any case",
            vec![("A", Some(&["b"][..])), ("B", Some(&[]))],
            vec![1, 0],
        ),
        (
            "a two-mod cycle keeps the given order",
            vec![("A", Some(&["B"][..])), ("B", Some(&["A"]))],
            vec![0, 1],
        ),
        (
            "a three-mod cycle keeps the given order",
            vec![
                ("A", Some(&["B"][..])),
                ("B", Some(&["C"])),
                ("C", Some(&["A"])),
            ],
            vec![0, 1, 2],
        ),
        (
            "a mod with no CfgPatches keeps its place",
            vec![("A", Some(&["B"][..])), ("B", Some(&[])), ("C", None)],
            vec![1, 0, 2],
        ),
    ];
    for (label, cases, want) in cases {
        assert_eq!(order_of(&cases), want, "{label}");
    }
}

#[test]
fn a_cycle_warns_for_each_pair_the_order_puts_the_wrong_way_round() {
    let mut mods = Mods::default();
    mods.add("A", patches("A", &["B"]));
    mods.add("B", patches("B", &["C"]));
    mods.add("C", patches("C", &["A"]));
    let order = mods.order();

    let conflicts = mods.conflicts(&order);

    assert_eq!(
        conflicts.warnings,
        [
            "A requires B, which loads after it (requiredAddons cycle); the given order is kept for them",
            "B requires C, which loads after it (requiredAddons cycle); the given order is kept for them",
        ]
    );
    assert!(conflicts.findings.is_empty());
}

#[test]
fn an_order_that_follows_every_requirement_warns_about_nothing() {
    let mut mods = Mods::default();
    mods.add("A", patches("A", &["B"]));
    mods.add("B", patches("B", &[]));
    let order = mods.order();

    assert_eq!(order, [1, 0]);
    assert!(mods.conflicts(&order).warnings.is_empty());
}

#[test]
fn a_mod_s_packing_warnings_are_prefixed_with_its_name() {
    let mut mods = Mods::default();
    let mut first = packing(&[("a", "", &["x.c"])]);
    first.warnings.push("Bad.pbo: not a pbo".into());
    mods.add_packed("A", Vec::new(), first);
    let mut second = packing(&[("b", "", &["y.c"])]);
    second.warnings.push("Worse.pbo: not a pbo".into());
    mods.add_packed("B", Vec::new(), second);

    assert_eq!(
        mods.conflicts(&[0, 1]).warnings,
        ["A: Bad.pbo: not a pbo", "B: Worse.pbo: not a pbo"]
    );
}

#[test]
fn a_patch_name_two_mods_declare_is_one_error_with_both_places_in_load_order() {
    let mut mods = Mods::default();
    mods.add(
        "Second",
        vec![cfg(
            "config.cpp",
            "class CfgPatches {\n    class Foo {};\n};",
        )],
    );
    mods.add(
        "First",
        vec![cfg(
            "config.cpp",
            "class CfgPatches {\n    class foo {};\n};",
        )],
    );
    let order = mods.order();

    let conflicts = mods.conflicts(&order);

    assert_eq!(conflicts.findings.len(), 1);
    let finding = &conflicts.findings[0];
    assert_eq!(finding.diagnostic.code, "MF-M201");
    assert_eq!(finding.diagnostic.severity, Severity::Error);
    assert_eq!(
        finding.diagnostic.message,
        "CfgPatches class \"Foo\" is declared by Second, First"
    );
    assert_eq!(finding.mods, ["Second", "First"]);
    assert_eq!(
        finding.places,
        [
            FindingPlace {
                mod_name: "Second".into(),
                path: "config.cpp".into(),
                line: Some(2),
            },
            FindingPlace {
                mod_name: "First".into(),
                path: "config.cpp".into(),
                line: Some(2),
            },
        ]
    );
}

#[test]
fn a_patch_name_declared_twice_in_one_mod_is_no_error() {
    let mut mods = Mods::default();
    mods.add(
        "One",
        vec![
            cfg("a.cpp", "class CfgPatches { class Foo {}; };"),
            cfg("b.cpp", "class CfgPatches { class Foo {}; };"),
        ],
    );
    mods.add("Other", patches("Other", &[]));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn a_patch_name_a_pbo_declares_has_a_place_with_no_line() {
    let mut mods = Mods::default();
    mods.add(
        "Bin",
        vec![bin("Bin.pbo", "class CfgPatches { class Foo {}; };")],
    );
    mods.add("Src", patches("Foo", &[]));

    let conflicts = mods.conflicts(&[0, 1]);

    assert_eq!(conflicts.findings.len(), 1);
    assert_eq!(conflicts.findings[0].places[0].path, "Bin.pbo");
    assert_eq!(conflicts.findings[0].places[0].line, None);
}

#[test]
fn a_patch_name_three_mods_declare_is_one_error_with_three_places() {
    let mut mods = Mods::default();
    mods.add("A", patches("Shared", &[]));
    mods.add("B", patches("Shared", &[]));
    mods.add("C", patches("Shared", &[]));

    let conflicts = mods.conflicts(&[0, 1, 2]);

    assert_eq!(conflicts.findings.len(), 1);
    assert_eq!(conflicts.findings[0].mods, ["A", "B", "C"]);
    assert_eq!(conflicts.findings[0].places.len(), 3);
}

#[test]
fn read_packing_reads_each_pbo_header_with_its_prefix_and_entry_names() {
    let root = tmp("conflict-packing-headers");
    pack(
        &root.join("Two.pbo"),
        &[("prefix", "JM\\CF")],
        &[("Scripts\\3_Game\\a.c", b"x"), ("config.cpp", b"y")],
    );
    pack(
        &root.join("One.pbo"),
        &[("prefix", "JM\\Other")],
        &[("README.txt", b"z")],
    );

    let packing = read_packing(&root);

    assert_eq!(
        packing,
        Packing {
            addons: vec![
                PackedAddon {
                    prefix: "JM/Other".into(),
                    base: "One.pbo".into(),
                    files: vec!["README.txt".into()],
                },
                PackedAddon {
                    prefix: "JM/CF".into(),
                    base: "Two.pbo".into(),
                    files: vec!["Scripts/3_Game/a.c".into(), "config.cpp".into()],
                },
            ],
            warnings: Vec::new(),
        }
    );
}

#[test]
fn a_pbo_without_a_prefix_property_has_the_empty_prefix() {
    let root = tmp("conflict-packing-no-prefix");
    pack(&root.join("Bare.pbo"), &[], &[("a.c", b"x")]);

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].prefix, "");
    assert_eq!(packing.addons[0].files, ["a.c"]);
}

#[test]
fn a_pbo_two_folders_deep_is_found() {
    let root = tmp("conflict-packing-deep");
    pack(
        &root.join("a").join("b").join("Deep.pbo"),
        &[("prefix", "Deep")],
        &[("x.c", b"x")],
    );

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].base, "a/b/Deep.pbo");
}

#[test]
fn a_pbo_whose_data_is_cut_off_still_lists_its_entries() {
    let root = tmp("conflict-packing-truncated");
    let pbo = root.join("Cut.pbo");
    pack(&pbo, &[("prefix", "Cut")], &[("a.c", b"0123456789")]);
    let full = std::fs::metadata(&pbo).unwrap().len();
    let file = std::fs::OpenOptions::new().write(true).open(&pbo).unwrap();
    file.set_len(full - 10).unwrap();

    let packing = read_packing(&root);

    assert!(packing.warnings.is_empty(), "{:?}", packing.warnings);
    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].files, ["a.c"]);
}

#[test]
fn an_ebo_is_skipped_with_a_warning_and_nothing_else_is_listed() {
    let root = tmp("conflict-packing-ebo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Map.ebo"), b"encrypted").unwrap();

    let packing = read_packing(&root);

    assert!(packing.addons.is_empty());
    assert_eq!(packing.warnings.len(), 1);
    assert!(
        packing.warnings[0].contains("Map.ebo")
            && packing.warnings[0].contains(".ebo is not supported"),
        "{:?}",
        packing.warnings
    );
}

#[test]
fn an_unreadable_pbo_warns_and_the_other_pbo_is_still_read() {
    let root = tmp("conflict-packing-garbage");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Bad.pbo"), b"not a pbo at all").unwrap();
    pack(
        &root.join("Good.pbo"),
        &[("prefix", "Good")],
        &[("g.c", b"g")],
    );

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].base, "Good.pbo");
    assert_eq!(packing.warnings.len(), 1);
    assert!(
        packing.warnings[0].contains("Bad.pbo"),
        "{:?}",
        packing.warnings
    );
}

#[test]
fn loose_files_beside_a_pbo_are_not_listed() {
    let root = tmp("conflict-packing-loose-beside");
    pack(
        &root.join("Mod.pbo"),
        &[("prefix", "Mod")],
        &[("m.c", b"m")],
    );
    write(&root, "config.cpp", "class C {};");

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].files, ["m.c"]);
}

#[test]
fn a_source_folder_with_no_marker_is_one_addon_named_after_the_folder() {
    let base = tmp("conflict-packing-source");
    let root = base.join("@MyMod");
    write(&root, "config.cpp", "class C {};");
    write(&root, "Scripts/3_Game/a.c", "x");
    write(&root, "mod.cpp", "name = \"MyMod\";");
    write(&root, "meta.cpp", "x");
    write(&root, "keys/key.bikey", "x");
    write(&root, "builds/x/y.c", "x");
    write(&root, ".hidden", "x");

    let packing = read_packing(&root);

    assert_eq!(
        packing,
        Packing {
            addons: vec![PackedAddon {
                prefix: "MyMod".into(),
                base: String::new(),
                files: vec!["Scripts/3_Game/a.c".into(), "config.cpp".into()],
            }],
            warnings: Vec::new(),
        }
    );
}

#[test]
fn a_marker_in_a_subfolder_makes_that_folder_an_addon_of_its_own() {
    let base = tmp("conflict-packing-marker");
    let root = base.join("@Mod");
    write(&root, "config.cpp", "class C {};");
    write(&root, "Scripts/3_Game/a.c", "x");
    write(
        &root,
        "Extra/$PBOPREFIX$",
        "\n  \\JM/Extra\\  \nsecond line\n",
    );
    write(&root, "Extra/3_Game/b.c", "y");

    let packing = read_packing(&root);

    assert_eq!(
        packing,
        Packing {
            addons: vec![
                PackedAddon {
                    prefix: "Mod".into(),
                    base: String::new(),
                    files: vec!["Scripts/3_Game/a.c".into(), "config.cpp".into()],
                },
                PackedAddon {
                    prefix: "JM/Extra".into(),
                    base: "Extra".into(),
                    files: vec!["3_Game/b.c".into()],
                },
            ],
            warnings: Vec::new(),
        }
    );
}

#[test]
fn an_upper_case_marker_file_name_is_read_too() {
    let base = tmp("conflict-packing-marker-case");
    let root = base.join("Mod");
    write(&root, "Sub/$PBOPREFIX$.TXT", "JM/Sub");
    write(&root, "Sub/a.c", "x");

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].prefix, "JM/Sub");
    assert_eq!(packing.addons[0].files, ["a.c"]);
}

#[test]
fn an_empty_marker_warns_and_makes_no_addon() {
    let base = tmp("conflict-packing-marker-empty");
    let root = base.join("Mod");
    write(&root, "Empty/$PBOPREFIX$", "  \n\t\n");
    write(&root, "Empty/a.c", "x");

    let packing = read_packing(&root);

    assert_eq!(
        packing,
        Packing {
            addons: vec![PackedAddon {
                prefix: "Mod".into(),
                base: String::new(),
                files: vec!["Empty/a.c".into()],
            }],
            warnings: vec!["Empty/$PBOPREFIX$: no prefix in it".into()],
        }
    );
}

#[test]
fn the_same_prefix_in_two_mods_is_one_error_with_two_places() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("jm\\cf", "b", &["y.c"])]));

    let conflicts = mods.conflicts(&[0, 1]);

    assert_eq!(conflicts.findings.len(), 1);
    let finding = &conflicts.findings[0];
    assert_eq!(finding.diagnostic.code, "MF-M202");
    assert_eq!(
        finding.diagnostic.message,
        "PBO prefix \"JM/CF\" is used by A, B"
    );
    assert_eq!(
        finding.places,
        [
            FindingPlace {
                mod_name: "A".into(),
                path: ".".into(),
                line: None,
            },
            FindingPlace {
                mod_name: "B".into(),
                path: "b".into(),
                line: None,
            },
        ]
    );
}

#[test]
fn an_empty_prefix_in_two_mods_is_no_error() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("", "", &["y.c"])]));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn the_same_prefix_in_two_pbos_of_one_mod_is_no_error() {
    let mut mods = Mods::default();
    mods.add_packed(
        "A",
        Vec::new(),
        packing(&[
            ("JM/CF", "one.pbo", &["x.c"]),
            ("JM/CF", "two.pbo", &["y.c"]),
        ]),
    );
    mods.add_packed("B", Vec::new(), packing(&[("Other", "", &["z.c"])]));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn a_prefix_inside_another_mod_s_prefix_is_a_warning() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("JM/CF/Scripts", "", &["y.c"])]));

    let conflicts = mods.conflicts(&[0, 1]);

    assert_eq!(conflicts.findings.len(), 1);
    let finding = &conflicts.findings[0];
    assert_eq!(finding.diagnostic.code, "MF-M203");
    assert_eq!(finding.diagnostic.severity, Severity::Warning);
    assert_eq!(
        finding.diagnostic.message,
        "PBO prefix \"JM/CF/Scripts\" of B is inside \"JM/CF\" of A"
    );
    assert_eq!(finding.mods, ["A", "B"]);
}

#[test]
fn a_prefix_that_only_shares_a_stem_is_not_inside() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("JM/CFx", "", &["y.c"])]));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn two_mods_with_the_same_prefix_get_no_nesting_warning() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("JM/CF", "", &["y.c"])]));

    let codes: Vec<String> = mods
        .conflicts(&[0, 1])
        .findings
        .iter()
        .map(|f| f.diagnostic.code.clone())
        .collect();

    assert_eq!(codes, ["MF-M202"]);
}

#[test]
fn the_same_game_path_in_two_mods_is_one_error() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("a/b", "", &["c/x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("A", "", &["b\\c\\x.c"])]));

    let conflicts = mods.conflicts(&[0, 1]);

    let reported: Vec<&Finding> = conflicts
        .findings
        .iter()
        .filter(|f| f.diagnostic.code == "MF-M204")
        .collect();
    assert_eq!(reported.len(), 1, "{:#?}", conflicts.findings);
    let finding = reported[0];
    assert_eq!(finding.diagnostic.severity, Severity::Error);
    assert_eq!(finding.diagnostic.message, "a/b/c/x.c is packed by A, B");
    assert_eq!(
        finding.places,
        [
            FindingPlace {
                mod_name: "A".into(),
                path: "c/x.c".into(),
                line: None,
            },
            FindingPlace {
                mod_name: "B".into(),
                path: "b/c/x.c".into(),
                line: None,
            },
        ]
    );
}

#[test]
fn a_path_under_a_reported_shared_prefix_is_not_reported_twice() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("JM/CF", "", &["x.c"])]));

    let codes: Vec<String> = mods
        .conflicts(&[0, 1])
        .findings
        .iter()
        .map(|f| f.diagnostic.code.clone())
        .collect();

    assert_eq!(codes, ["MF-M202"]);
}

#[test]
fn a_path_one_mod_packs_twice_is_no_error() {
    let mut mods = Mods::default();
    mods.add_packed(
        "A",
        Vec::new(),
        packing(&[("a", "", &["x.c"]), ("a", "p2", &["x.c"])]),
    );
    mods.add_packed("B", Vec::new(), packing(&[("b", "", &["y.c"])]));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

fn vehicle_class(properties: &str) -> Vec<ModConfig> {
    vec![cfg(
        "config.cpp",
        &format!("class CfgVehicles {{\n    class AKM {{\n{properties}\n    }};\n}};"),
    )]
}

#[test]
fn a_property_two_mods_set_is_one_warning_naming_the_later_mod_as_winner() {
    let mut mods = Mods::default();
    mods.add("A", vehicle_class("        mass = 10;"));
    mods.add("B", vehicle_class("        mass = 20;"));

    let conflicts = mods.conflicts(&[0, 1]);

    assert_eq!(conflicts.findings.len(), 1);
    let finding = &conflicts.findings[0];
    assert_eq!(finding.diagnostic.code, "MF-M205");
    assert_eq!(finding.diagnostic.severity, Severity::Warning);
    assert_eq!(
        finding.diagnostic.message,
        "CfgVehicles/AKM: mass (B wins) set by more than one mod"
    );
    assert_eq!(finding.mods, ["A", "B"]);
    assert_eq!(
        finding.places,
        [
            FindingPlace {
                mod_name: "A".into(),
                path: "config.cpp".into(),
                line: Some(2),
            },
            FindingPlace {
                mod_name: "B".into(),
                path: "config.cpp".into(),
                line: Some(2),
            },
        ]
    );
}

#[test]
fn reversing_the_load_order_flips_the_winner() {
    let mut mods = Mods::default();
    mods.add("A", vehicle_class("        mass = 10;"));
    mods.add("B", vehicle_class("        mass = 20;"));

    let forward = mods.conflicts(&[0, 1]);
    let reversed = mods.conflicts(&[1, 0]);

    assert_eq!(
        forward.findings[0].diagnostic.message,
        "CfgVehicles/AKM: mass (B wins) set by more than one mod"
    );
    assert_eq!(
        reversed.findings[0].diagnostic.message,
        "CfgVehicles/AKM: mass (A wins) set by more than one mod"
    );
    assert_eq!(reversed.findings[0].mods, ["B", "A"]);
}

#[test]
fn different_properties_appends_and_one_mod_s_own_overlap_are_no_finding() {
    let mut mods = Mods::default();
    mods.add(
        "A",
        vehicle_class("        mass = 10;\n        items[] += { \"a\" };"),
    );
    mods.add(
        "B",
        vehicle_class("        scope = 2;\n        items[] += { \"b\" };"),
    );

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());

    let mut one = Mods::default();
    one.add("A", vehicle_class("        mass = 10;"));
    one.add(
        "B",
        vec![
            cfg(
                "one.cpp",
                "class CfgVehicles { class AKM { mass = 10; }; };",
            ),
            cfg(
                "two.cpp",
                "class CfgVehicles { class AKM { mass = 30; }; };",
            ),
        ],
    );
    one.add("C", vehicle_class("        mass = 40;"));

    let findings = one.conflicts(&[0, 1, 2]).findings;
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].diagnostic.message,
        "CfgVehicles/AKM: mass (C wins) set by more than one mod"
    );
    assert_eq!(findings[0].mods, ["A", "B", "C"]);
    assert_eq!(findings[0].places[1].path, "one.cpp");
}

#[test]
fn a_delete_in_one_mod_and_a_set_in_another_is_a_finding() {
    let mut mods = Mods::default();
    mods.add("A", vehicle_class("        delete mass;"));
    mods.add("B", vehicle_class("        mass = 20;"));

    let findings = mods.conflicts(&[0, 1]).findings;

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].diagnostic.code, "MF-M205");
}

#[test]
fn cfgpatches_required_addons_and_depth_one_classes_are_no_finding() {
    let mut mods = Mods::default();
    mods.add("A", patches("A", &["DZ_Data"]));
    mods.add("B", patches("B", &["DZ_Data"]));

    let findings = mods.conflicts(&[0, 1]).findings;
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.diagnostic.code != "MF-M201")
            .count(),
        0
    );

    let mut shallow = Mods::default();
    shallow.add("A", vec![cfg("a.cpp", "class CfgVehicles { mass = 10; };")]);
    shallow.add("B", vec![cfg("b.cpp", "class CfgVehicles { mass = 20; };")]);
    assert!(shallow.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn a_forward_declaration_sets_nothing() {
    let mut mods = Mods::default();
    mods.add("A", vec![cfg("a.cpp", "class CfgVehicles { class AKM; };")]);
    mods.add("B", vehicle_class("        mass = 20;"));

    assert!(mods.conflicts(&[0, 1]).findings.is_empty());
}

#[test]
fn a_nested_class_keeps_its_full_path_and_two_classes_give_two_findings() {
    let mut mods = Mods::default();
    let nested = "class CfgVehicles {\n\
                  \x20   class AKM {\n\
                  \x20       class Damage {\n\
                  \x20           health = 1;\n\
                  \x20       };\n\
                  \x20       mass = 10;\n\
                  \x20   };\n\
                  };";
    mods.add("A", vec![cfg("config.cpp", nested)]);
    mods.add(
        "B",
        vec![cfg(
            "config.cpp",
            "class CfgVehicles {\n\
             \x20   class AKM {\n\
             \x20       class Damage { health = 2; };\n\
             \x20       mass = 20;\n\
             \x20   };\n\
             };",
        )],
    );

    let findings = mods.conflicts(&[0, 1]).findings;

    assert_eq!(findings.len(), 2);
    assert_eq!(
        findings[0].diagnostic.message,
        "CfgVehicles/AKM: mass (B wins) set by more than one mod"
    );
    assert_eq!(
        findings[1].diagnostic.message,
        "CfgVehicles/AKM/Damage: health (B wins) set by more than one mod"
    );
    assert_eq!(findings[0].places[0].line, Some(2));
}

#[test]
fn a_later_mod_requiring_the_earlier_one_makes_it_a_hint() {
    let mut mods = Mods::default();
    mods.add("A", {
        let mut configs = patches("A", &[]);
        configs.extend(vehicle_class("        mass = 10;"));
        configs
    });
    mods.add("B", {
        let mut configs = patches("B", &["A"]);
        configs.extend(vehicle_class("        mass = 20;"));
        configs
    });

    let order = mods.order();
    let conflicts = mods.conflicts(&order);

    let finding = conflicts
        .findings
        .iter()
        .find(|f| f.diagnostic.message.contains("mass"))
        .expect("a property finding");
    assert_eq!(finding.diagnostic.code, "MF-M206");
    assert_eq!(finding.diagnostic.severity, Severity::Hint);
}

#[test]
fn requiring_the_earlier_mod_through_a_third_one_also_makes_it_a_hint() {
    let mut mods = Mods::default();
    mods.add("A", {
        let mut configs = patches("A", &[]);
        configs.extend(vehicle_class("        mass = 10;"));
        configs
    });
    mods.add("Bridge", patches("Bridge", &["A"]));
    mods.add("B", {
        let mut configs = patches("B", &["Bridge"]);
        configs.extend(vehicle_class("        mass = 20;"));
        configs
    });

    let order = mods.order();
    let conflicts = mods.conflicts(&order);

    let finding = conflicts
        .findings
        .iter()
        .find(|f| f.diagnostic.message.contains("mass"))
        .expect("a property finding");
    assert_eq!(finding.diagnostic.code, "MF-M206");
}

#[test]
fn later_mods_that_require_only_the_first_still_get_a_warning() {
    let mut mods = Mods::default();
    mods.add("A", {
        let mut configs = patches("A", &[]);
        configs.extend(vehicle_class("        mass = 10;"));
        configs
    });
    for name in ["B", "C"] {
        mods.add(name, {
            let mut configs = patches(name, &["A"]);
            configs.extend(vehicle_class("        mass = 20;"));
            configs
        });
    }

    let order = mods.order();
    let finding = mods
        .conflicts(&order)
        .findings
        .into_iter()
        .find(|f| f.diagnostic.message.contains("mass"))
        .expect("a property finding");

    assert_eq!(finding.diagnostic.code, "MF-M205");
    assert_eq!(finding.diagnostic.severity, Severity::Warning);
}

#[test]
fn requiring_a_mod_outside_the_set_still_gives_a_warning() {
    let mut mods = Mods::default();
    mods.add("A", vehicle_class("        mass = 10;"));
    mods.add("B", {
        let mut configs = patches("B", &["DZ_Data"]);
        configs.extend(vehicle_class("        mass = 20;"));
        configs
    });

    let finding = mods
        .conflicts(&[0, 1])
        .findings
        .into_iter()
        .find(|f| f.diagnostic.message.contains("mass"))
        .expect("a property finding");

    assert_eq!(finding.diagnostic.code, "MF-M205");
}

#[test]
fn findings_come_out_sorted_by_code_and_a_second_run_is_equal() {
    let mass_mod = |patch: &str, mass: &str| {
        let mut configs = patches(patch, &[]);
        configs.extend(vehicle_class(&format!("        mass = {mass};")));
        configs
    };
    let mut mods = Mods::default();
    mods.add_packed(
        "A",
        mass_mod("Shared", "10"),
        packing(&[("q", "", &["a.c"]), ("", "", &["shared/x.c"])]),
    );
    mods.add_packed(
        "B",
        mass_mod("Shared", "20"),
        packing(&[("Q", "", &["b.c"]), ("", "", &["shared/x.c"])]),
    );

    let first = mods.conflicts(&[0, 1]);
    let second = mods.conflicts(&[0, 1]);

    let codes: Vec<&str> = first
        .findings
        .iter()
        .map(|f| f.diagnostic.code.as_str())
        .collect();
    assert_eq!(codes, ["MF-M201", "MF-M202", "MF-M204", "MF-M205"]);
    assert_eq!(first, second);
}

#[test]
fn a_cycle_keeps_an_unrelated_mod_between_its_members_in_place() {
    let order = order_of(&[
        ("A", Some(&["C"][..])),
        ("B", Some(&[])),
        ("C", Some(&["A"])),
    ]);
    assert_eq!(order, [0, 1, 2]);
}

#[test]
fn a_marker_in_the_mod_folder_sets_the_mod_s_own_prefix() {
    let base = tmp("conflict-packing-root-marker");
    let root = base.join("Mod");
    write(&root, "$PBOPREFIX$", "JM\\Root");
    write(&root, "a.c", "x");

    let packing = read_packing(&root);

    assert_eq!(packing.addons.len(), 1);
    assert_eq!(packing.addons[0].prefix, "JM/Root");
    assert_eq!(packing.addons[0].files, ["a.c"]);
}

#[test]
fn a_nested_prefix_warning_names_two_different_mods() {
    let mut mods = Mods::default();
    mods.add_packed(
        "A",
        Vec::new(),
        packing(&[("x", "one", &["a.c"]), ("x/y", "two", &["b.c"])]),
    );
    mods.add_packed("B", Vec::new(), packing(&[("x/y", "", &["c.c"])]));

    let conflicts = mods.conflicts(&[0, 1]);

    let nested: Vec<&Finding> = conflicts
        .findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-M203")
        .collect();
    assert_eq!(nested.len(), 1);
    assert_eq!(
        nested[0].diagnostic.message,
        "PBO prefix \"x/y\" of B is inside \"x\" of A"
    );
}

#[test]
fn many_shared_paths_are_checked_quickly() {
    let files: Vec<String> = (0..15_000).map(|i| format!("data/f{i}.paa")).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let mut mods = Mods::default();
    for name in ["A", "B", "C", "D"] {
        let prefix = format!("{name}/pfx");
        mods.add_packed(name, Vec::new(), packing(&[(prefix.as_str(), "", &refs)]));
    }

    let started = std::time::Instant::now();
    let conflicts = mods.conflicts(&[0, 1, 2, 3]);

    assert!(conflicts.findings.is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_path_under_a_prefix_that_only_shares_a_stem_with_a_reported_one_is_still_reported() {
    let mut mods = Mods::default();
    mods.add_packed("A", Vec::new(), packing(&[("a/b", "", &["x.c"])]));
    mods.add_packed("B", Vec::new(), packing(&[("a/b", "", &["x.c"])]));
    mods.add_packed("C", Vec::new(), packing(&[("a/bc", "", &["x.c"])]));
    mods.add_packed("D", Vec::new(), packing(&[("a", "", &["bc/x.c"])]));

    let conflicts = mods.conflicts(&[0, 1, 2, 3]);

    let shared: Vec<&str> = conflicts
        .findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-M204")
        .map(|finding| finding.diagnostic.message.as_str())
        .collect();
    assert_eq!(shared, ["a/bc/x.c is packed by C, D"]);
}
