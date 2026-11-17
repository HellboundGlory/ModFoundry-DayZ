//! `image.convert` through the registry: PNG files and folders become DayZ `.edds` or `.paa` textures.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::common::tmp;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use modfoundry_commands::image_ops::ImageArgs;
use modfoundry_core::{Actor, CommandError, Core, Outcome};
use modfoundry_texture::edds::decode_edds;
use modfoundry_texture::paa::{decode_paa, PaaFormat};
use serde_json::{json, Value};

fn dispatch(core: &Core, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, "image.convert", args)
        .unwrap_or_else(|e| panic!("image.convert failed: {e}"))
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
fn a_png_converts_to_an_edds_beside_it() {
    let core = crate::common::core();
    let dir = tmp("image-file");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("icon.png");
    fs::write(&input, png(64, 32)).unwrap();

    dispatch(&core, json!({ "path": input, "to": "edds" }));

    let levels = decode_edds(&fs::read(dir.join("icon.edds")).unwrap()).unwrap();
    assert_eq!((levels[0].width, levels[0].height), (64, 32));
}

fn textures_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "edds") {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_folder_of_pngs_mirrors_each_texture_under_out_and_skips_other_files() {
    let core = crate::common::core();
    let dir = tmp("image-folder");
    let source = dir.join("source");
    let out = dir.join("out");
    fs::create_dir_all(source.join("ui/deep")).unwrap();
    fs::write(source.join("top.png"), png(32, 32)).unwrap();
    fs::write(source.join("ui/deep/inner.png"), png(16, 16)).unwrap();
    fs::write(source.join("notes.txt"), "not a texture").unwrap();

    dispatch(&core, json!({ "path": source, "to": "edds", "out": out }));

    assert!(out.join("top.edds").is_file());
    assert!(out.join("ui/deep/inner.edds").is_file());
    assert!(
        !dir.join("source/top.edds").exists(),
        "outputs go under out"
    );
}

#[test]
fn a_folder_with_names_that_differ_only_in_case_is_refused_and_writes_nothing() {
    let core = crate::common::core();
    let cases = [
        (
            "file case",
            vec!["Art.png", "art.png"],
            "Art.png and art.png would both be written as Art.edds",
        ),
        (
            "extension case",
            vec!["a.png", "a.PNG"],
            "a.PNG and a.png would both be written as a.edds",
        ),
        (
            "folder case",
            vec!["UI/a.png", "ui/a.png"],
            "UI/a.png and ui/a.png would both be written as UI/a.edds",
        ),
    ];
    for (label, files, expected) in cases {
        let dir = tmp("image-clash");
        for name in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Not PNGs: the clash must be found before any file is read.
            fs::write(&path, b"not a png").unwrap();
        }

        for preview in [false, true] {
            let error = core
                .dispatch(
                    Actor::CliSession,
                    "image.convert",
                    json!({ "path": dir, "to": "edds", "preview": preview }),
                )
                .unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "{label}, preview {preview}: {error}"
            );
        }
        assert!(
            textures_under(&dir).is_empty(),
            "{label}: {:?}",
            textures_under(&dir)
        );
    }
}

#[cfg(unix)]
#[test]
fn a_png_name_that_is_not_utf8_is_refused_and_writes_nothing() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let core = crate::common::core();
    let dir = tmp("image-non-utf8");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("good.png"), png(16, 16)).unwrap();
    fs::write(dir.join(OsStr::from_bytes(b"bad\xFF.png")), png(16, 16)).unwrap();

    let error = core
        .dispatch(
            Actor::CliSession,
            "image.convert",
            json!({ "path": dir, "to": "edds" }),
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("bad\u{FFFD}.png is not valid UTF-8"),
        "{error}"
    );
    assert!(
        textures_under(&dir).is_empty(),
        "{:?}",
        textures_under(&dir)
    );

    let single = tmp("image-non-utf8-out");
    fs::create_dir_all(&single).unwrap();
    let input = single.join("good.png");
    fs::write(&input, png(16, 16)).unwrap();
    let out = single.join(OsStr::from_bytes(b"bad\xFF.edds"));

    let error = modfoundry_commands::image_ops::convert_images(&ImageArgs {
        path: input,
        to: "edds".to_owned(),
        out: Some(out),
        format: None,
    })
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("bad\u{FFFD}.edds is not valid UTF-8"),
        "{error}"
    );
    assert!(
        textures_under(&single).is_empty(),
        "{:?}",
        textures_under(&single)
    );
}

#[test]
fn a_bad_png_late_in_the_folder_leaves_no_texture_behind() {
    let core = crate::common::core();
    let dir = tmp("image-bad-late");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.png"), png(16, 16)).unwrap();
    fs::write(dir.join("z.png"), b"not a png").unwrap();

    let error = core
        .dispatch(
            Actor::CliSession,
            "image.convert",
            json!({ "path": dir, "to": "edds" }),
        )
        .unwrap_err();

    let error = error.to_string();
    assert!(error.contains("z.png: not a readable PNG"), "{error}");
    assert!(!dir.join("a.edds").exists(), "a.edds was written");
    assert!(!dir.join("z.edds").exists(), "z.edds was written");
}

#[test]
fn a_preview_returns_the_change_set_and_writes_nothing() {
    let core = crate::common::core();
    let dir = tmp("image-preview");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("icon.png");
    fs::write(&input, png(32, 32)).unwrap();

    let Outcome::Preview(plan) = dispatch(
        &core,
        json!({ "path": input, "to": "edds", "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    assert_eq!(
        plan["changes"][0]["path"],
        format!(
            "mod:{}:icon.edds",
            dir.file_name().unwrap().to_string_lossy()
        )
    );
    assert!(!dir.join("icon.edds").exists());
}

#[test]
fn bad_arguments_are_refused_as_invalid_arguments_before_anything_is_written() {
    let core = crate::common::core();
    let dir = tmp("image-bad-args");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("icon.png");
    fs::write(&input, png(32, 32)).unwrap();
    let text = dir.join("notes.txt");
    fs::write(&text, "not an image").unwrap();
    let cases = [
        json!({ "path": input, "to": "png" }),
        json!({ "path": input, "to": "paa", "format": "dxt3" }),
        json!({ "path": input, "to": "edds", "format": "dxt1" }),
        json!({ "path": text, "to": "edds" }),
        json!({ "path": dir.join("missing.png"), "to": "edds" }),
        json!({ "path": input, "to": "edds", "out": input }),
        json!({ "path": input, "to": "edds", "out": dir.join(".").join("icon.png") }),
    ];

    for args in cases {
        let error = core
            .dispatch(Actor::CliSession, "image.convert", args.clone())
            .unwrap_err();

        assert!(
            matches!(error, CommandError::InvalidArguments(_)),
            "{args}: {error}"
        );
    }
    assert!(!dir.join("icon.edds").exists());
}

#[test]
fn a_folder_with_no_pngs_is_an_error() {
    let core = crate::common::core();
    let dir = tmp("image-no-pngs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("notes.txt"), "not an image").unwrap();

    let error = core
        .dispatch(
            Actor::CliSession,
            "image.convert",
            json!({ "path": dir, "to": "edds" }),
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains(&format!("no PNG files under {}", dir.display())),
        "{error}"
    );
}

#[test]
fn a_paa_format_argument_sets_the_block_format() {
    let core = crate::common::core();
    let dir = tmp("image-paa-format");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("tile.png");
    fs::write(&input, png(64, 64)).unwrap();

    dispatch(
        &core,
        json!({ "path": input, "to": "paa", "format": "dxt5" }),
    );

    let (format, _) = decode_paa(&fs::read(dir.join("tile.paa")).unwrap()).unwrap();
    assert_eq!(format, PaaFormat::Dxt5);
}

#[test]
fn paa_refuses_a_png_that_is_not_a_power_of_two_and_writes_nothing() {
    let core = crate::common::core();
    let dir = tmp("image-paa-refused");
    fs::create_dir_all(&dir).unwrap();
    let odd = dir.join("odd.png");
    fs::write(&odd, png(48, 48)).unwrap();
    fs::write(dir.join("valid.png"), png(64, 64)).unwrap();
    let expected = format!(
        "{}: 48x48 is not a power-of-two size, which .paa needs; resize the PNG to 32x32 or 64x64",
        odd.display()
    );

    for path in [odd.clone(), dir.clone()] {
        let error = core
            .dispatch(
                Actor::CliSession,
                "image.convert",
                json!({ "path": path, "to": "paa" }),
            )
            .unwrap_err();

        assert!(error.to_string().contains(&expected), "{error}");
    }
    assert!(!dir.join("valid.paa").exists());
    assert!(!dir.join("odd.paa").exists());
}
