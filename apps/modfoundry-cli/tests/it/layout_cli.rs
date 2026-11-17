//! `mf layout check`: the findings it renders, its counts and its exit code.

use std::path::Path;

use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};

use crate::binary::{mf, stderr, stdout, Temp};

pub(crate) fn pack(path: &Path, prefix: &str, files: &[(&str, &[u8])]) {
    let properties = vec![("prefix".to_string(), prefix.to_string())];
    let entries: Vec<PboFileToWrite> = files
        .iter()
        .map(|(name, data)| PboFileToWrite {
            name: (*name).to_string(),
            data: data.to_vec(),
            timestamp: 1,
        })
        .collect();
    let bytes = pack_pbo(&properties, &entries, &PackOptions::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

const GAME_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 size 100 100
 {
  TextWidgetClass Label {
   text "Hello"
   font "gui/fonts/f"
  }
 }
}
"#;

fn fake_game(dir: &Path) -> std::path::PathBuf {
    let game = dir.join("game");
    pack(
        &game.join("dta/gui.pbo"),
        "gui",
        &[
            ("layouts/a.layout", GAME_LAYOUT.as_bytes()),
            ("fonts/f.fnt", b"font"),
        ],
    );
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[("widgets.c", b"class Widget {}\n")],
    );
    game
}

const BROKEN_LAYOUT: &str = r#"FrameWidgetClass Root {
 position 0 0
 size 100 100
 {
  TextWidgetClas Label {
   text "Hello"
  }
 }
}
"#;

#[test]
fn a_broken_layout_prints_the_finding_at_its_line_and_exits_one() {
    let dir = Temp::new("layout-cli-broken");
    let game = fake_game(dir.path());
    let file = dir.join("mod/layouts/broken.layout");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, BROKEN_LAYOUT).unwrap();

    let out = mf(&[
        "layout",
        "check",
        file.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!("] error {}:5:3: ", file.display())),
        "got: {text}"
    );
    assert!(text.contains("MF-L102"), "got: {text}");
    assert!(text.contains("1 issue(s) in 1 file(s)"), "got: {text}");
}

#[test]
fn a_missing_path_is_an_error_on_stderr() {
    let dir = Temp::new("layout-cli-missing");
    let game = fake_game(dir.path());

    let out = mf(&[
        "layout",
        "check",
        dir.join("nope").to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("does not exist"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn a_clean_layout_against_the_game_prints_ok_and_exits_zero() {
    let dir = Temp::new("layout-cli-clean");
    let game = fake_game(dir.path());
    let layouts = dir.join("mod/layouts");
    std::fs::create_dir_all(&layouts).unwrap();
    std::fs::write(layouts.join("clean.layout"), GAME_LAYOUT).unwrap();

    let out = mf(&[
        "layout",
        "check",
        dir.join("mod/layouts").to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("ok: no issues in 1 file(s)"),
        "got: {}",
        stdout(&out)
    );
}

#[test]
fn a_finding_in_a_file_that_is_not_utf8_still_has_its_line() {
    let dir = Temp::new("layout-cli-latin1");
    let game = fake_game(dir.path());
    let file = dir.join("mod/layouts/latin1.layout");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut bytes = BROKEN_LAYOUT
        .replace("\"Hello\"", "\"H\u{1}llo\"")
        .into_bytes();
    let at = bytes.iter().position(|&b| b == 1).unwrap();
    bytes[at] = 0xE9;
    std::fs::write(&file, bytes).unwrap();

    let out = mf(&[
        "layout",
        "check",
        file.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    let text = stdout(&out);
    assert!(
        text.contains(&format!("] error {}:5:3: ", file.display())),
        "got: {text}"
    );
}

#[test]
fn check_takes_the_saved_dependencies_unless_told_there_are_none() {
    use modfoundry_commands::project_settings::{project_id, PREFS_PROJECT_KEY};
    use modfoundry_store::settings::Scope;

    let dir = Temp::new("layout-saved-deps");
    let root = dir.join("mod");
    std::fs::create_dir_all(root.join("layouts")).unwrap();
    std::fs::write(root.join("config.cpp"), "class CfgPatches {};\n").unwrap();
    let layout = root.join("layouts/mine.layout");
    std::fs::write(
        &layout,
        "TextWidgetClass Root {\n imageTexture \"{5A89D58DD2276E85}gui/t3.edds\"\n}\n",
    )
    .unwrap();
    let dep = dir.join("Dep");
    std::fs::create_dir_all(dep.join("gui")).unwrap();
    std::fs::write(dep.join("gui/t3.edds"), "edds").unwrap();
    let game = fake_game(dir.path());
    let db = dir.join("db/modfoundry.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let saved = serde_json::json!({"launch.mods.test": {"Mods": [{
        "id": "dep", "name": "Dep", "role": "Both", "enabled": true, "use_as": "Dependency",
        "source": {"Folder": {"path": dep}},
    }]}});
    modfoundry_store::Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&root)),
            PREFS_PROJECT_KEY,
            &saved,
        )
        .unwrap();
    let check = |extra: &[&str]| {
        let mut args = vec![
            "layout",
            "check",
            layout.to_str().unwrap(),
            "--game",
            game.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        stdout(&crate::binary::mf_in(&db, &args))
    };

    let with_saved = check(&[]);
    let without = check(&["--no-dependencies"]);

    assert!(!with_saved.contains("MF-L106"), "{with_saved}");
    assert!(without.contains("MF-L106"), "{without}");
}
