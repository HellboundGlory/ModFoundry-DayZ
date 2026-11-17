//! `game.diff` through the command registry.

use modfoundry_commands::game_diff::{
    game_diff, read_mod, render_text, GameDiffReport, GameDiffRequest, SideInfo, Summary,
};
use modfoundry_commands::game_snapshot::save_snapshot;
use modfoundry_gamediff::config::{config_model, ConfigDiff};
use modfoundry_gamediff::hits::{Hit, HitKind, Level};
use modfoundry_gamediff::layout::{layout_model, LayoutDiff};
use modfoundry_gamediff::script::{script_model, Place};
use modfoundry_gamediff::script_diff::script_diff;
use modfoundry_gamediff::snapshot::Snapshot;

use crate::common::{config_bin, pack, tmp, write};

const SCRIPT_OLD: &str = "class A\n{\n\tvoid F() {}\n\n\tvoid G() {}\n}\n";
const SCRIPT_NEW: &str = "class A\n{\n\tvoid F() {}\n}\n";
const CONFIG_OLD: &str = "class CfgVehicles\n{\n\tclass Car\n\t{\n\t\tmass = 10;\n\t};\n};\n";
const CONFIG_NEW: &str = "class CfgVehicles\n{\n\tclass Car\n\t{\n\t\tmass = 12;\n\t};\n};\n";
const LAYOUT_OLD: &str = "FrameWidgetClass Root {\n position 0 0\n size 100 100\n {\n  TextWidgetClass Label {\n   text \"Hi\"\n  }\n }\n}\n";
const LAYOUT_NEW: &str = "FrameWidgetClass Root {\n position 0 0\n size 100 100\n}\n";

fn snapshot(build: &str) -> Snapshot {
    let scripts = script_model(&[("scripts/3_Game/a.c".to_owned(), SCRIPT_NEW.to_owned())]);
    let parsed = modfoundry_config::parser::parse(CONFIG_NEW);
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let configs = config_model(&[("scripts/config.cpp".to_owned(), parsed.file)]);
    let layouts = layout_model(&[("gui/layouts/a.layout".to_owned(), LAYOUT_NEW.to_owned())]);
    Snapshot::new(build.to_owned(), scripts, configs, layouts)
}

#[test]
fn two_folders_give_all_three_diffs() {
    let dir = tmp("game-diff-two-folders");
    let old = dir.join("old");
    let new = dir.join("new");
    write(&old, "scripts/3_Game/a.c", SCRIPT_OLD);
    write(&new, "scripts/3_Game/a.c", SCRIPT_NEW);
    write(&old, "scripts/config.cpp", CONFIG_OLD);
    write(&new, "scripts/config.cpp", CONFIG_NEW);
    write(&old, "gui/layouts/a.layout", LAYOUT_OLD);
    write(&new, "gui/layouts/a.layout", LAYOUT_NEW);
    let request = GameDiffRequest {
        old: old.to_string_lossy().into_owned(),
        new: new.to_string_lossy().into_owned(),
        against: Vec::new(),
        snapshots: Some(dir.join("snapshots")),
    };

    let report = game_diff(&request).unwrap();

    assert_eq!(report.old.kind, "folder");
    assert_eq!(report.new.kind, "folder");
    assert_eq!(report.old.build, None);
    assert_eq!(
        report.scripts.classes_changed.len(),
        1,
        "{:?}",
        report.scripts
    );
    assert_eq!(
        report.scripts.classes_changed[0].methods.removed[0].name,
        "G"
    );
    assert_eq!(report.configs.changed.len(), 1, "{:?}", report.configs);
    assert_eq!(
        report.configs.changed[0].properties[0].old.as_deref(),
        Some("10")
    );
    assert_eq!(report.layouts.changed.len(), 1, "{:?}", report.layouts);
    assert_eq!(
        report.layouts.changed[0].widgets_removed[0].path,
        "Root/Label"
    );
    assert_eq!(
        report.summary,
        Summary {
            scripts: 1,
            configs: 1,
            layouts: 1,
            breaks: 0,
            check: 0,
        }
    );
}

#[test]
fn a_saved_build_is_a_side() {
    let dir = tmp("game-diff-saved-build");
    let snapshots = dir.join("snapshots");
    save_snapshot(&snapshots, &snapshot("1.0.1")).unwrap();
    let new = dir.join("new");
    write(&new, "scripts/3_Game/a.c", SCRIPT_NEW);
    write(&new, "scripts/config.cpp", CONFIG_NEW);
    write(&new, "gui/layouts/a.layout", LAYOUT_NEW);
    let new = new.to_string_lossy().into_owned();

    let report = game_diff(&GameDiffRequest {
        old: "1.0.1".to_owned(),
        new: new.clone(),
        against: Vec::new(),
        snapshots: Some(snapshots.clone()),
    })
    .unwrap();
    assert_eq!(report.old.kind, "snapshot");
    assert_eq!(report.old.build.as_deref(), Some("1.0.1"));
    assert_eq!(report.summary, Summary::default());

    let error = game_diff(&GameDiffRequest {
        old: "9.9.9".to_owned(),
        new,
        against: Vec::new(),
        snapshots: Some(snapshots),
    })
    .unwrap_err();
    assert!(error.starts_with("old side:"), "{error}");
}

const MOD_SCRIPT: &str = "class Patched extends Car\n{\n\tvoid F() {}\n}\n";
const MOD_CONFIG: &str = "class CfgPatches\n{\n\tclass Patch\n\t{\n\t\tunits[] = {};\n\t};\n};\n";

#[test]
fn mods_are_read_from_sources_or_pbos() {
    let dir = tmp("game-diff-mods");
    let source = dir.join("SourceMod");
    write(&source, "scripts/3_Game/m.c", MOD_SCRIPT);
    write(&source, "config.cpp", MOD_CONFIG);

    let (input, info) = read_mod(&source).unwrap();
    assert_eq!(input.name, "SourceMod");
    assert_eq!(input.scripts.len(), 1);
    assert_eq!(input.scripts[0].0, "scripts/3_Game/m.c");
    assert_eq!(input.configs.len(), 1);
    assert_eq!(input.configs[0].path, "config.cpp");
    assert_eq!(
        input.configs[0].text.as_deref(),
        Some(MOD_CONFIG),
        "a config.cpp keeps its text for line numbers"
    );
    assert_eq!(info.name, "SourceMod");
    assert_eq!(info.path, source);
    assert_eq!((info.scripts, info.configs), (1, 1));

    let packed = dir.join("@Packed");
    pack(
        &packed.join("addons/m.pbo"),
        &[],
        &[
            ("3_Game\\m.c", MOD_SCRIPT.as_bytes()),
            ("config.bin", config_bin(MOD_CONFIG).as_slice()),
        ],
    );
    let (input, info) = read_mod(&packed).unwrap();
    assert_eq!(input.scripts.len(), 1);
    assert!(input.scripts[0].0.ends_with("3_Game/m.c"));
    assert_eq!(input.configs.len(), 1);
    assert_eq!(input.configs[0].text, None, "a config.bin has no text");
    assert_eq!(info.name, "@Packed");

    let both = dir.join("Both");
    write(&both, "scripts/3_Game/loose.c", MOD_SCRIPT);
    pack(
        &both.join("addons/m.pbo"),
        &[],
        &[("3_Game\\packed.c", b"class Packed {};\n")],
    );
    let (input, _) = read_mod(&both).unwrap();
    assert_eq!(input.scripts.len(), 1);
    assert!(input.scripts[0].0.ends_with("loose.c"));

    let empty = dir.join("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        read_mod(&empty).unwrap_err(),
        format!("no scripts or configs under {}", empty.display())
    );
    let file = write(&dir, "notes.txt", "x");
    assert_eq!(
        read_mod(&file).unwrap_err(),
        format!("{} is not a folder", file.display())
    );
}

#[test]
fn an_unpacked_folder_with_a_prefix_marker_matches_its_pbo() {
    let dir = tmp("game-diff-prefix-marker");
    let folder = dir.join("unpacked");
    write(&folder, "a/config.cpp", MOD_CONFIG);
    write(&folder, "a/Scripts/3_Game/x.c", MOD_SCRIPT);
    write(&folder, "a/gui/layouts/x.layout", LAYOUT_NEW);
    write(&folder, "a/$PBOPREFIX$", "My\\Mod");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[
            ("config.bin", config_bin(MOD_CONFIG).as_slice()),
            ("Scripts\\3_Game\\x.c", MOD_SCRIPT.as_bytes()),
            ("gui\\layouts\\x.layout", LAYOUT_NEW.as_bytes()),
        ],
    );
    let request = GameDiffRequest {
        old: folder.to_string_lossy().into_owned(),
        new: pbo.to_string_lossy().into_owned(),
        against: Vec::new(),
        snapshots: Some(dir.join("snapshots")),
    };

    let report = game_diff(&request).unwrap();

    assert_eq!(
        (
            report.summary.scripts,
            report.summary.configs,
            report.summary.layouts
        ),
        (0, 0, 0),
        "{report:?}"
    );
}

#[test]
fn an_unpacked_folder_with_its_config_bin_and_config_cpp_matches_its_pbo() {
    let dir = tmp("game-diff-bin-and-cpp");
    let folder = dir.join("unpacked");
    let text = "class CfgPatches { class P { units[] = {}; }; };\nclass CfgAmmo { class B { hit[] = {\"\", 0.0125892535, 1}; minY = -0.0; }; };\n";
    let bin = config_bin(text);
    let cpp = modfoundry_config::to_text(&modfoundry_config::read_rap(&bin).unwrap());
    write(&folder, "a/config.cpp", &cpp);
    std::fs::write(folder.join("a/config.bin"), &bin).unwrap();
    write(&folder, "a/$PBOPREFIX$", "My\\Mod");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.bin", bin.as_slice())],
    );
    let request = GameDiffRequest {
        old: folder.to_string_lossy().into_owned(),
        new: pbo.to_string_lossy().into_owned(),
        against: Vec::new(),
        snapshots: Some(dir.join("snapshots")),
    };

    let report = game_diff(&request).unwrap();

    assert_eq!(report.summary.configs, 0, "{}", render_text(&report));
}

const DIAG_SCRIPT: &str =
    "class A\n{\n\tvoid F() {}\n\n#ifdef DIAG_DEVELOPER\n\tvoid G() {}\n#endif\n}\n";
const DIAG_SCRIPT_NEW: &str = "class A\n{\n}\n";

fn side(arg: &str, kind: &str, build: Option<&str>) -> SideInfo {
    SideInfo {
        arg: arg.to_owned(),
        kind: kind.to_owned(),
        build: build.map(str::to_owned),
        warnings: Vec::new(),
    }
}

#[test]
fn render_text_lists_diag_only_items_last() {
    let before = script_model(&[("scripts/3_Game/a.c".to_owned(), DIAG_SCRIPT.to_owned())]);
    let after = script_model(&[("scripts/3_Game/a.c".to_owned(), DIAG_SCRIPT_NEW.to_owned())]);
    let report = GameDiffReport {
        old: side("1.0.1", "snapshot", Some("1.0.1")),
        new: side("/game", "install", None),
        scripts: script_diff(&before, &after),
        configs: ConfigDiff::default(),
        layouts: LayoutDiff::default(),
        mods: Vec::new(),
        hits: vec![Hit {
            level: Level::Breaks,
            kind: HitKind::ModdedClassRemoved,
            maybe: false,
            mod_name: "SampleMods".to_owned(),
            place: Place {
                file: "scripts/4_World/p.c".to_owned(),
                line: 7,
            },
            vanilla: "A".to_owned(),
            detail: "modded class A has no vanilla class".to_owned(),
            diag_only: true,
        }],
        summary: Summary {
            scripts: 2,
            configs: 0,
            layouts: 0,
            breaks: 1,
            check: 0,
        },
    };

    let lines: Vec<String> = render_text(&report).lines().map(str::to_owned).collect();
    let at = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} line in {lines:?}"))
    };

    let plain = at("method removed  F");
    let diag = at("Diag only");
    let diag_member = at("method removed  G");
    let hit = at("Hits in SampleMods");
    let last = at("2 script, 0 config and 0 layout changes; 1 breaks, 0 to check");

    assert!(plain < diag, "{lines:?}");
    assert!(
        diag < diag_member,
        "a Diag-only item goes under its section: {lines:?}"
    );
    assert!(diag_member < hit, "{lines:?}");
    assert!(hit < last, "the counts are the last line: {lines:?}");
    assert_eq!(last, lines.len() - 1);
    assert_eq!(
        lines[diag_member], "  Scripts  class A  method removed  G  scripts/3_Game/a.c:6",
        "{lines:?}"
    );
    assert!(
        lines[hit + 1].starts_with("  breaks  scripts/4_World/p.c:7  modded class removed  A: "),
        "{lines:?}"
    );
}

#[test]
fn a_mod_config_with_a_parse_issue_is_kept_and_warned() {
    let dir = tmp("game-diff-mod-issue");
    let partial = dir.join("Partial");
    write(
        &partial,
        "config.cpp",
        "class CfgVehicles\n{\n\tclass Mine: Car\n\t{\n\t\tunits[] = {}\n\t};\n};\n",
    );

    let (input, info) = read_mod(&partial).unwrap();

    assert_eq!(input.configs.len(), 1);
    assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
    assert!(
        info.warnings[0].contains("config.cpp"),
        "{:?}",
        info.warnings
    );

    let broken = dir.join("Broken");
    write(&broken, "config.bin", "not a binarized config");
    let error = read_mod(&broken).unwrap_err();
    assert!(error.contains("no scripts or configs under"), "{error}");
    assert!(
        error.contains("config.bin"),
        "the warning is in the error: {error}"
    );
}

#[test]
fn build_output_inside_a_mod_is_not_read() {
    let project = tmp("game-diff-mod-builds").join("Project");
    write(&project, "scripts/3_Game/m.c", MOD_SCRIPT);
    write(
        &project,
        "builds/dev/@Project/scripts/3_Game/m.c",
        MOD_SCRIPT,
    );

    let (input, _) = read_mod(&project).unwrap();

    let paths: Vec<&str> = input
        .scripts
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(paths, ["scripts/3_Game/m.c"]);
}

#[test]
fn diag_only_lines_name_their_class_and_item() {
    let before = script_model(&[(
        "scripts/3_Game/a.c".to_owned(),
        "class K\n{\n#ifdef DIAG_DEVELOPER\n\tint f = 1;\n#endif\n}\n".to_owned(),
    )]);
    let after = script_model(&[(
        "scripts/3_Game/a.c".to_owned(),
        "class K\n{\n#ifdef DIAG_DEVELOPER\n\tint f = 2;\n#endif\n}\n".to_owned(),
    )]);
    let report = GameDiffReport {
        old: side("old", "folder", None),
        new: side("new", "folder", None),
        scripts: script_diff(&before, &after),
        configs: ConfigDiff::default(),
        layouts: LayoutDiff::default(),
        mods: Vec::new(),
        hits: vec![Hit {
            level: Level::Check,
            kind: HitKind::OverrideBody,
            maybe: false,
            mod_name: "M".to_owned(),
            place: Place {
                file: "p.c".to_owned(),
                line: 3,
            },
            vanilla: "K.F()".to_owned(),
            detail: "overrides K.F(), whose body changed".to_owned(),
            diag_only: true,
        }],
        summary: Summary::default(),
    };

    let text = render_text(&report);
    let lines: Vec<&str> = text.lines().collect();

    assert!(
        !lines.contains(&"Scripts"),
        "no plain section for Diag-only changes: {lines:?}"
    );
    let diag = lines.iter().position(|line| *line == "Diag only").unwrap();
    assert_eq!(
        lines[diag + 1..diag + 3],
        [
            "  Scripts  class K  field changed  f  scripts/3_Game/a.c:4",
            "  Scripts  class K  f  value: 1 -> 2",
        ],
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with("whose body changed (Diag only)")),
        "{lines:?}"
    );
}

#[test]
fn every_mod_is_checked_and_named_in_the_report() {
    let dir = tmp("game-diff-two-mods");
    let old = dir.join("old");
    let new = dir.join("new");
    write(&old, "scripts/3_Game/a.c", SCRIPT_OLD);
    write(&new, "scripts/3_Game/a.c", SCRIPT_NEW);
    let hit = dir.join("HitMod");
    write(
        &hit,
        "scripts/4_World/m.c",
        "modded class A\n{\n\toverride void G() {}\n}\n",
    );
    let clean = dir.join("CleanMod");
    write(
        &clean,
        "scripts/4_World/m.c",
        "modded class A\n{\n\toverride void F() {}\n}\n",
    );
    let request = GameDiffRequest {
        old: old.to_string_lossy().into_owned(),
        new: new.to_string_lossy().into_owned(),
        against: vec![hit, clean],
        snapshots: Some(dir.join("snapshots")),
    };

    let report = game_diff(&request).unwrap();

    assert_eq!(report.mods.len(), 2);
    let hits: Vec<(&str, HitKind)> = report
        .hits
        .iter()
        .map(|hit| (hit.mod_name.as_str(), hit.kind))
        .collect();
    assert_eq!(hits, [("HitMod", HitKind::OverrideRemoved)]);
    assert_eq!(report.summary.breaks, 1);
    let text = render_text(&report);
    assert!(text.contains("Hits in HitMod"), "{text}");
    assert!(
        text.contains("No hits in CleanMod (1 scripts, 0 configs)"),
        "{text}"
    );
}

#[test]
fn without_a_snapshots_folder_only_snapshot_files_are_snapshots() {
    let dir = tmp("game-diff-no-snapshots");
    let saved = save_snapshot(&dir.join("saved"), &snapshot("1.0.1")).unwrap();
    let new = dir.join("new");
    write(&new, "scripts/3_Game/a.c", SCRIPT_NEW);
    let request = |old: &str| GameDiffRequest {
        old: old.to_owned(),
        new: new.to_string_lossy().into_owned(),
        against: Vec::new(),
        snapshots: None,
    };

    let report = game_diff(&request(&saved.to_string_lossy())).unwrap();
    assert_eq!(report.old.kind, "snapshot");

    let error = game_diff(&request("1.0.1")).unwrap_err();
    assert_eq!(error, "old side: 1.0.1 does not exist");
}

const PATCHING_CONFIG: &str = "class CfgVehicles\n{\n\tclass Car\n\t{\n\t\tmass = 12;\n\t};\n};\n";

#[test]
fn a_mod_config_with_a_byte_order_mark_still_gets_hits() {
    let dir = tmp("game-diff-mod-bom");
    let old = dir.join("old");
    let new = dir.join("new");
    write(&old, "scripts/config.cpp", CONFIG_OLD);
    write(&new, "scripts/config.cpp", CONFIG_NEW);
    let plain = dir.join("Plain");
    write(&plain, "config.cpp", PATCHING_CONFIG);
    let marked = dir.join("Marked");
    write(&marked, "config.cpp", &format!("\u{feff}{PATCHING_CONFIG}"));

    let report = game_diff(&GameDiffRequest {
        old: old.to_string_lossy().into_owned(),
        new: new.to_string_lossy().into_owned(),
        against: vec![plain, marked],
        snapshots: Some(dir.join("snapshots")),
    })
    .unwrap();

    let hits: Vec<(&str, HitKind)> = report
        .hits
        .iter()
        .map(|hit| (hit.mod_name.as_str(), hit.kind))
        .collect();
    assert_eq!(
        hits,
        [
            ("Marked", HitKind::ConfigPatchChanged),
            ("Plain", HitKind::ConfigPatchChanged),
        ]
    );
    assert!(
        report.mods.iter().all(|info| info.warnings.is_empty()),
        "{:?}",
        report.mods
    );
}

#[test]
fn two_mods_with_one_name_keep_separate_hits() {
    let dir = tmp("game-diff-same-name");
    let old = dir.join("old");
    let new = dir.join("new");
    write(&old, "scripts/3_Game/a.c", SCRIPT_OLD);
    write(&new, "scripts/3_Game/a.c", SCRIPT_NEW);
    let first = dir.join("a").join("@Mod");
    let second = dir.join("b").join("@Mod");
    for root in [&first, &second] {
        write(
            root,
            "scripts/4_World/m.c",
            "modded class A\n{\n\toverride void G() {}\n}\n",
        );
    }
    let first_arg = first.to_string_lossy().into_owned();
    let second_arg = second.to_string_lossy().into_owned();

    let report = game_diff(&GameDiffRequest {
        old: old.to_string_lossy().into_owned(),
        new: new.to_string_lossy().into_owned(),
        against: vec![first, second],
        snapshots: Some(dir.join("snapshots")),
    })
    .unwrap();

    let names: Vec<String> = report.mods.iter().map(|info| info.name.clone()).collect();
    assert_eq!(
        names,
        [
            format!("@Mod ({first_arg})"),
            format!("@Mod ({second_arg})"),
        ]
    );
    let hits: Vec<(&str, HitKind)> = report
        .hits
        .iter()
        .map(|hit| (hit.mod_name.as_str(), hit.kind))
        .collect();
    assert_eq!(
        hits,
        [
            (names[0].as_str(), HitKind::OverrideRemoved),
            (names[1].as_str(), HitKind::OverrideRemoved),
        ]
    );
    let text = render_text(&report);
    assert_eq!(text.matches("Hits in @Mod (").count(), 2, "{text}");
}

#[test]
fn a_mod_source_with_a_prefix_marker_keeps_its_paths_on_disk() {
    let dir = tmp("game-diff-mod-marker");
    let source = dir.join("SourceMod");
    write(&source, "scripts/3_Game/m.c", MOD_SCRIPT);
    write(&source, "config.cpp", MOD_CONFIG);
    write(&source, "$PBOPREFIX$", "My\\Mod");

    let (input, _) = read_mod(&source).unwrap();

    assert_eq!(input.scripts[0].0, "scripts/3_Game/m.c");
    assert_eq!(input.configs[0].path, "config.cpp");
}
