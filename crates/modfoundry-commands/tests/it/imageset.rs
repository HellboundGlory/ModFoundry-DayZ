//! `imageset.build` through the registry: a folder of PNGs becomes one DayZ texture set.

use std::fs;
use std::io::Cursor;

use crate::common::tmp;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use modfoundry_core::{Actor, Core, Outcome};
use modfoundry_texture::edds::decode_edds;
use serde_json::{json, Value};

fn dispatch(core: &Core, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, "imageset.build", args)
        .unwrap_or_else(|e| panic!("imageset.build failed: {e}"))
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let buffer = ImageBuffer::from_fn(width, height, |x, y| {
        Rgba([x as u8, y as u8, (x + y) as u8, 255])
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(buffer)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

#[test]
fn a_folder_of_pngs_writes_the_set_texture_and_text() {
    let core = crate::common::core();
    let dir = tmp("imageset-singles");
    let source = dir.join("icons");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("beta.png"), png(16, 16)).unwrap();
    fs::write(source.join("nested/ignored.png"), png(64, 64)).unwrap();
    fs::write(source.join("notes.txt"), "not a texture").unwrap();
    let out = dir.join("sample.imageset");

    let outcome = dispatch(
        &core,
        json!({ "path": source, "out": out, "game_dir": "MyMod/gui/imagesets" }),
    );

    let text = fs::read_to_string(dir.join("sample.imageset")).unwrap();
    assert!(text.contains(" Name \"sample\""), "{text}");
    assert!(text.contains("Name \"alpha\""), "{text}");
    assert!(text.contains("Name \"beta\""), "{text}");
    assert!(!text.contains("ignored"), "subfolders are not read: {text}");
    assert!(
        text.contains("path \"MyMod/gui/imagesets/sample.edds\""),
        "{text}"
    );
    assert!(text.contains(" mpix 1"), "{text}");
    assert!(!text.contains("mpix 2"), "{text}");

    let atlas = decode_edds(&fs::read(dir.join("sample.edds")).unwrap()).unwrap();
    let (width, height) = (atlas[0].width, atlas[0].height);
    assert!(!dir.join("sample@2x.edds").exists());
    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a recorded change");
    };
    assert_eq!(
        summary.description,
        format!("packed 2 images into {} ({width}x{height})", out.display())
    );
}

#[test]
fn double_images_write_the_second_texture() {
    let core = crate::common::core();
    let dir = tmp("imageset-doubles");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("alpha@2x.png"), png(16, 16)).unwrap();
    fs::write(source.join("beta.png"), png(4, 4)).unwrap();
    fs::write(source.join("beta@2x.png"), png(8, 8)).unwrap();
    let out = dir.join("sample.imageset");

    let outcome = dispatch(
        &core,
        json!({ "path": source, "out": out, "game_dir": "MyMod/gui/imagesets" }),
    );

    let text = fs::read_to_string(dir.join("sample.imageset")).unwrap();
    assert!(text.contains(" mpix 2"), "{text}");
    assert!(
        text.contains("path \"MyMod/gui/imagesets/sample@2x.edds\""),
        "{text}"
    );
    let single = decode_edds(&fs::read(dir.join("sample.edds")).unwrap()).unwrap();
    let double = decode_edds(&fs::read(dir.join("sample@2x.edds")).unwrap()).unwrap();
    assert_eq!(
        (double[0].width, double[0].height),
        (single[0].width * 2, single[0].height * 2)
    );
    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a recorded change");
    };
    assert_eq!(
        summary.description,
        format!(
            "packed 2 images into {} ({}x{}), plus @2x",
            out.display(),
            single[0].width,
            single[0].height
        )
    );
}

#[test]
fn a_preview_returns_the_change_set_and_writes_nothing() {
    let core = crate::common::core();
    let dir = tmp("imageset-preview");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("alpha@2x.png"), png(16, 16)).unwrap();
    let out = dir.join("Sample.IMAGESET");

    let Outcome::Preview(plan) = dispatch(
        &core,
        json!({ "path": source, "out": out, "game_dir": "MyMod/gui/imagesets", "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    let root = format!("mod:{}", dir.file_name().unwrap().to_string_lossy());
    let paths: Vec<String> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            format!("{root}:Sample.IMAGESET"),
            format!("{root}:Sample.edds"),
            format!("{root}:Sample@2x.edds"),
        ]
    );
    assert!(!out.exists());
    assert!(!dir.join("Sample.edds").exists());
    assert!(!dir.join("Sample@2x.edds").exists());
}

#[test]
fn bad_sets_are_refused_and_write_nothing() {
    let core = crate::common::core();
    let dir = tmp("imageset-refused");
    let lone = dir.join("lone");
    fs::create_dir_all(&lone).unwrap();
    fs::write(lone.join("x@2x.png"), png(16, 16)).unwrap();
    let empty = dir.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let broken = dir.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("x.png"), "not a png").unwrap();
    let unicode = dir.join("unicode");
    fs::create_dir_all(&unicode).unwrap();
    fs::write(unicode.join("🔥.png"), png(8, 8)).unwrap();
    let duplicate = dir.join("duplicate");
    fs::create_dir_all(&duplicate).unwrap();
    fs::write(duplicate.join("x@2x.png"), png(16, 16)).unwrap();
    fs::write(duplicate.join("x@2X.png"), png(16, 16)).unwrap();
    let orphans = dir.join("orphans");
    fs::create_dir_all(&orphans).unwrap();
    fs::write(orphans.join("x@2x.png"), png(16, 16)).unwrap();
    fs::write(orphans.join("y.png"), "not a png").unwrap();
    let cases = [
        (
            json!({ "path": lone, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "lone/x@2x.png",
        ),
        (
            json!({ "path": empty, "out": dir.join("set.set"), "game_dir": "gui" }),
            "is not an .imageset file",
        ),
        (
            json!({ "path": empty, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "no PNG files in",
        ),
        (
            json!({ "path": broken, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "broken/x.png",
        ),
        (
            json!({ "path": empty, "out": dir.join("missing/set.imageset"), "game_dir": "gui" }),
            "is not in an existing folder",
        ),
        (
            json!({ "path": unicode, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "may only use letters, digits and _",
        ),
        (
            json!({ "path": dir.join("nope"), "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "is not a folder",
        ),
        (
            json!({ "path": duplicate, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "both name the 2x image of x",
        ),
        (
            json!({ "path": orphans, "out": dir.join("set.imageset"), "game_dir": "gui" }),
            "orphans/x@2x.png has no 1x image",
        ),
    ];

    for (args, expected) in cases {
        let error = core
            .dispatch(Actor::CliSession, "imageset.build", args.clone())
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{args}: {error}");
    }
    assert!(!dir.join("set.imageset").exists());
    assert!(!dir.join("set.edds").exists());
}

#[test]
fn undo_removes_the_written_files() {
    let core = crate::common::core();
    let dir = tmp("imageset-undo");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("alpha@2x.png"), png(16, 16)).unwrap();
    let out = dir.join("sample.imageset");

    let outcome = dispatch(
        &core,
        json!({ "path": source, "out": out, "game_dir": "MyMod/gui/imagesets" }),
    );
    let Outcome::Changed {
        change_set: Some(change_set),
        ..
    } = outcome
    else {
        panic!("expected a recorded change");
    };
    assert!(dir.join("sample.edds").is_file());

    core.dispatch(
        Actor::CliSession,
        "workspace.undo",
        json!({ "change_set": change_set }),
    )
    .unwrap();

    assert!(!dir.join("sample.imageset").exists());
    assert!(!dir.join("sample.edds").exists());
    assert!(!dir.join("sample@2x.edds").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_png_is_packed_and_a_dangling_link_is_skipped() {
    let core = crate::common::core();
    let dir = tmp("imageset-links");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    let elsewhere = dir.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("beta.png"), png(16, 16)).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("beta.png"), source.join("beta.png")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("gone.png"), source.join("gone.png")).unwrap();

    dispatch(
        &core,
        json!({ "path": source, "out": dir.join("sample.imageset"), "game_dir": "gui" }),
    );

    let text = fs::read_to_string(dir.join("sample.imageset")).unwrap();
    assert!(text.contains("Name \"alpha\""), "{text}");
    assert!(text.contains("Name \"beta\""), "{text}");
    assert!(!text.contains("gone"), "{text}");
}

#[test]
fn an_appledouble_sidecar_is_ignored() {
    let core = crate::common::core();
    let dir = tmp("imageset-appledouble");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("._alpha.png"), "not a png").unwrap();

    dispatch(
        &core,
        json!({ "path": source, "out": dir.join("sample.imageset"), "game_dir": "gui" }),
    );

    let text = fs::read_to_string(dir.join("sample.imageset")).unwrap();
    assert!(text.contains("Name \"alpha\""), "{text}");
    assert!(!text.contains("._"), "{text}");
}

#[test]
fn a_double_pairs_with_its_image_whatever_the_case() {
    let core = crate::common::core();
    let dir = tmp("imageset-case");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    fs::write(source.join("ALPHA@2x.png"), png(16, 16)).unwrap();

    dispatch(
        &core,
        json!({ "path": source, "out": dir.join("set.imageset"), "game_dir": "gui" }),
    );

    assert!(dir.join("set@2x.edds").is_file());
}

#[test]
fn relative_paths_record_the_absolute_folder_for_undo() {
    let core = crate::common::core();
    let dir = tmp("imageset-relative");
    let source = dir.join("icons");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("alpha.png"), png(8, 8)).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative =
        |path: &std::path::Path| format!("{up}{}", path.strip_prefix("/").unwrap().display());

    let outcome = dispatch(
        &core,
        json!({ "path": relative(&source), "out": relative(&dir.join("set.imageset")), "game_dir": "gui" }),
    );

    let Outcome::Changed {
        change_set: Some(change_set),
        ..
    } = outcome
    else {
        panic!("expected a recorded change");
    };
    let host_root = std::path::PathBuf::from(change_set["host_root"].as_str().unwrap());
    assert!(host_root.is_absolute(), "{}", host_root.display());
    assert_eq!(
        host_root.canonicalize().unwrap(),
        dir.canonicalize().unwrap()
    );
}
