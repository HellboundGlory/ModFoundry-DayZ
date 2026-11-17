//! `font.build` through the registry: a TTF becomes DayZ .fnt, .edds and .xml files.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::tmp;
use epaint_default_fonts::{HACK_REGULAR, UBUNTU_LIGHT};
use modfoundry_core::{Actor, Core, Outcome};
use modfoundry_font::{render_field_font, render_font, CharSet, Sizing};
use serde_json::{json, Value};

fn dispatch(core: &Core, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, "font.build", args)
        .unwrap_or_else(|e| panic!("font.build failed: {e}"))
}

fn font_file(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("Hack-Regular.ttf");
    fs::write(&path, HACK_REGULAR).unwrap();
    path
}

fn kerned_font_file(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("Ubuntu-Light.ttf");
    fs::write(&path, UBUNTU_LIGHT).unwrap();
    path
}

#[test]
fn sizes_write_their_fonts_and_a_size_list() {
    let core = crate::common::core();
    let dir = tmp("font-sizes");
    let source = kerned_font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let outcome = dispatch(
        &core,
        json!({ "path": source, "sizes": [12, 10], "out": out }),
    );

    for size in [10, 12] {
        let fnt = out.join(format!("ubuntu_light{size}.fnt"));
        let bytes = fs::read(&fnt).unwrap();
        assert_eq!(&bytes[..4], b"FORM", "{}", fnt.display());
        assert_eq!(&bytes[8..12], b"FNT3", "{}", fnt.display());
        assert!(out.join(format!("ubuntu_light{size}.edds")).is_file());
    }
    assert_eq!(
        fs::read_to_string(out.join("ubuntu_light.xml")).unwrap(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<fontGenerator>\n    <font size=\"10\"/>\n    <font size=\"12\"/>\n</fontGenerator>\n"
    );

    let bytes = fs::read(&source).unwrap();
    let [small, large] = [10, 12].map(|size| {
        render_font(&bytes, size, CharSet::Default, Sizing::Line)
            .unwrap()
            .kerning
            .len()
    });
    assert!(1 < small && small < large, "{small} and {large} pairs");
    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a recorded change, got {outcome:?}");
    };
    assert!(
        summary.description.starts_with(&format!(
            "wrote ubuntu_light at sizes 10, 12 into {}",
            out.display()
        )),
        "{}",
        summary.description
    );
    assert!(
        summary
            .description
            .ends_with(&format!("glyphs, {large} kerning pairs)")),
        "{}",
        summary.description
    );
}

#[test]
fn a_single_kerning_pair_is_named_in_the_singular() {
    let core = crate::common::core();
    let dir = tmp("font-one-pair");
    let source = kerned_font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let outcome = dispatch(
        &core,
        json!({ "path": source, "sizes": [3], "out": out, "chars": "all", "sizing": "em" }),
    );

    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a recorded change, got {outcome:?}");
    };
    let pairs = render_font(&fs::read(&source).unwrap(), 3, CharSet::All, Sizing::Em)
        .unwrap()
        .kerning
        .len();
    assert_eq!(pairs, 1, "the font and size give one pair");
    assert!(
        summary.description.ends_with("glyphs, 1 kerning pair)"),
        "{}",
        summary.description
    );
}

#[test]
fn msdf_writes_one_font_pair_and_no_size_list() {
    let core = crate::common::core();
    let dir = tmp("font-msdf");
    let source = kerned_font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let outcome = dispatch(&core, json!({ "path": source, "msdf": true, "out": out }));

    let fnt = fs::read(out.join("ubuntu_light.fnt")).unwrap();
    assert_eq!(&fnt[..4], b"FORM");
    assert_eq!(&fnt[8..12], b"FNT5");
    assert!(out.join("ubuntu_light.edds").is_file());
    let mut names: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["ubuntu_light.edds", "ubuntu_light.fnt"]);

    let glps = fnt.windows(4).position(|tag| tag == b"GLPS").unwrap() + 8;
    let glyphs = u32::from_le_bytes(fnt[glps + 8..glps + 12].try_into().unwrap());
    let pairs = render_field_font(&fs::read(&source).unwrap(), CharSet::Default)
        .unwrap()
        .kerning
        .len();
    assert!(pairs > 1, "{pairs} pairs");

    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a recorded change, got {outcome:?}");
    };
    assert_eq!(
        summary.description,
        format!(
            "wrote ubuntu_light as one distance-field font into {} ({glyphs} glyphs, {pairs} kerning pairs)",
            out.display()
        )
    );
}

#[test]
fn a_name_overrides_the_stem_and_all_characters_add_glyphs() {
    let core = crate::common::core();
    let dir = tmp("font-name-chars");
    let source = font_file(&dir.join("src"));
    let default_out = dir.join("default");
    fs::create_dir_all(&default_out).unwrap();
    let all_out = dir.join("all");
    fs::create_dir_all(&all_out).unwrap();

    dispatch(
        &core,
        json!({ "path": source, "sizes": [10], "out": default_out, "name": "custom" }),
    );
    dispatch(
        &core,
        json!({ "path": source, "sizes": [10], "out": all_out, "name": "custom", "chars": "all" }),
    );

    assert!(all_out.join("custom10.fnt").is_file());
    assert!(all_out.join("custom10.edds").is_file());
    assert!(all_out.join("custom.xml").is_file());
    assert!(!default_out.join("hack_regular10.fnt").exists());

    let default_bytes = fs::metadata(default_out.join("custom10.fnt"))
        .unwrap()
        .len();
    let all_bytes = fs::metadata(all_out.join("custom10.fnt")).unwrap().len();
    assert!(
        all_bytes > default_bytes,
        "all characters write more glyphs: {all_bytes} vs {default_bytes}"
    );
}

#[test]
fn a_preview_returns_the_change_set_and_writes_nothing() {
    let core = crate::common::core();
    let dir = tmp("font-preview");
    let source = font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let Outcome::Preview(plan) = dispatch(
        &core,
        json!({ "path": source, "sizes": [10], "out": out, "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    let root = format!("mod:{}", out.file_name().unwrap().to_string_lossy());
    let paths: Vec<String> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            format!("{root}:hack_regular10.fnt"),
            format!("{root}:hack_regular10.edds"),
            format!("{root}:hack_regular.xml"),
        ]
    );
    assert!(fs::read_dir(&out).unwrap().next().is_none());
}

#[test]
fn bad_arguments_are_refused_and_write_nothing() {
    let core = crate::common::core();
    let dir = tmp("font-refused");
    let source = font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();
    let not_a_font = dir.join("not-a-font.txt");
    fs::write(&not_a_font, "hello").unwrap();
    let cases = [
        (
            json!({ "path": source, "sizes": [], "out": out }),
            "no sizes",
        ),
        (
            json!({ "path": source, "sizes": [0], "out": out }),
            "outside 1 to 512",
        ),
        (
            json!({ "path": source, "sizes": [10, 10], "out": out }),
            "given twice",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": out, "name": "Bad Name" }),
            "may only use",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": out, "name": "" }),
            "the name is empty",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": out, "name": "a".repeat(250) }),
            "is too long",
        ),
        (
            json!({ "path": source, "msdf": true, "sizes": [10], "out": out }),
            "--msdf bakes one font for every size; drop --sizes",
        ),
        (
            json!({ "path": source, "msdf": true, "sizing": "em", "out": out }),
            "--sizing is for bitmap sizes; --msdf has none",
        ),
        (
            json!({ "path": source, "msdf": true, "name": "a".repeat(251), "out": out }),
            "is too long",
        ),
        (
            json!({ "path": source, "sizes": [10, 512], "out": out, "chars": "all" }),
            "at 512 pixels: ",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": out, "chars": "latin" }),
            "default or all",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": out, "sizing": "cell" }),
            "line or em",
        ),
        (
            json!({ "path": not_a_font, "sizes": [10], "out": out }),
            "not a font",
        ),
        (
            json!({ "path": dir.join("missing.ttf"), "sizes": [10], "out": out }),
            "is not a file",
        ),
        (
            json!({ "path": source, "sizes": [10], "out": dir.join("missing") }),
            "is not a folder",
        ),
    ];

    for (args, expected) in cases {
        let error = core
            .dispatch(Actor::CliSession, "font.build", args.clone())
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{args}: {error}");
    }
    assert!(
        fs::read_dir(&out).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
}

#[test]
fn undo_removes_the_written_files() {
    let core = crate::common::core();
    let dir = tmp("font-undo");
    let source = font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let outcome = dispatch(
        &core,
        json!({ "path": source, "sizes": [10, 12], "out": out }),
    );
    let Outcome::Changed {
        change_set: Some(change_set),
        ..
    } = outcome
    else {
        panic!("expected a recorded change");
    };
    assert_eq!(fs::read_dir(&out).unwrap().count(), 5);

    core.dispatch(
        Actor::CliSession,
        "workspace.undo",
        json!({ "change_set": change_set }),
    )
    .unwrap();

    assert!(
        fs::read_dir(&out).unwrap().next().is_none(),
        "undo removes every written file"
    );
}

#[test]
fn a_size_is_the_cell_height_unless_em_sizing_is_asked() {
    let core = crate::common::core();
    let dir = tmp("font-sizing");
    let source = font_file(&dir.join("src"));
    let out = dir.join("fonts");
    fs::create_dir_all(&out).unwrap();

    let cell_height = |name: &str| {
        let fnt = fs::read(out.join(format!("{name}12.fnt"))).unwrap();
        let tcrd = fnt.windows(4).position(|tag| tag == b"TCRD").unwrap() + 8;
        u16::from_le_bytes([fnt[tcrd + 6], fnt[tcrd + 7]])
    };
    dispatch(
        &core,
        json!({ "path": source, "sizes": [12], "out": out, "name": "line" }),
    );
    dispatch(
        &core,
        json!({ "path": source, "sizes": [12], "out": out, "name": "em", "sizing": "em" }),
    );

    // At a 12 pixel em Hack's cells grow to hold its tallest and lowest ink.
    assert_eq!((cell_height("line"), cell_height("em")), (12, 16));
}
