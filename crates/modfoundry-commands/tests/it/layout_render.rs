//! The `layout.render` command: a .layout drawn from a fake game, the mod's files and a PNG written through the VFS.

use std::fs;
use std::path::Path;

use modfoundry_core::{Actor, CommandError, Outcome};
use modfoundry_font::{
    build_font, layout_text, parse_fnt, Glyph, HAlign, RenderedFont, TextOptions,
};
use modfoundry_texture::edds::encode_edds;
use modfoundry_texture::png::decode_png;
use modfoundry_texture::RgbaImage;
use serde_json::{json, Value};

use crate::common::{core, pack, tmp, write};

const PANEL: &str = "PanelWidgetClass Panel {\n position 0 0\n size 0.5 0.5\n style rover_sim_colorable\n color 1 0 0 1\n}\n";

const STRINGTABLE: &str =
    "\"Language\",\"original\",\"english\"\n\"panel_title\",\"Panel\",\"Panel\"\n";

const PANEL_IMAGESET: &str = "ImageSetClass {\n Name \"panel\"\n RefSize 2 2\n Textures {\n  ImageSetTextureClass {\n   mpix 1\n   path \"{52DD0A1B2C3D4E5F}gui/textures/panel.edds\"\n  }\n }\n Images {\n  ImageSetDefClass {\n   Name \"quad\"\n   Pos 0 0\n   Size 2 2\n  }\n }\n}\n";

fn fake_game(dir: &Path) -> std::path::PathBuf {
    let game = dir.join("game");
    let blue = RgbaImage {
        width: 2,
        height: 2,
        pixels: [0, 0, 255, 255].repeat(4),
    };
    pack(
        &game.join("dta/gui.pbo"),
        &[("prefix", "gui")],
        &[
            ("imagesets/panel.imageset", PANEL_IMAGESET.as_bytes()),
            ("textures/panel.edds", &encode_edds(&[blue]).unwrap()),
        ],
    );
    pack(
        &game.join("dta/languagecore.pbo"),
        &[("prefix", "dta")],
        &[("stringtable.csv", STRINGTABLE.as_bytes())],
    );
    game
}

fn render(args: Value) -> Result<Outcome, CommandError> {
    core().dispatch(Actor::CliSession, "layout.render", args)
}

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.pixels[at..at + 4].try_into().unwrap()
}

#[test]
fn a_flat_panel_is_written_as_a_png_through_the_command() {
    let dir = tmp("layout-render-panel");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let out = dir.join("out/panel.png");

    let outcome = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let Outcome::Data(report) = outcome else {
        panic!("expected data, got {outcome:?}");
    };
    assert_eq!(report["png"], json!(out));
    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!((image.width, image.height), (200, 200));
    assert_eq!(pixel(&image, 50, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 150, 150), [32, 32, 32, 255]);
}

#[test]
fn a_size_past_the_limit_is_refused_by_name() {
    let dir = tmp("layout-render-size");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "99999x1",
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::InvalidArguments(message) if message == "size 99999x1 is over the 7680x4320 limit"),
        "{err:?}"
    );
}

#[test]
fn the_background_colour_is_drawn_behind_the_widgets() {
    let dir = tmp("layout-render-background");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let out = dir.join("panel.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "background": "#102030",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 150), [16, 32, 48, 255]);
}

#[test]
fn a_language_the_stringtable_lacks_is_refused_naming_the_real_ones() {
    let dir = tmp("layout-render-language");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "language": "klingon",
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::Failed(message) if message == "language klingon is not in the stringtable; it has original, english"),
        "{err:?}"
    );
}

#[test]
fn an_out_file_that_is_not_a_png_is_refused_before_rendering() {
    let dir = tmp("layout-render-out-type");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let out = dir.join("panel.txt");

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "out": out,
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::InvalidArguments(message) if message == "out must be a .png file"),
        "{err:?}"
    );
    assert!(!out.exists());
}

#[test]
fn a_preview_names_the_png_without_writing_it() {
    let dir = tmp("layout-render-preview");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let out = dir.join("panel.png");

    let outcome = core()
        .dispatch(
            Actor::CliSession,
            "layout.render",
            json!({
                "path": layout,
                "root": dir.join("mod"),
                "game": game,
                "dependencies": [],
                "out": out,
                "preview": true,
            }),
        )
        .unwrap();

    let Outcome::Preview(plan) = outcome else {
        panic!("expected a preview, got {outcome:?}");
    };
    assert_eq!(plan["changes"].as_array().map(Vec::len), Some(1), "{plan}");
    assert!(!out.exists());
}

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn a_render_without_out_writes_nothing() {
    let dir = tmp("layout-render-no-out");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let before = files_under(&dir);

    let outcome = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
    }))
    .unwrap();

    let Outcome::Data(report) = outcome else {
        panic!("expected data, got {outcome:?}");
    };
    assert!(report["png"].is_null(), "{report}");
    assert_eq!(files_under(&dir), before);
}

#[test]
fn boxes_lists_every_widget_including_children() {
    let dir = tmp("layout-render-boxes");
    let game = fake_game(&dir);
    let nested = "FrameWidgetClass Parent {\n position 0 0\n size 1 1\n {\n PanelWidgetClass A {\n  position 0 0\n  size 0.5 0.5\n }\n PanelWidgetClass B {\n  position 0.5 0.5\n  size 0.5 0.5\n }\n }\n}\n";
    let layout = write(&dir, "mod/gui/nested.layout", nested);

    let outcome = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "boxes": true,
    }))
    .unwrap();

    let Outcome::Data(report) = outcome else {
        panic!("expected data, got {outcome:?}");
    };
    assert_eq!(report["widgets"], 3, "{report}");
    assert_eq!(
        report["boxes"].as_array().map(Vec::len),
        Some(3),
        "{report}"
    );
}

#[test]
fn an_unknown_style_is_reported_as_not_drawn() {
    let dir = tmp("layout-render-unknown-style");
    let game = fake_game(&dir);
    let text =
        "PanelWidgetClass Odd {\n position 0 0\n size 1 1\n style mystery\n color 1 0 0 1\n}\n";
    let layout = write(&dir, "mod/gui/odd.layout", text);

    let outcome = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
    }))
    .unwrap();

    let Outcome::Data(report) = outcome else {
        panic!("expected data, got {outcome:?}");
    };
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{report}");
    assert_eq!(findings[0]["code"], "MF-L210");
    assert_eq!(findings[0]["path"], json!(layout));
}

#[test]
fn an_image_widget_draws_its_picture_from_the_game_imageset() {
    let dir = tmp("layout-render-image");
    let game = fake_game(&dir);
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"set:panel image:quad\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [0, 0, 255, 255]);
}

#[test]
fn an_image_whose_texture_is_missing_is_a_placeholder_with_a_finding() {
    let dir = tmp("layout-render-missing-texture");
    let game = fake_game(&dir);
    let gone = PANEL_IMAGESET
        .replace("panel", "gone")
        .replace("gui/textures/panel.edds", "gui/textures/gone.edds");
    write(&dir, "mod/gui/gone.imageset", &gone);
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"set:gone image:quad\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);

    let outcome = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
    }))
    .unwrap();

    let Outcome::Data(report) = outcome else {
        panic!("expected data, got {outcome:?}");
    };
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{report}");
    assert_eq!(findings[0]["code"], "MF-L214");
}

#[test]
fn a_mod_file_overrides_the_game_file_at_the_same_path() {
    let dir = tmp("layout-render-override");
    let game = fake_game(&dir);
    let red = RgbaImage {
        width: 2,
        height: 2,
        pixels: [255, 0, 0, 255].repeat(4),
    };
    fs::create_dir_all(dir.join("mod/gui/textures")).unwrap();
    fs::write(
        dir.join("mod/gui/textures/panel.edds"),
        encode_edds(&[red]).unwrap(),
    )
    .unwrap();
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"gui/textures/panel.edds\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [255, 0, 0, 255]);
}

fn solid_edds(colour: [u8; 4]) -> Vec<u8> {
    let image = RgbaImage {
        width: 2,
        height: 2,
        pixels: colour.repeat(4),
    };
    encode_edds(&[image]).unwrap()
}

#[test]
fn a_later_dependency_wins_over_an_earlier_one_of_either_kind() {
    let dir = tmp("layout-render-dependency-order");
    let game = fake_game(&dir);
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"gui/textures/panel.edds\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    fs::create_dir_all(dir.join("loose/gui/textures")).unwrap();
    fs::write(
        dir.join("loose/gui/textures/panel.edds"),
        solid_edds([255, 0, 0, 255]),
    )
    .unwrap();
    pack(
        &dir.join("deps/Late.pbo"),
        &[("prefix", "gui")],
        &[("textures/panel.edds", &solid_edds([0, 255, 0, 255]))],
    );

    for (order, expected) in [
        (
            vec![dir.join("loose"), dir.join("deps/Late.pbo")],
            [0, 255, 0, 255],
        ),
        (
            vec![dir.join("deps/Late.pbo"), dir.join("loose")],
            [255, 0, 0, 255],
        ),
    ] {
        let out = dir.join(format!("pic-{}.png", expected[1]));
        render(json!({
            "path": layout,
            "root": dir.join("mod"),
            "game": game,
            "dependencies": order,
            "size": "200x200",
            "out": out,
        }))
        .unwrap();
        let image = decode_png(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(pixel(&image, 150, 50), expected, "{order:?}");
    }
}

#[test]
fn texture_paths_match_ignoring_ascii_case() {
    let dir = tmp("layout-render-case");
    let game = fake_game(&dir);
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"GUI/Textures/PANEL.EDDS\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [0, 0, 255, 255]);
}

#[test]
fn a_loose_file_matches_a_path_with_an_unknown_folder_in_front() {
    let dir = tmp("layout-render-suffix");
    let game = fake_game(&dir);
    fs::create_dir_all(dir.join("mod/gui/textures")).unwrap();
    fs::write(
        dir.join("mod/gui/textures/panel.edds"),
        solid_edds([255, 0, 0, 255]),
    )
    .unwrap();
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"MyMod/gui/textures/panel.edds\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [255, 0, 0, 255]);
}

#[test]
fn a_dependency_folder_with_a_prefix_marker_is_keyed_under_that_prefix() {
    let dir = tmp("layout-render-prefix-marker");
    let game = fake_game(&dir);
    write(&dir, "deps/Dep/$PBOPREFIX$", "MyDep/gui\n");
    fs::create_dir_all(dir.join("deps/Dep/textures")).unwrap();
    fs::write(
        dir.join("deps/Dep/textures/x.edds"),
        solid_edds([255, 0, 0, 255]),
    )
    .unwrap();
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"MyDep/gui/textures/x.edds\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [dir.join("deps/Dep")],
        "size": "200x200",
        "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [255, 0, 0, 255]);
}

#[test]
fn an_out_that_is_a_folder_is_refused() {
    let dir = tmp("layout-render-out-folder");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let out = dir.join("panels.png");
    fs::create_dir_all(&out).unwrap();

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "out": out,
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::InvalidArguments(message) if message == &format!("out {} is a folder, not a file", out.display())),
        "{err:?}"
    );
}

#[test]
fn a_background_that_is_not_hex_colour_is_refused() {
    let dir = tmp("layout-render-background-bad");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "background": "red",
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::InvalidArguments(message) if message == "background red must be #rrggbb or #rrggbbaa"),
        "{err:?}"
    );
}

#[test]
fn a_zero_sized_picture_is_refused() {
    let dir = tmp("layout-render-size-zero");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "0x10",
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::InvalidArguments(message) if message == "size must be at least 1x1"),
        "{err:?}"
    );
}

#[test]
fn a_layout_that_does_not_parse_is_refused_with_its_position() {
    let dir = tmp("layout-render-parse");
    let game = fake_game(&dir);
    let layout = write(
        &dir,
        "mod/gui/broken.layout",
        "PanelWidgetClass Panel {\n position 0 0\n",
    );

    let err = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
    }))
    .unwrap_err();

    assert!(
        matches!(&err, CommandError::Failed(message) if message.starts_with(&format!("{}: ", layout.display()))),
        "{err:?}"
    );
}

fn test_font(dir: &Path) {
    use modfoundry_font::{build_font, Glyph, RenderedFont};
    let glyph = |codepoint: u16| Glyph {
        codepoint,
        width: 4,
        height: 6,
        coverage: vec![255; 24],
        left: 0,
        top: 6,
        advance: 5,
    };
    let space = Glyph {
        codepoint: 0x20,
        width: 0,
        height: 0,
        coverage: Vec::new(),
        left: 0,
        top: 0,
        advance: 3,
    };
    let font = RenderedFont {
        size: 6,
        ascent: 6,
        descent: 0,
        glyphs: vec![
            space,
            glyph(0x50),
            glyph(0x61),
            glyph(0x6E),
            glyph(0x65),
            glyph(0x6C),
        ],
        kerning: vec![],
    };
    let built = build_font("Test16", &font).unwrap();
    let folder = dir.join("mod/gui/fonts");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("Test16.fnt"), built.fnt).unwrap();
    fs::write(folder.join("Test16.edds"), built.texture).unwrap();
}

fn text_layout(text: &str) -> String {
    format!("TextWidgetClass Label {{\n position 0 0\n size 1 1\n text \"{text}\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}}\n")
}

#[test]
fn a_stringtable_key_is_drawn_in_a_font_from_the_mod() {
    let dir = tmp("layout-render-text");
    let game = fake_game(&dir);
    test_font(&dir);
    let layout = write(&dir, "mod/gui/label.layout", &text_layout("#panel_title"));
    let out = dir.join("out/label.png");
    let Outcome::Data(report) = render(json!({
        "path": layout, "root": dir.join("mod"), "game": game,
        "dependencies": [], "size": "200x100", "out": out,
    }))
    .unwrap() else {
        panic!("expected data");
    };
    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert!(
        image.pixels.chunks(4).any(|p| p[..3] == [255, 255, 255]),
        "no text ink"
    );
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(!codes.contains(&"MF-L230"), "{codes:?}");
}

#[test]
fn a_missing_stringtable_key_is_reported() {
    let dir = tmp("layout-render-missing-key");
    let game = fake_game(&dir);
    test_font(&dir);
    let layout = write(&dir, "mod/gui/label.layout", &text_layout("#no_such_key"));
    let Outcome::Data(report) = render(json!({
        "path": layout, "root": dir.join("mod"), "game": game,
        "dependencies": [], "size": "200x100",
    }))
    .unwrap() else {
        panic!("expected data");
    };
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(codes.contains(&"MF-L230"), "{codes:?}");
}

#[test]
fn an_exact_game_entry_beats_a_loose_file_that_only_matches_a_suffix() {
    let dir = tmp("layout-render-exact-first");
    let game = fake_game(&dir);
    fs::create_dir_all(dir.join("mod/textures")).unwrap();
    fs::write(
        dir.join("mod/textures/panel.edds"),
        solid_edds([255, 0, 0, 255]),
    )
    .unwrap();
    let text = "ImageWidgetClass Pic {\n position 0.5 0\n size 0.5 0.5\n image0 \"gui/textures/panel.edds\"\n}\n";
    let layout = write(&dir, "mod/gui/pic.layout", text);
    let out = dir.join("pic.png");

    render(json!({
        "path": layout, "root": dir.join("mod"), "game": game,
        "dependencies": [], "size": "200x200", "out": out,
    }))
    .unwrap();

    let image = decode_png(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(pixel(&image, 150, 50), [0, 0, 255, 255]);
}

#[test]
fn the_language_name_is_matched_ignoring_case() {
    let dir = tmp("layout-render-language-case");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    render(json!({
        "path": layout, "root": dir.join("mod"), "game": game,
        "dependencies": [], "language": "English",
    }))
    .unwrap();
}

#[test]
fn a_game_without_a_stringtable_renders_with_a_warning() {
    let dir = tmp("layout-render-no-stringtable");
    let game = fake_game(&dir);
    fs::remove_file(game.join("dta/languagecore.pbo")).unwrap();
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);
    let Outcome::Data(report) = render(json!({
        "path": layout, "root": dir.join("mod"), "game": game, "dependencies": [],
    }))
    .unwrap() else {
        panic!("expected data");
    };
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("stringtable"), "{warnings}");
}

#[test]
fn a_text_sized_to_its_text_takes_the_width_of_the_text() {
    let dir = tmp("layout-render-size-to-text");
    let game = fake_game(&dir);
    let glyph = |codepoint: u16| Glyph {
        codepoint,
        width: 4,
        height: 6,
        coverage: vec![255; 24],
        left: 0,
        top: 6,
        advance: 5,
    };
    let space = Glyph {
        codepoint: 0x20,
        width: 0,
        height: 0,
        coverage: Vec::new(),
        left: 0,
        top: 0,
        advance: 3,
    };
    let font = RenderedFont {
        size: 6,
        ascent: 6,
        descent: 0,
        glyphs: vec![space, glyph(0x41), glyph(0x42)],
        kerning: vec![],
    };
    let built = build_font("Test16", &font).unwrap();
    pack(
        &game.join("dta/fonts.pbo"),
        &[("prefix", "gui")],
        &[
            ("fonts/test16.fnt", &built.fnt),
            ("fonts/test16.edds", &built.texture),
        ],
    );
    let layout = write(
        &dir,
        "mod/gui/label.layout",
        "TextWidgetClass Label {\n position 0 0\n size 1 0.5\n \"size to text h\" 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}\n",
    );

    let Outcome::Data(report) = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
        "size": "200x200",
        "boxes": true,
        "out": dir.join("label.png"),
    }))
    .unwrap() else {
        panic!("expected data");
    };
    let text = parse_fnt(&built.fnt).unwrap();
    let options = TextOptions {
        em_px: 80.0,
        wrap_width: None,
        halign: HAlign::Left,
    };
    let expected = layout_text(&text, "AB", &options).width;
    let width = report["boxes"][0]["rect"][2].as_f64().unwrap();
    assert!(
        (width - f64::from(expected)).abs() < 0.01,
        "box width {width}, text width {expected}"
    );
}

#[test]
fn a_render_without_size_is_1920_by_1080() {
    let dir = tmp("layout-render-default-size");
    let game = fake_game(&dir);
    let layout = write(&dir, "mod/gui/panel.layout", PANEL);

    let Outcome::Data(report) = render(json!({
        "path": layout,
        "root": dir.join("mod"),
        "game": game,
        "dependencies": [],
    }))
    .unwrap() else {
        panic!("expected data");
    };
    assert_eq!(
        (report["width"].as_u64(), report["height"].as_u64()),
        (Some(1920), Some(1080))
    );
}
