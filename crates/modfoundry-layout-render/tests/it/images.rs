use std::cell::Cell;
use std::collections::HashMap;

use modfoundry_layout::geometry::{resolve, Screen};
use modfoundry_layout::imageset::{Imageset, ImagesetImage, ImagesetTexture};
use modfoundry_layout::parse::parse;
use modfoundry_layout_render::{render, MemoryResources, RenderOptions, Rendered, Resources};
use modfoundry_texture::edds::encode_edds;
use modfoundry_texture::paa::{encode_paa, PaaFormat};
use modfoundry_texture::RgbaImage;

use crate::common::pixel;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn checker() -> RgbaImage {
    RgbaImage {
        width: 2,
        height: 2,
        pixels: [WHITE, BLACK, BLACK, WHITE].concat(),
    }
}

fn draw(text: &str, resources: &MemoryResources) -> Rendered {
    let widgets = parse(text).unwrap();
    let layout = resolve(
        &widgets,
        text,
        Screen {
            width: 4,
            height: 4,
        },
    );
    render(
        &layout,
        &widgets,
        text,
        &RenderOptions::default(),
        resources,
    )
}

/// Set `gui`: one image `pic` at texel-half position (1, 1) in a `RefSize` of 2 x 2, on `gui/s.edds`.
fn gui_set() -> Imageset {
    Imageset {
        name: "gui".to_owned(),
        ref_size: (2, 2),
        textures: vec![ImagesetTexture {
            mpix: 0,
            path: "gui/s.edds".to_owned(),
        }],
        images: vec![ImagesetImage {
            name: "pic".to_owned(),
            x: 1,
            y: 1,
            w: 1,
            h: 1,
            flags: String::new(),
        }],
    }
}

fn with_files(files: &[(&str, Vec<u8>)]) -> MemoryResources {
    MemoryResources {
        files: files
            .iter()
            .map(|(path, bytes)| (path.to_string(), bytes.clone()))
            .collect::<HashMap<_, _>>(),
        ..MemoryResources::default()
    }
}

#[test]
fn plain_texture_path_draws_the_whole_picture() {
    let texture = encode_edds(&[checker()]).unwrap();
    let resources = with_files(&[("gui/x.edds", texture)]);
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 imageTexture "{GUID}gui\x.edds"
}"#;
    let rendered = draw(text, &resources);
    assert_eq!(pixel(&rendered, 0, 0), WHITE);
    assert_eq!(pixel(&rendered, 3, 0), BLACK);
    assert_eq!(pixel(&rendered, 0, 3), BLACK);
    assert_eq!(pixel(&rendered, 3, 3), WHITE);
    assert!(rendered.findings.is_empty());
    assert_eq!(rendered.drawn, 1);
}

#[test]
fn imageset_picture_is_cut_from_its_texture_by_the_ref_size_scale() {
    let mut texels = [BLACK; 4 * 4];
    for y in 2..4 {
        for x in 2..4 {
            texels[y * 4 + x] = WHITE;
        }
    }
    let texture = encode_edds(&[RgbaImage {
        width: 4,
        height: 4,
        pixels: texels.concat(),
    }])
    .unwrap();
    let resources = MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), gui_set())]),
        ..with_files(&[("gui/s.edds", texture)])
    };
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 image0 "set:gui image:pic"
}"#;
    let rendered = draw(text, &resources);
    assert_eq!(pixel(&rendered, 0, 0), WHITE);
    assert_eq!(pixel(&rendered, 3, 3), WHITE);
    assert_eq!(rendered.drawn, 1);
}

#[test]
fn unreadable_pictures_are_drawn_as_placeholders_with_the_reason() {
    let cases = [
        (
            "set:nope image:pic",
            MemoryResources::default(),
            "imageset nope not found",
        ),
        (
            "set:gui image:missing",
            MemoryResources {
                imagesets: HashMap::from([("gui".to_owned(), gui_set())]),
                ..MemoryResources::default()
            },
            "image missing is not in imageset gui",
        ),
        (
            "set:gui image:pic",
            MemoryResources {
                imagesets: HashMap::from([("gui".to_owned(), gui_set())]),
                ..MemoryResources::default()
            },
            "texture gui/s.edds not found",
        ),
        (
            "gui/none.edds",
            MemoryResources::default(),
            "texture gui/none.edds not found",
        ),
        (
            "gui/bad.edds",
            with_files(&[("gui/bad.edds", b"junk".to_vec())]),
            "texture gui/bad.edds cannot be read (not an .edds file)",
        ),
    ];
    for (reference, resources, reason) in cases {
        let text = format!(
            "ImageWidgetClass Pic {{\n position 0 0\n size 1 1\n image0 \"{reference}\"\n}}"
        );
        let rendered = draw(&text, &resources);
        assert_eq!(rendered.findings.len(), 1, "{reference}");
        assert_eq!(rendered.findings[0].code, "MF-L214");
        assert_eq!(
            rendered.findings[0].message,
            format!("image {reference} is drawn as a placeholder: {reason}: Pic")
        );
        assert_eq!(pixel(&rendered, 0, 0), WHITE, "{reference} outline");
    }
}

#[test]
fn a_compressed_high_quality_texture_falls_back_to_the_next_one() {
    let mut compressed = encode_edds(&[checker()]).unwrap();
    compressed[0x54..0x58].copy_from_slice(b"DXT3");
    let mut set = gui_set();
    set.textures = vec![
        ImagesetTexture {
            mpix: 2,
            path: "gui/s@2x.edds".to_owned(),
        },
        ImagesetTexture {
            mpix: 0,
            path: "gui/s.edds".to_owned(),
        },
    ];
    let resources = MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), set)]),
        ..with_files(&[
            ("gui/s@2x.edds", compressed),
            ("gui/s.edds", encode_edds(&[checker()]).unwrap()),
        ])
    };
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 image0 "set:gui image:pic"
}"#;
    let rendered = draw(text, &resources);
    assert!(rendered.findings.is_empty());
    assert_eq!(pixel(&rendered, 0, 0), WHITE);
    assert_eq!(pixel(&rendered, 3, 3), WHITE);
}

#[test]
fn translucent_picture_without_src_alpha_is_reported() {
    let translucent = RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![255, 255, 255, 128],
    };
    let resources = with_files(&[("gui/t.edds", encode_edds(&[translucent]).unwrap())]);
    let without = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 imageTexture "gui/t.edds"
}"#;
    let rendered = draw(without, &resources);
    assert_eq!(rendered.findings.len(), 1);
    assert_eq!(rendered.findings[0].code, "MF-L215");
    assert_eq!(
        rendered.findings[0].message,
        "picture alpha is not used without \"src alpha\" 1: Pic"
    );
    let with = format!("{}\n \"src alpha\" 1\n}}", without.trim_end_matches('}'));
    assert!(draw(&with, &resources).findings.is_empty());
}

#[test]
fn image_options_that_are_not_drawn_are_reported_one_each() {
    let resources = with_files(&[("gui/x.edds", encode_edds(&[checker()]).unwrap())]);
    let cases = [
        ("mode additive", "mode additive"),
        ("\"stretch mode\" fit_w_center", "stretch mode fit_w_center"),
        ("Mask 1", "Mask 1"),
        ("Progress 1", "Progress 1"),
        ("inheritalpha 1", "inheritalpha 1"),
    ];
    for (option, shown) in cases {
        let text = format!(
            "ImageWidgetClass Pic {{\n position 0 0\n size 1 1\n imageTexture \"gui/x.edds\"\n {option}\n}}"
        );
        let rendered = draw(&text, &resources);
        assert_eq!(rendered.findings.len(), 1, "{option}");
        assert_eq!(rendered.findings[0].code, "MF-L216");
        assert_eq!(
            rendered.findings[0].message,
            format!("image option not drawn: {shown}: Pic")
        );
    }
}

#[test]
fn clamp_mode_is_not_reported() {
    let resources = with_files(&[("gui/x.edds", encode_edds(&[checker()]).unwrap())]);
    let text = "ImageWidgetClass Pic {\n position 0 0\n size 1 1\n imageTexture \"gui/x.edds\"\n \"clamp mode\" clamp\n}";
    assert!(draw(text, &resources).findings.is_empty());
}

#[test]
fn a_texture_shared_by_two_widgets_is_read_once() {
    struct CountingReads {
        inner: MemoryResources,
        reads: Cell<usize>,
    }
    impl Resources for CountingReads {
        fn imageset(&self, name: &str) -> Option<std::rc::Rc<Imageset>> {
            self.inner.imageset(name)
        }
        fn file(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.inner.file(path)
        }
        fn text(&self, key: &str) -> Option<String> {
            self.inner.text(key)
        }
        fn font_style(&self, class: &str, style: &str) -> Option<String> {
            self.inner.font_style(class, style)
        }

        fn style_color(&self, class: &str, style: &str) -> Option<u32> {
            self.inner.style_color(class, style)
        }
    }
    let resources = CountingReads {
        inner: with_files(&[("gui/x.edds", encode_edds(&[checker()]).unwrap())]),
        reads: Cell::new(0),
    };
    let text = r#"ImageWidgetClass A {
 position 0 0
 size 1 1
 imageTexture "gui/x.edds"
}
ImageWidgetClass B {
 position 0 0
 size 1 1
 imageTexture "gui/x.edds"
}"#;
    let widgets = parse(text).unwrap();
    let layout = resolve(
        &widgets,
        text,
        Screen {
            width: 4,
            height: 4,
        },
    );
    let rendered = render(
        &layout,
        &widgets,
        text,
        &RenderOptions::default(),
        &resources,
    );
    assert_eq!(rendered.drawn, 2);
    assert_eq!(resources.reads.get(), 1);
}

#[test]
fn paa_texture_is_decoded_by_its_extension() {
    let texture = encode_paa(&[checker()], PaaFormat::Dxt1).unwrap();
    let resources = with_files(&[("gui/x.paa", texture)]);
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 imageTexture "gui/x.paa"
}"#;
    let rendered = draw(text, &resources);
    assert_eq!(pixel(&rendered, 0, 0), WHITE);
    assert_eq!(pixel(&rendered, 3, 3), WHITE);
    assert_eq!(pixel(&rendered, 3, 0), BLACK);
    assert!(rendered.findings.is_empty());
}

#[test]
fn image_widget_with_an_empty_image_keeps_its_style_fill() {
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 image0 ""
 style rover_sim_colorable
 color 1 0 0 1
}"#;
    let rendered = draw(text, &MemoryResources::default());
    assert_eq!(pixel(&rendered, 2, 2), [255, 0, 0, 255]);
    assert!(rendered.findings.is_empty());
}

#[test]
fn picture_is_multiplied_by_the_widget_colour() {
    let resources = with_files(&[("gui/x.edds", encode_edds(&[checker()]).unwrap())]);
    let text = r#"ImageWidgetClass Pic {
 position 0 0
 size 1 1
 imageTexture "gui/x.edds"
 color 1 0 0 1
}"#;
    let rendered = draw(text, &resources);
    assert_eq!(pixel(&rendered, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&rendered, 3, 0), [0, 0, 0, 255]);
}

fn solid(colour: [u8; 4]) -> Vec<u8> {
    encode_edds(&[RgbaImage {
        width: 2,
        height: 2,
        pixels: colour.repeat(4),
    }])
    .unwrap()
}

#[test]
fn the_highest_mpix_texture_is_used_when_both_decode() {
    let mut set = gui_set();
    set.textures = vec![
        ImagesetTexture {
            mpix: 0,
            path: "gui/lo.edds".to_owned(),
        },
        ImagesetTexture {
            mpix: 2,
            path: "gui/hi.edds".to_owned(),
        },
    ];
    let resources = MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), set)]),
        ..with_files(&[("gui/lo.edds", solid(WHITE)), ("gui/hi.edds", solid(BLACK))])
    };
    let text = "ImageWidgetClass Pic {\n position 0 0\n size 1 1\n image0 \"set:gui image:pic\"\n}";
    assert_eq!(pixel(&draw(text, &resources), 0, 0), BLACK);
}

#[test]
fn a_hidden_picture_is_skipped_or_drawn_faded() {
    let resources = with_files(&[("gui/x.edds", solid(WHITE))]);
    let text = "ImageWidgetClass Pic {\n position 0 0\n size 1 1\n visible 0\n imageTexture \"gui/x.edds\"\n}";
    let skipped = draw(text, &resources);
    assert_eq!(pixel(&skipped, 0, 0), [32, 32, 32, 255]);
    let widgets = parse(text).unwrap();
    let layout = resolve(
        &widgets,
        text,
        Screen {
            width: 4,
            height: 4,
        },
    );
    let options = RenderOptions {
        background: [0, 0, 0, 255],
        show_hidden: true,
    };
    let faded = render(&layout, &widgets, text, &options, &resources);
    assert_eq!(pixel(&faded, 0, 0), [89, 89, 89, 255]);
}

#[test]
fn extra_spaces_in_a_set_reference_still_find_the_image() {
    let mut set = gui_set();
    set.textures[0].path = "gui/s.edds".to_owned();
    let resources = MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), set)]),
        ..with_files(&[("gui/s.edds", encode_edds(&[checker()]).unwrap())])
    };
    let text =
        "ImageWidgetClass Pic {\n position 0 0\n size 1 1\n image0 \"set:gui   image:pic\"\n}";
    let rendered = draw(text, &resources);
    assert!(rendered.findings.is_empty(), "{:?}", rendered.findings);
}

#[test]
fn an_image_rectangle_past_the_integer_range_is_a_placeholder_not_a_crash() {
    let mut set = gui_set();
    set.images[0].x = u32::MAX;
    set.images[0].w = 2;
    let resources = MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), set)]),
        ..with_files(&[("gui/s.edds", encode_edds(&[checker()]).unwrap())])
    };
    let text = "ImageWidgetClass Pic {\n position 0 0\n size 1 1\n image0 \"set:gui image:pic\"\n}";
    let rendered = draw(text, &resources);
    assert_eq!(rendered.findings[0].code, "MF-L214");
}
