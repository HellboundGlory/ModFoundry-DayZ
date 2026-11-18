use modfoundry_font::{
    build_font, layout_text, parse_fnt, Glyph, HAlign, RenderedFont, TextBlock, TextOptions,
};
use modfoundry_layout::geometry::{resolve_with, Screen, TextMeasure, TextSize};
use modfoundry_layout::parse::parse;
use modfoundry_layout_render::{render, GameText, MemoryResources, RenderOptions, Rendered};
use modfoundry_texture::RgbaImage;

const FONT: &str = "gui/fonts/Test16";

fn glyph(codepoint: u16, advance: i16) -> Glyph {
    Glyph {
        codepoint,
        width: 4,
        height: 6,
        coverage: vec![255; 24],
        left: 0,
        top: 6,
        advance,
    }
}

fn space(advance: i16) -> Glyph {
    Glyph {
        codepoint: 0x20,
        width: 0,
        height: 0,
        coverage: Vec::new(),
        left: 0,
        top: 0,
        advance,
    }
}

fn resources_with_font() -> MemoryResources {
    let font = RenderedFont {
        size: 6,
        ascent: 6,
        descent: 0,
        glyphs: vec![space(3), glyph(0x41, 5), glyph(0x42, 5)],
        kerning: vec![],
    };
    let built = build_font("Test16", &font).unwrap();
    let mut resources = MemoryResources::default();
    resources.files.insert(format!("{FONT}.fnt"), built.fnt);
    resources
        .files
        .insert(format!("{FONT}.edds"), built.texture);
    resources
}

fn block(
    resources: &MemoryResources,
    text: &str,
    em_px: f32,
    wrap_width: Option<f32>,
) -> TextBlock {
    let font = parse_fnt(&resources.files[&format!("{FONT}.fnt")]).unwrap();
    let options = TextOptions {
        em_px,
        wrap_width,
        halign: HAlign::Left,
    };
    layout_text(&font, text, &options)
}

#[test]
fn measured_size_of_one_line_is_the_laid_out_block() {
    let resources = resources_with_font();
    let text = "TextWidgetClass Label {\n size 1 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    let measured = GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .unwrap();
    let expected = block(&resources, "AB", 12.0, None);
    assert_eq!(
        measured,
        TextSize {
            width: expected.width,
            height: expected.height,
        }
    );
}

#[test]
fn a_narrow_wrap_width_measures_the_extra_lines() {
    let resources = resources_with_font();
    let text = "MultilineTextWidgetClass Label {\n size 1 1\n text \"AB AB AB\"\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    let one_line = GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .unwrap();
    let wrapped = GameText::new(&resources)
        .measure(&widgets[0], 12.0, Some(12.0))
        .unwrap();
    let expected = block(&resources, "AB AB AB", 12.0, Some(12.0));
    assert!(expected.lines > 1, "fixture did not wrap");
    assert_eq!(
        wrapped,
        TextSize {
            width: expected.width,
            height: expected.height,
        }
    );
    assert!(wrapped.height > one_line.height);
}

#[test]
fn a_text_without_its_font_is_not_measured() {
    let resources = MemoryResources::default();
    let text = "TextWidgetClass Label {\n size 1 1\n text \"AB\"\n font \"gui/fonts/Missing\"\n}";
    let widgets = parse(text).unwrap();
    assert_eq!(
        GameText::new(&resources).measure(&widgets[0], 12.0, None),
        None
    );
}

#[test]
fn a_widget_without_text_is_not_measured() {
    let resources = resources_with_font();
    let text = "TextWidgetClass Label {\n size 1 1\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    assert_eq!(
        GameText::new(&resources).measure(&widgets[0], 12.0, None),
        None
    );
}

#[test]
fn a_stringtable_key_that_is_missing_is_measured_as_written() {
    let resources = resources_with_font();
    let text = "TextWidgetClass Label {\n size 1 1\n text \"#AB\"\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    let measured = GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .unwrap();
    let expected = block(&resources, "#AB", 12.0, None);
    assert_eq!(
        measured,
        TextSize {
            width: expected.width,
            height: expected.height,
        }
    );
}

#[test]
fn a_stringtable_key_is_measured_as_its_translated_text() {
    let mut resources = resources_with_font();
    resources.texts.insert("ab_key".to_owned(), "AB".to_owned());
    let text =
        "TextWidgetClass Label {\n size 1 1\n text \"#AB_key\"\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    let measured = GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .unwrap();
    let expected = block(&resources, "AB", 12.0, None);
    assert_eq!(
        measured,
        TextSize {
            width: expected.width,
            height: expected.height,
        }
    );
}

fn render_measured(text: &str, resources: &MemoryResources) -> Rendered {
    let widgets = parse(text).unwrap();
    let layout = resolve_with(
        &widgets,
        text,
        Screen {
            width: 200,
            height: 200,
        },
        &GameText::new(resources),
    );
    let options = RenderOptions {
        background: [0, 0, 0, 255],
        show_hidden: false,
    };
    render(&layout, &widgets, text, &options, resources)
}

/// The pixel box `[left, top, right, bottom)` of everything that is not background black.
fn ink_bounds(image: &RgbaImage) -> [u32; 4] {
    let width = image.width as usize;
    let inked: Vec<(u32, u32)> = image
        .pixels
        .chunks(4)
        .enumerate()
        .filter(|(_, pixel)| pixel[..3].iter().any(|&channel| channel > 0))
        .map(|(index, _)| ((index % width) as u32, (index / width) as u32))
        .collect();
    let left = inked.iter().map(|p| p.0).min().expect("no ink");
    let top = inked.iter().map(|p| p.1).min().expect("no ink");
    let right = inked.iter().map(|p| p.0).max().expect("no ink") + 1;
    let bottom = inked.iter().map(|p| p.1).max().expect("no ink") + 1;
    [left, top, right, bottom]
}

fn ink_height(rendered: &Rendered) -> u32 {
    let [_, top, _, bottom] = ink_bounds(&rendered.image);
    bottom - top
}

#[test]
fn a_size_to_text_widget_is_drawn_at_the_size_it_was_measured_with() {
    let resources = resources_with_font();
    let plain = "MultilineTextWidgetClass Label {\n position 0 0\n size 1 0.5\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
    let fitted = "MultilineTextWidgetClass Label {\n position 0 0\n size 1 0.5\n \"size to text v\" 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
    let expected = ink_height(&render_measured(plain, &resources));
    let drawn = ink_height(&render_measured(fitted, &resources));
    assert_eq!(drawn, expected);
}

#[test]
fn a_text_sized_to_its_width_ends_where_its_text_ends() {
    let resources = resources_with_font();
    let text = "TextWidgetClass Label {\n position 0 0\n size 1 0.5\n \"size to text h\" 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
    let widgets = parse(text).unwrap();
    let layout = resolve_with(
        &widgets,
        text,
        Screen {
            width: 200,
            height: 200,
        },
        &GameText::new(&resources),
    );
    let item = &layout.boxes[0];
    let em = item.text_em_px.unwrap();
    let expected = block(&resources, "AB", em, None);
    assert_eq!(item.rect.w, expected.width);
    let rendered = render_measured(text, &resources);
    let [_, _, right, _] = ink_bounds(&rendered.image);
    assert!(right as f32 <= item.rect.x + item.rect.w);
    assert!(item.rect.x + item.rect.w - right as f32 <= em);
}

#[test]
fn a_text_over_the_painter_limit_is_measured_as_the_painter_cuts_it() {
    let resources = resources_with_font();
    let long = format!("{}B", "A".repeat(2000));
    let text = format!(
        "TextWidgetClass Label {{\n size 1 1\n text \"{long}\"\n font \"gui/fonts/Test16\"\n}}"
    );
    let widgets = parse(&text).unwrap();
    let measured = GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .unwrap();
    let expected = block(&resources, &"A".repeat(2000), 12.0, None);
    assert_eq!(measured.width, expected.width);
}

fn family_resources() -> MemoryResources {
    let font = |size: &str, advance: i16| {
        let rendered = RenderedFont {
            size: 6,
            ascent: 6,
            descent: 0,
            glyphs: vec![space(3), glyph(0x41, advance)],
            kerning: vec![],
        };
        build_font(size, &rendered).unwrap().fnt
    };
    let mut resources = MemoryResources::default();
    resources.files.insert(
        "gui/fonts/Fam.xml".to_owned(),
        b"<font><size=\"16\"/><size=\"32\"/></font>".to_vec(),
    );
    resources
        .files
        .insert("gui/fonts/Fam16.fnt".to_owned(), font("Fam16", 5));
    resources
        .files
        .insert("gui/fonts/Fam32.fnt".to_owned(), font("Fam32", 9));
    resources
}

#[test]
fn a_font_family_is_measured_with_the_size_nearest_the_text_size() {
    let resources = family_resources();
    let text = "TextWidgetClass Label {\n size 1 1\n text \"A\"\n font \"gui/fonts/Fam\"\n}";
    let widgets = parse(text).unwrap();
    let measure = GameText::new(&resources);
    let at = |em: f32| measure.measure(&widgets[0], em, None).unwrap().width;
    let big = parse_fnt(&resources.files["gui/fonts/Fam32.fnt"]).unwrap();
    let small = parse_fnt(&resources.files["gui/fonts/Fam16.fnt"]).unwrap();
    let width = |font, em| {
        layout_text(
            font,
            "A",
            &TextOptions {
                em_px: em,
                wrap_width: None,
                halign: HAlign::Left,
            },
        )
        .width
    };
    assert_eq!(at(30.0), width(&big, 30.0));
    assert_eq!(at(18.0), width(&small, 18.0));
    assert_ne!(width(&big, 30.0), width(&small, 30.0));
}

#[test]
fn an_unreadable_font_file_is_not_measured() {
    let mut resources = resources_with_font();
    resources
        .files
        .insert(format!("{FONT}.fnt"), b"not a font".to_vec());
    let text = "TextWidgetClass Label {\n size 1 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n}";
    let widgets = parse(text).unwrap();
    assert!(GameText::new(&resources)
        .measure(&widgets[0], 12.0, None)
        .is_none());
}
