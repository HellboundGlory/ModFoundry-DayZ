//! Tests for the files `mf new` writes: the template filled with the mod's
//! options, and the generated config.cpp validating clean against its tree.

use std::path::{Path, PathBuf};

use modfoundry_config::scaffold::{mod_skeleton, FileToWrite, ScaffoldOptions};
use modfoundry_core::Severity;

fn paths(files: &[FileToWrite]) -> Vec<&str> {
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths
}

fn contents<'a>(files: &'a [FileToWrite], path: &str) -> &'a str {
    &files.iter().find(|f| f.path == path).expect(path).contents
}

fn write_tree(dir: &Path, files: &[FileToWrite]) {
    for file in files {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &file.contents).unwrap();
    }
}

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mf-scaffold-{label}-{}", std::process::id()))
}

#[test]
fn defaults_write_the_template_files() {
    let files = mod_skeleton(&ScaffoldOptions::new("MyMod")).expect("default options scaffold");
    assert_eq!(
        paths(&files),
        [
            ".gitignore",
            "Scripts/3_Game/.gitkeep",
            "Scripts/4_World/.gitkeep",
            "Scripts/5_Mission/MyModMission.c",
            "config.cpp",
            "mod.cpp",
        ],
        "{files:?}"
    );
}

#[test]
fn generated_config_validates_clean_against_its_own_tree() {
    let dir = temp_dir("validate");
    let files = mod_skeleton(&ScaffoldOptions::new("MyMod")).unwrap();
    write_tree(&dir, &files);
    let cfg_text = std::fs::read_to_string(dir.join("config.cpp")).unwrap();
    let v = modfoundry_config::validate_text(&cfg_text, Some(&dir));
    assert!(
        v.diagnostics.iter().all(|d| d.severity < Severity::Error),
        "{:?}",
        v.diagnostics
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn filled_options_render_mod_cpp() {
    let options = ScaffoldOptions {
        display_name: Some("Demo Mod".into()),
        description: "Line one \"quoted\"\nLine two".into(),
        author: "Me".into(),
        version: "1.2.3".into(),
        ..ScaffoldOptions::new("Demo")
    };
    let files = mod_skeleton(&options).expect("valid options scaffold");
    assert_eq!(
        contents(&files, "mod.cpp"),
        "name = \"Demo Mod\";\n\
         tooltip = \"Line one \"\"quoted\"\"\";\n\
         overview = \"Line one \"\"quoted\"\" Line two\";\n\
         author = \"Me\";\n\
         version = \"1.2.3\";\n"
    );
}

#[test]
fn a_prefix_and_module_subset_choose_the_files_paths_and_folders() {
    let options = ScaffoldOptions {
        prefix: Some("Pfx".into()),
        modules: vec!["5_Mission".into(), "3_Game".into()],
        ..ScaffoldOptions::new("Demo")
    };
    let files = mod_skeleton(&options).expect("valid options scaffold");
    assert_eq!(
        paths(&files),
        [
            ".gitignore",
            "Scripts/3_Game/.gitkeep",
            "Scripts/5_Mission/DemoMission.c",
            "config.cpp",
            "mod.cpp",
        ],
        "{files:?}"
    );
    let config = contents(&files, "config.cpp");
    let expected = concat!(
        "            class gameScriptModule\n",
        "            {\n",
        "                files[] = { \"Pfx/Scripts/3_Game\" };\n",
        "            };\n",
        "            class missionScriptModule\n",
    );
    assert!(config.contains(expected), "{config}");
    assert!(!config.contains("worldScriptModule"), "{config}");
}

#[test]
fn the_starter_needs_both_the_option_and_the_mission_module() {
    let options = ScaffoldOptions {
        modules: vec!["3_Game".into(), "5_Mission".into()],
        starter: false,
        ..ScaffoldOptions::new("Demo")
    };
    let files = mod_skeleton(&options).expect("valid options scaffold");
    assert!(paths(&files).contains(&"Scripts/5_Mission/.gitkeep"));
    assert!(!files.iter().any(|f| f.path.ends_with("Mission.c")));

    let options = ScaffoldOptions {
        modules: vec!["3_Game".into()],
        ..ScaffoldOptions::new("Demo")
    };
    let files = mod_skeleton(&options).expect("valid options scaffold");
    assert!(!files.iter().any(|f| f.path.contains("Mission")));
}

#[test]
fn unsafe_or_incomplete_options_are_rejected() {
    let cases = [
        (
            ScaffoldOptions::new("bad name/../evil"),
            "is not a safe mod name",
        ),
        (ScaffoldOptions::new("my-mod"), "is not a safe mod name"),
        (ScaffoldOptions::new("3Mod"), "is not a safe mod name"),
        (
            ScaffoldOptions {
                prefix: Some("JM//CF".into()),
                ..ScaffoldOptions::new("Demo")
            },
            "`JM//CF` is not a safe PBO prefix",
        ),
        (
            ScaffoldOptions {
                prefix: Some("bad prefix".into()),
                ..ScaffoldOptions::new("Demo")
            },
            "`bad prefix` is not a safe PBO prefix",
        ),
        (
            ScaffoldOptions {
                modules: vec!["6_Extra".into()],
                ..ScaffoldOptions::new("Demo")
            },
            "`6_Extra` is not a script module",
        ),
        (
            ScaffoldOptions {
                modules: Vec::new(),
                ..ScaffoldOptions::new("Demo")
            },
            "Pick at least one script module.",
        ),
    ];
    for (options, message) in cases {
        let err = mod_skeleton(&options).expect_err(message);
        assert!(err.contains(message), "{err:?} does not say {message:?}");
    }
}

#[test]
fn placeholder_text_in_a_value_stays_literal() {
    let mut opts = ScaffoldOptions::new("Demo");
    opts.display_name = Some("{{Author}}".into());
    opts.author = "Me".into();
    let files = mod_skeleton(&opts).unwrap();
    assert!(contents(&files, "mod.cpp").contains("name = \"{{Author}}\";"));
}

#[test]
fn line_breaks_never_split_a_mod_cpp_string() {
    let mut opts = ScaffoldOptions::new("Demo");
    opts.author = "Me\nversion = \"9\"".into();
    opts.display_name = Some("Demo Mod\n".into());
    let files = mod_skeleton(&opts).unwrap();
    let mod_cpp = contents(&files, "mod.cpp");
    assert_eq!(
        mod_cpp.lines().filter(|l| l.starts_with("version")).count(),
        1,
        "{mod_cpp}"
    );
    assert!(mod_cpp.contains("name = \"Demo Mod \";"), "{mod_cpp}");
}

#[test]
fn a_nested_prefix_roots_the_module_paths() {
    let mut opts = ScaffoldOptions::new("CF");
    opts.prefix = Some("JM/CF".into());
    let files = mod_skeleton(&opts).unwrap();
    assert!(contents(&files, "config.cpp").contains("\"JM/CF/Scripts/3_Game\""));
}
