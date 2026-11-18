//! The layout check: the game's image sets and styles, the rules over a mod's layouts, the cache.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Instant;

use modfoundry_commands::layout_check::{check_layouts, image_set, LayoutCheckRequest};
use modfoundry_layout::styles::style_names;

use std::path::{Path, PathBuf};

use crate::common::{pack, tmp, write};
use modfoundry_pbo::PboArchive;

#[test]
fn image_set_reads_the_set_name_and_every_image() {
    let text = r#"ImageSetClass {
 Name "ccgui_enforce"
 RefSize 1024 1024
 Textures {
  ImageSetTextureClass {
   mpix 1
   path "{52DD0A1B2C3D4E5F}Gui/imagesets/ccgui_enforce.edds"
  }
 }
 Images {
  ImageSetDefClass ButtonMenuDefaultLeftTop {
   Name "ButtonMenuDefaultLeftTop"
   Pos 598 683
   Size 10 10
  }
  ImageSetDefClass ButtonMenuDefaultRight {
   Name "ButtonMenuDefaultRight"
   Pos 620 683
   Size 10 10
  }
 }
}
"#;
    let (name, images) = image_set(text).unwrap();
    assert_eq!(name, "ccgui_enforce");
    assert_eq!(
        images,
        BTreeSet::from([
            "ButtonMenuDefaultLeftTop".to_string(),
            "ButtonMenuDefaultRight".to_string(),
        ])
    );
}

#[test]
fn image_set_without_a_name_is_an_error() {
    let text = "ImageSetClass {\n RefSize 2 2\n Images {\n  ImageSetDefClass Lone {\n   Pos 0 0\n   Size 1 1\n  }\n }\n}\n";
    let err = image_set(text).unwrap_err();
    assert!(err.message.contains("no Name"), "{err}");
}

#[test]
fn image_set_without_a_ref_size_is_refused_with_its_line() {
    let text = "ImageSetClass {\n Name \"s\"\n}\n";
    assert!(image_set(text).is_err());
}

#[test]
fn style_names_lists_every_style_in_a_styles_file() {
    let text = r#"<Widgets>
  <Widget Name="Root">
    <Style Name="DayZDefaultPanel" Disabled="0"/>
    <Style Name="DayZDefaultButton"/>
  </Widget>
</Widgets>
"#;
    assert_eq!(
        style_names(text),
        BTreeSet::from([
            "DayZDefaultButton".to_string(),
            "DayZDefaultPanel".to_string(),
        ])
    );
}

#[test]
fn check_refuses_a_path_that_does_not_exist() {
    let dir = tmp("layout-missing-path");
    let req = LayoutCheckRequest {
        path: dir.join("nope"),
        root: None,
        game: dir.join("game"),
        dependencies: Vec::new(),
    };
    let err = check_layouts(&req, &dir.join("cache")).unwrap_err();
    assert!(err.contains("does not exist"), "{err}");
}

#[test]
fn check_refuses_a_game_path_that_is_not_a_folder() {
    let dir = tmp("layout-game-file");
    write(&dir, "game.txt", "not an install");
    write(&dir, "mod/a.layout", "");
    let req = LayoutCheckRequest {
        path: dir.join("mod"),
        root: None,
        game: dir.join("game.txt"),
        dependencies: Vec::new(),
    };
    let err = check_layouts(&req, &dir.join("cache")).unwrap_err();
    assert!(err.contains("not a folder"), "{err}");
}

const GAME_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 size 100 100
 {
  TextWidgetClass Label {
   text "Hello"
   font "gui/fonts/f"
   style "DayZDefaultPanel"
   imageTexture "{5A89D58DD2276E85}gui/textures/t.edds"
   image0 "set:x image:img"
  }
 }
}
"#;

const GAME_IMAGESET: &str = r#"ImageSetClass {
 Name "x"
 RefSize 2 2
 Textures {
  ImageSetTextureClass {
   mpix 1
   path "{52DD0A1B2C3D4E5F}gui/textures/t.edds"
  }
 }
 Images {
  ImageSetDefClass img {
   Name "img"
   Pos 0 0
   Size 1 1
  }
 }
}
"#;

const GAME_STYLES: &str = r#"<Widgets>
  <Widget Name="Root">
    <Style Name="DayZDefaultPanel"/>
  </Widget>
</Widgets>
"#;

/// A game folder with one GUI PBO and one script PBO.
fn fake_game(dir: &Path) -> PathBuf {
    let game = dir.join("game");
    pack(
        &game.join("dta/gui.pbo"),
        &[("prefix", "gui")],
        &[
            ("layouts/a.layout", GAME_LAYOUT.as_bytes()),
            ("imagesets/x.imageset", GAME_IMAGESET.as_bytes()),
            ("looknfeel/x.styles", GAME_STYLES.as_bytes()),
            ("fonts/f.fnt", b"font"),
            ("fonts/family.xml", b"<fontGenerator/>"),
            ("textures/t.edds", b"edds"),
        ],
    );
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[(
            "widgets.c",
            b"class Widget {}\nclass RenderTargetWidget: Widget {}\n",
        )],
    );
    game
}

fn request(path: PathBuf, game: PathBuf) -> LayoutCheckRequest {
    LayoutCheckRequest {
        path,
        root: None,
        game,
        dependencies: Vec::new(),
    }
}

const CLEAN_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 size 100 100
 {
  TextWidgetClass Label {
   text "Hello"
   font "gui/fonts/f"
   style "DayZDefaultPanel"
   imageTexture "{5A89D58DD2276E85}gui/textures/t.edds"
   image0 "set:x image:img"
  }
 }
}
"#;

#[test]
fn a_clean_layout_against_the_game_has_no_findings() {
    let dir = tmp("layout-clean");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    assert_eq!(report.files, 1);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

const BROKEN_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 size 100 100
 {
  TextWidgetClas Label {
   text "Hello"
  }
  TextWidgetClass Other {
   colour 1 1 1 1
   font "gui/fonts/missing"
   style "NoSuchPanel"
   imageTexture "{5A89D58DD2276E85}gui/textures/missing.edds"
   image0 "set:x image:nope"
  }
 }
}
"#;

#[test]
fn each_broken_reference_is_reported_once_at_its_line() {
    let dir = tmp("layout-broken");
    let game = fake_game(&dir);
    write(&dir, "mod/broken.layout", BROKEN_LAYOUT);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    let found: Vec<(String, usize)> = report
        .findings
        .iter()
        .map(|finding| {
            let span = finding.diagnostic.span.unwrap();
            let line = BROKEN_LAYOUT[..span.start].matches('\n').count() + 1;
            (finding.diagnostic.code.clone(), line)
        })
        .collect();
    let expected = [
        ("MF-L102", 5),
        ("MF-L103", 9),
        ("MF-L106", 10),
        ("MF-L106", 11),
        ("MF-L106", 12),
        ("MF-L106", 13),
    ];
    let expected: Vec<(String, usize)> = expected
        .iter()
        .map(|(code, line)| (code.to_string(), *line))
        .collect();
    assert_eq!(found, expected);
}

const MOD_IMAGESET: &str = r#"ImageSetClass {
 Name "mine"
 RefSize 2 2
 Images {
  ImageSetDefClass pic {
   Name "pic"
   Pos 0 0
   Size 1 1
  }
 }
}
"#;

const MOD_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 {
  RenderTargetWidgetClass Target {
  }
  TextWidgetClass Label {
   imageTexture "{5A89D58DD2276E85}MyMod/gui/t2.edds"
   image0 "set:mine image:pic"
  }
 }
}
"#;

#[test]
fn a_mod_layout_resolves_its_own_files_and_script_widgets() {
    let dir = tmp("layout-mod-files");
    let game = fake_game(&dir);
    write(&dir, "MyMod/config.cpp", "class CfgPatches {};\n");
    write(&dir, "MyMod/gui/t2.edds", "edds");
    write(&dir, "MyMod/gui/imagesets/mine.imageset", MOD_IMAGESET);
    write(&dir, "MyMod/layouts/mine.layout", MOD_LAYOUT);
    let report = check_layouts(
        &request(dir.join("MyMod/layouts"), game),
        &dir.join("cache"),
    )
    .unwrap();
    assert_eq!(report.files, 1);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_mod_imageset_the_strict_reader_refuses_is_a_warning_naming_the_file() {
    let dir = tmp("layout-mod-bad-imageset");
    let game = fake_game(&dir);
    write(&dir, "MyMod/config.cpp", "class CfgPatches {};\n");
    write(&dir, "MyMod/gui/t2.edds", "edds");
    write(
        &dir,
        "MyMod/gui/imagesets/mine.imageset",
        "ImageSetClass {\n Name \"mine\"\n}\n",
    );
    write(&dir, "MyMod/layouts/mine.layout", MOD_LAYOUT);
    let report = check_layouts(
        &request(dir.join("MyMod/layouts"), game),
        &dir.join("cache"),
    )
    .unwrap();
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.contains("mine.imageset"))
        .unwrap_or_else(|| panic!("{:?}", report.warnings));
    assert!(warning.contains("RefSize"), "{warning}");
}

#[test]
fn the_game_layouts_are_cached_and_a_corrupt_cache_is_rebuilt() {
    let dir = tmp("layout-cache");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let cache = dir.join("cache");
    let run = || check_layouts(&request(dir.join("mod"), game.clone()), &cache).unwrap();

    let first = run();
    assert!(!first.cached);
    assert!(
        first.cache.ends_with("layout-game.json"),
        "{}",
        first.cache.display()
    );
    assert!(first.cache.starts_with(&cache) && first.cache.is_file());

    assert!(run().cached);

    std::fs::write(&first.cache, b"{ not json").unwrap();
    let rebuilt = run();
    assert!(!rebuilt.cached);
    assert!(rebuilt.findings.is_empty(), "{:?}", rebuilt.findings);
    assert!(run().cached);
}

#[test]
#[ignore]
fn vanilla_gui_layouts_against_the_install() {
    let Some(install) = std::env::var_os("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL is not set");
        return;
    };
    let install = PathBuf::from(install);
    let dir = tmp("layout-vanilla");
    let archive = PboArchive::open(&install.join("dta/gui.pbo")).unwrap();
    let mut copied = 0;
    for (index, entry) in archive.entries().iter().enumerate() {
        let name = entry.name_str().replace('\\', "/");
        if Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("layout"))
        {
            let target = dir.join("layouts").join(&name);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, archive.read_entry(index).unwrap()).unwrap();
            copied += 1;
        }
    }
    let started = Instant::now();
    let report = check_layouts(&request(dir.join("layouts"), install), &dir.join("cache")).unwrap();
    println!("files {} in {:?}", report.files, started.elapsed());
    let mut per_code: BTreeMap<&str, usize> = BTreeMap::new();
    for finding in &report.findings {
        *per_code
            .entry(finding.diagnostic.code.as_str())
            .or_default() += 1;
    }
    println!("findings per code {per_code:?}");
    for finding in report
        .findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-L106")
        .take(10)
    {
        println!(
            "MF-L106 {}: {}",
            finding.path.display(),
            finding.diagnostic.message
        );
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    assert_eq!(report.files, copied);
    assert!(
        report
            .warnings
            .iter()
            .all(|warning| !warning.contains("nested deeper")),
        "{:?}",
        report.warnings
    );
    let unparsed: Vec<_> = report
        .findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-L101")
        .collect();
    assert!(unparsed.is_empty(), "{unparsed:#?}");
}

#[test]
fn a_deep_mod_tree_is_read_and_a_deeper_one_is_warned_about() {
    let dir = tmp("layout-depth");
    let game = fake_game(&dir);
    let deep = "mod/a/b/c/d/e/f/g/h/i/j";
    write(
        &dir,
        &format!("{deep}/deep.layout"),
        "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/textures/missing.edds\"\n}\n",
    );
    let deeper: String = (1..=26).map(|n| format!("f{n}/")).collect();
    write(&dir, &format!("mod/{deeper}too_deep.layout"), CLEAN_LAYOUT);

    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    assert_eq!(report.files, 1);
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.diagnostic.code.eq("MF-L106")),
        "{:?}",
        report.findings
    );
    let nested: Vec<&String> = report
        .warnings
        .iter()
        .filter(|warning| warning.contains("nested deeper"))
        .collect();
    assert_eq!(nested.len(), 1, "{:?}", report.warnings);
    assert!(nested[0].contains("f24"), "{}", nested[0]);
    assert!(nested[0].contains("1 folder(s)"), "{}", nested[0]);
}

#[test]
fn a_tree_with_layouts_only_past_the_depth_cap_says_so() {
    let dir = tmp("layout-depth-only");
    let game = fake_game(&dir);
    let deeper: String = (1..=26).map(|n| format!("f{n}/")).collect();
    write(&dir, &format!("mod/{deeper}too_deep.layout"), CLEAN_LAYOUT);
    let err = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap_err();
    assert!(err.contains("no .layout files"), "{err}");
    assert!(err.contains("nested deeper"), "{err}");
}

#[test]
fn a_plain_tree_gets_no_nesting_warning() {
    let dir = tmp("layout-depth-plain");
    let game = fake_game(&dir);
    write(&dir, "mod/a/b/clean.layout", CLEAN_LAYOUT);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .all(|warning| !warning.contains("nested deeper")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn style_names_reads_real_syntax() {
    let cases: [(&str, &str, &[&str]); 7] = [
        ("single quotes", "<Style Name='A'/>\n", &["A"]),
        ("empty name", "<Style Name=\"\"/>\n", &[]),
        ("blanks around equals", "<Style Name = \"A\"/>\n", &["A"]),
        ("sub style", "<SubStyle Name=\"B\"/>\n", &[]),
        ("attribute ending in Name", "<Style FontName=\"C\"/>\n", &[]),
        (
            "comment",
            "<!-- <Style Name=\"D\"/> -->\n<Style Name=\"E\"/>\n",
            &["E"],
        ),
        (
            "name after other attributes",
            "<Style Disabled=\"0\" Name=\"F\" ImageSet=\"\"/>\n",
            &["F"],
        ),
    ];
    for (label, text, expected) in cases {
        let expected: BTreeSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(style_names(text), expected, "{label}");
    }
}

#[test]
fn an_attribute_value_that_holds_a_closing_bracket_stays_quoted() {
    let text = "<Style Name=\"A\" Tooltip=\"a > b\"/>\n<Style Name=\"B\"/>\n";
    assert_eq!(
        style_names(text),
        BTreeSet::from(["A".to_string(), "B".to_string()])
    );
}

#[test]
fn the_loose_file_suffix_rule() {
    let cases = [
        ("gui/t.edds", "gui/t.edds", true),
        ("gui/t.edds", "MyMod/gui/t.edds", true),
        ("gui/xt.edds", "gui/t.edds", false),
        ("xt.edds", "t.edds", false),
        ("t.edds", "xt.edds", false),
    ];
    for (i, (loose, reference, resolves)) in cases.iter().enumerate() {
        let dir = tmp(&format!("layout-suffix-{i}"));
        let game = fake_game(&dir);
        write(&dir, &format!("mod/{loose}"), "edds");
        let layout = format!(
            "TextWidgetClass Root {{\n imageTexture \"{{5A89D58DD2276E85}}{reference}\"\n}}\n"
        );
        write(&dir, "mod/layouts/mine.layout", &layout);
        let mut req = request(dir.join("mod/layouts"), game);
        req.root = Some(dir.join("mod"));
        let report = check_layouts(&req, &dir.join("cache")).unwrap();
        let unresolved = report
            .findings
            .iter()
            .any(|finding| finding.diagnostic.code == "MF-L106");
        assert_eq!(unresolved, !resolves, "{loose} / {reference}");
    }
}

#[test]
fn findings_from_a_cached_run_match_a_cold_run() {
    let dir = tmp("layout-cached-findings");
    let game = fake_game(&dir);
    write(&dir, "mod/broken.layout", BROKEN_LAYOUT);
    let cache = dir.join("cache");
    let run = || check_layouts(&request(dir.join("mod"), game.clone()), &cache).unwrap();
    let cold = run();
    assert!(!cold.cached);
    let warm = run();
    assert!(warm.cached);
    let summarise = |report: &modfoundry_commands::layout_check::LayoutCheckReport| {
        report
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.diagnostic.code.clone(),
                    finding.diagnostic.message.clone(),
                    finding.diagnostic.span.map(|span| span.start),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(summarise(&cold), summarise(&warm));
}

#[test]
fn an_explicit_root_supplies_the_files() {
    let dir = tmp("layout-explicit-root");
    let game = fake_game(&dir);
    write(&dir, "assets/gui/t2.edds", "edds");
    write(
        &dir,
        "mod/mine.layout",
        "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/t2.edds\"\n}\n",
    );
    let without =
        check_layouts(&request(dir.join("mod"), game.clone()), &dir.join("cache")).unwrap();
    assert!(
        without
            .findings
            .iter()
            .any(|finding| finding.diagnostic.code == "MF-L106"),
        "{:?}",
        without.findings
    );

    let mut req = request(dir.join("mod"), game);
    req.root = Some(dir.join("assets"));
    let with = check_layouts(&req, &dir.join("cache2")).unwrap();
    assert!(with.findings.is_empty(), "{:?}", with.findings);
}

#[test]
fn a_dependencys_loose_files_resolve_references() {
    let dir = tmp("layout-dependency-files");
    let game = fake_game(&dir);
    write(&dir, "Dep/gui/t3.edds", "edds");
    write(
        &dir,
        "Dep/gui/sets/dep.imageset",
        "ImageSetClass {\n Name \"dep\"\n RefSize 2 2\n Images {\n  ImageSetDefClass pic {\n   Name \"pic\"\n   Pos 0 0\n   Size 1 1\n  }\n }\n}\n",
    );
    write(
        &dir,
        "Dep/gui/looknfeel/dep.styles",
        "<Widgets>\n <Style Name=\"DepStyle\"/>\n</Widgets>\n",
    );
    let layout = "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/t3.edds\"\n image0 \"set:dep image:pic\"\n style DepStyle\n}\n";
    write(&dir, "mod/mine.layout", layout);

    let without =
        check_layouts(&request(dir.join("mod"), game.clone()), &dir.join("cache")).unwrap();
    let unresolved = |report: &modfoundry_commands::layout_check::LayoutCheckReport| {
        report
            .findings
            .iter()
            .filter(|finding| finding.diagnostic.code == "MF-L106")
            .count()
    };
    assert_eq!(unresolved(&without), 3, "{:?}", without.findings);

    let mut req = request(dir.join("mod"), game);
    req.dependencies = vec![dir.join("Dep")];
    let with = check_layouts(&req, &dir.join("cache2")).unwrap();
    assert_eq!(unresolved(&with), 0, "{:?}", with.findings);
}

#[test]
fn a_single_file_path_finds_its_mod_root() {
    let dir = tmp("layout-single-file");
    let game = fake_game(&dir);
    write(&dir, "MyMod/config.cpp", "class CfgPatches {};\n");
    write(&dir, "MyMod/gui/t4.edds", "edds");
    let file = write(
        &dir,
        "MyMod/layouts/mine.layout",
        "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/t4.edds\"\n}\n",
    );
    let report = check_layouts(&request(file, game), &dir.join("cache")).unwrap();
    assert_eq!(report.files, 1);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_tab_after_the_def_keyword_still_names_the_image() {
    let text = "ImageSetClass {\n Name \"tabbed\"\n RefSize 2 2\n Images {\n  ImageSetDefClass\tpic {\n   Name \"pic\"\n   Pos 0 0\n   Size 1 1\n  }\n }\n}\n";
    let (_, images) = image_set(text).unwrap();
    assert_eq!(images, BTreeSet::from(["pic".to_string()]));
}

#[test]
fn a_game_pbo_that_cannot_be_read_is_not_cached() {
    let dir = tmp("layout-cache-partial");
    let game = fake_game(&dir);
    write(&dir, "game/dta/broken.pbo", "not a pbo");
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    assert!(!report.warnings.is_empty());
    assert!(!report.cache.is_file(), "{}", report.cache.display());
}

#[test]
fn a_game_pbo_that_cannot_be_read_warns_once() {
    let dir = tmp("layout-cache-unreadable-pbo");
    let game = fake_game(&dir);
    write(&dir, "game/dta/broken.pbo", "not a pbo");
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    let matching: Vec<&String> = report
        .warnings
        .iter()
        .filter(|warning| warning.contains("broken.pbo"))
        .collect();
    assert_eq!(matching.len(), 1, "{:?}", report.warnings);
}

#[cfg(unix)]
#[test]
fn a_failed_cache_write_leaves_no_part_file() {
    let dir = tmp("layout-cache-part-fail");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let cache = dir.join("cache");
    let run = || check_layouts(&request(dir.join("mod"), game.clone()), &cache).unwrap();

    let first = run();
    fs::remove_file(&first.cache).unwrap();
    fs::create_dir_all(&first.cache).unwrap();

    let report = run();
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("layout cache not written")),
        "{:?}",
        report.warnings
    );
    let parts: Vec<_> = fs::read_dir(first.cache.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".part"))
        .collect();
    assert!(parts.is_empty(), "{parts:?}");
}

#[test]
fn the_cache_from_an_older_schema_is_rebuilt() {
    let dir = tmp("layout-cache-schema");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let cache = dir.join("cache");
    let first = check_layouts(&request(dir.join("mod"), game.clone()), &cache).unwrap();
    let text = fs::read_to_string(&first.cache).unwrap();
    fs::write(&first.cache, text.replace("\"schema\":2", "\"schema\":1")).unwrap();
    let rebuilt = check_layouts(&request(dir.join("mod"), game), &cache).unwrap();
    assert!(!rebuilt.cached);
}

#[test]
fn a_dependency_class_that_derives_from_a_game_widget_is_known() {
    let dir = tmp("layout-dependency-widget");
    let game = fake_game(&dir);
    pack(
        &dir.join("deps/Dep.pbo"),
        &[("prefix", "dep")],
        &[("widgets.c", b"class MyWidget: RenderTargetWidget {}\n")],
    );
    write(&dir, "mod/mine.layout", "MyWidgetClass Root {\n}\n");
    let mut req = request(dir.join("mod"), game);
    req.dependencies = vec![dir.join("deps/Dep.pbo")];
    let report = check_layouts(&req, &dir.join("cache")).unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_missing_dependency_is_a_warning_and_the_check_still_runs() {
    let dir = tmp("layout-missing-dependency");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let mut req = request(dir.join("mod"), game);
    req.dependencies = vec![dir.join("deps/Gone")];
    let report = check_layouts(&req, &dir.join("cache")).unwrap();
    assert!(
        report.warnings.iter().any(|w| w.contains("not found")),
        "{:?}",
        report.warnings
    );
    assert!(report.findings.is_empty());
}

#[test]
fn a_font_family_and_the_enfusion_alias_resolve_like_the_engine() {
    let dir = tmp("layout-engine-names");
    let game = fake_game(&dir);
    let layout = "FrameWidgetClass Root {\n {\n  TextWidgetClass Label {\n   font \"gui/fonts/family\"\n   imageTexture \"Enfusion/Gui/textures/t.edds\"\n  }\n }\n}\n";
    write(&dir, "mod/engine.layout", layout);
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache")).unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_root_that_is_not_a_folder_is_a_warning() {
    let dir = tmp("layout-bad-root");
    let game = fake_game(&dir);
    write(&dir, "mod/clean.layout", CLEAN_LAYOUT);
    let mut req = request(dir.join("mod"), game);
    req.root = Some(dir.join("typo"));
    let report = check_layouts(&req, &dir.join("cache")).unwrap();
    assert!(
        report.warnings.iter().any(|w| w.contains("typo")),
        "{:?}",
        report.warnings
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_layout_is_a_warning_and_the_others_are_still_checked() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("layout-unreadable");
    let game = fake_game(&dir);
    write(&dir, "mod/a.layout", BROKEN_LAYOUT);
    let locked = write(&dir, "mod/b.layout", CLEAN_LAYOUT);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let report = check_layouts(&request(dir.join("mod"), game), &dir.join("cache"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    let report = report.unwrap();
    assert!(!report.findings.is_empty());
    assert_eq!(report.files, 1);
    assert!(
        report.warnings.iter().any(|w| w.contains("b.layout")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn a_dependency_folder_with_a_prefix_marker_resolves_only_under_its_prefix() {
    let dir = tmp("layout-dependency-marker");
    let game = fake_game(&dir);
    write(&dir, "Dep/a/$PBOPREFIX$", "My\\Mod");
    write(&dir, "Dep/a/gui/t5.edds", "edds");
    let layout = "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}My/Mod/gui/t5.edds\"\n image0 \"{5A89D58DD2276E85}Wrong/gui/t5.edds\"\n}\n";
    write(&dir, "mod/mine.layout", layout);

    let mut req = request(dir.join("mod"), game);
    req.dependencies = vec![dir.join("Dep")];
    let report = check_layouts(&req, &dir.join("cache")).unwrap();

    let unresolved: Vec<&str> = report
        .findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-L106")
        .map(|finding| finding.diagnostic.message.as_str())
        .collect();
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert!(
        unresolved[0].contains("Wrong/gui/t5.edds"),
        "{unresolved:?}"
    );
}
