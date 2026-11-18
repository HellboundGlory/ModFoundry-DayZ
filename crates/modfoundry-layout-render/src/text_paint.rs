//! The text hook: draws the text of text widgets and buttons from the game's fonts, with findings for what it cannot draw.

use std::rc::Rc;

use modfoundry_core::{Diagnostic, Severity};
use modfoundry_font::{
    align_offset, draw_text, layout_text, FntFont, HAlign, TextBlock, TextOptions,
};
use modfoundry_layout::geometry::{design_scale, text_em_px, wraps_text, WidgetBox};
use modfoundry_layout::parse::Widget;
use modfoundry_texture::RgbaImage;

use crate::context::DrawContext;
use crate::fonts::{Failure, FontCache};
use crate::paint::{first_text, numbers};
use crate::resources::{normalize_path, Resources};
use crate::textures::FILE_NOT_FOUND;

const TEXT_CLASSES: [&str; 4] = [
    "TextWidgetClass",
    "RichTextWidgetClass",
    "MultilineTextWidgetClass",
    "ButtonWidgetClass",
];

const MAX_TEXT_CHARS: usize = 2000;

struct Loaded {
    font: Rc<FntFont>,
    atlas: Rc<RgbaImage>,
}

pub(crate) struct TextPainter {
    fonts: FontCache,
    approximated: usize,
}

pub(crate) struct TextOutcome {
    pub(crate) findings: Vec<(&'static str, String)>,
    pub(crate) painted: bool,
}

impl TextPainter {
    pub(crate) fn new() -> Self {
        Self {
            fonts: FontCache::default(),
            approximated: 0,
        }
    }

    pub(crate) fn paint(
        &mut self,
        ctx: &mut DrawContext<'_>,
        item: &WidgetBox,
        widget: &Widget,
        tint: [f32; 4],
    ) -> TextOutcome {
        let mut outcome = TextOutcome {
            findings: Vec::new(),
            painted: false,
        };
        let Some(raw) = text_of(widget) else {
            return outcome;
        };
        let text = stringtable_text(ctx.resources, raw, widget, &mut outcome.findings);
        let text = cut_long(text, widget, &mut outcome.findings);
        rich_markup_finding(widget, &text, &mut outcome.findings);
        let Some(bounds) = ctx.canvas.pixel_bounds(item.rect, item.clip) else {
            return outcome;
        };
        let colour = text_colour(widget, tint);
        let em_px = item
            .text_em_px
            .unwrap_or_else(|| text_em_px(widget, item.rect.h, ctx.screen_height));
        let loaded = match self.font_for(ctx, widget, em_px, &mut outcome.findings) {
            Ok(loaded) => loaded,
            Err(finding) => {
                outcome.findings.push(finding);
                outcome.painted = ctx.canvas.outline(item.rect, item.clip, colour);
                return outcome;
            }
        };
        let wrap_width = wraps_text(widget).then_some(item.rect.w);
        let options = TextOptions {
            em_px,
            wrap_width,
            halign: halign(widget),
        };
        let block = layout_text(&loaded.font, &text, &options);
        let origin = origin(
            item,
            widget,
            ctx.screen_height,
            &block,
            wrap_width.is_some(),
        );
        draw_text(
            ctx.canvas.image_mut(),
            &loaded.atlas,
            &loaded.font,
            &block,
            origin,
            colour,
            Some(bounds),
        );
        self.approximated += 1;
        outcome.painted = touches(&block, origin, bounds);
        outcome
    }

    fn font_for(
        &mut self,
        ctx: &mut DrawContext<'_>,
        widget: &Widget,
        em_px: f32,
        findings: &mut Vec<(&'static str, String)>,
    ) -> Result<Loaded, (&'static str, String)> {
        match font_name(ctx.resources, widget, findings).map(|name| normalize_path(&name)) {
            Some(name) => self
                .load(ctx, &name, em_px)
                .map_err(|failure| font_finding(&name, &failure, widget)),
            None => Err(font_finding(
                &format!("Normal style of {}", style_class(widget)),
                &Failure::NotFound,
                widget,
            )),
        }
    }

    fn load(
        &mut self,
        ctx: &mut DrawContext<'_>,
        name: &str,
        em_px: f32,
    ) -> Result<Loaded, Failure> {
        let (stem, font) = self.fonts.load(ctx.resources, name, em_px)?;
        let atlas = ctx
            .textures
            .get(ctx.resources, &format!("{stem}.edds"))
            .map_err(|error| match error.as_str() {
                FILE_NOT_FOUND => Failure::NotFound,
                _ => Failure::Unreadable(error),
            })?;
        Ok(Loaded { font, atlas })
    }

    pub(crate) fn finish(self) -> Vec<Diagnostic> {
        if self.approximated == 0 {
            return Vec::new();
        }
        let message = format!(
            "text size and placement are approximated; {} text widgets drawn",
            self.approximated
        );
        vec![Diagnostic::new("MF-L235", Severity::Hint, message)]
    }
}

/// Whether any glyph of the block lands inside the pixel `bounds`.
fn touches(block: &TextBlock, origin: (f32, f32), bounds: [i32; 4]) -> bool {
    block.glyphs.iter().any(|glyph| {
        glyph.w > 0.0
            && glyph.h > 0.0
            && origin.0 + glyph.x + glyph.w > bounds[0] as f32
            && origin.0 + glyph.x < bounds[2] as f32
            && origin.1 + glyph.y + glyph.h > bounds[1] as f32
            && origin.1 + glyph.y < bounds[3] as f32
    })
}

/// Text past `MAX_TEXT_CHARS` is cut, so one widget cannot ask for unbounded drawing.
pub(crate) fn cut_long(
    text: String,
    widget: &Widget,
    findings: &mut Vec<(&'static str, String)>,
) -> String {
    let Some((end, _)) = text.char_indices().nth(MAX_TEXT_CHARS) else {
        return text;
    };
    let message = format!(
        "text longer than {MAX_TEXT_CHARS} characters is cut: {}",
        widget.label()
    );
    findings.push(("MF-L236", message));
    text[..end].to_owned()
}

fn rich_markup_finding(widget: &Widget, text: &str, findings: &mut Vec<(&'static str, String)>) {
    if widget.class == "RichTextWidgetClass" && text.contains('<') {
        let message = format!(
            "rich text markup is drawn as plain text: {}",
            widget.label()
        );
        findings.push(("MF-L234", message));
    }
}

/// Where the block's top-left goes; a wrapped block already aligns its lines inside the box.
fn origin(
    item: &WidgetBox,
    widget: &Widget,
    screen_height: u32,
    block: &TextBlock,
    wrapped: bool,
) -> (f32, f32) {
    let block_x = if wrapped {
        0.0
    } else {
        align_offset(halign(widget), item.rect.w, block.width)
    };
    let valign = valign_offset(widget, item.rect.h, block.height);
    let [offset_x, offset_y] = text_offset(widget, screen_height);
    (
        item.rect.x + block_x + offset_x,
        item.rect.y + valign + offset_y,
    )
}

fn text_colour(widget: &Widget, tint: [f32; 4]) -> [f32; 4] {
    match numbers(widget, "text color").as_slice() {
        [red, green, blue, ..] => [*red, *green, *blue, tint[3]],
        _ => tint,
    }
}

fn halign(widget: &Widget) -> HAlign {
    match first_text(widget, "text halign") {
        Some("center") => HAlign::Center,
        Some("right") => HAlign::Right,
        _ => HAlign::Left,
    }
}

fn text_offset(widget: &Widget, screen_height: u32) -> [f32; 2] {
    let scale = design_scale(screen_height);
    match numbers(widget, "text_offset").as_slice() {
        [x, y, ..] => [x * scale, y * scale],
        _ => [0.0, 0.0],
    }
}

fn valign_offset(widget: &Widget, box_height: f32, block_height: f32) -> f32 {
    match first_text(widget, "text valign") {
        Some("top") => 0.0,
        Some("bottom") => box_height - block_height,
        _ => (box_height - block_height) / 2.0,
    }
}

fn style_class(widget: &Widget) -> &str {
    widget.class.strip_suffix("Class").unwrap_or(&widget.class)
}

fn font_finding(name: &str, failure: &Failure, widget: &Widget) -> (&'static str, String) {
    match failure {
        Failure::NotFound => (
            "MF-L231",
            format!(
                "font {name} was not found, shown as an outline: {}",
                widget.label()
            ),
        ),
        Failure::Unreadable(error) => (
            "MF-L232",
            format!(
                "font {name} cannot be read ({error}), shown as an outline: {}",
                widget.label()
            ),
        ),
    }
}

/// The style names the game's classes list first; text, multiline and button widgets all map them to MetronBook.
const DEFAULT_STYLES: [&str; 3] = ["Normal", "DayZNormal", "Default"];

pub(crate) fn font_name(
    resources: &dyn Resources,
    widget: &Widget,
    findings: &mut Vec<(&'static str, String)>,
) -> Option<String> {
    if let Some(name) = widget.word("font").filter(|name| !name.is_empty()) {
        return Some(name.to_owned());
    }
    let class = style_class(widget);
    if let Some(style) = widget.word("style").filter(|style| !style.is_empty()) {
        if let Some(name) = resources.font_style(class, style) {
            return Some(name);
        }
    }
    let (style, font) = DEFAULT_STYLES
        .iter()
        .find_map(|style| Some((*style, resources.font_style(class, style)?)))?;
    let message = format!(
        "no font named, drawn with the {style} style font: {}",
        widget.label()
    );
    findings.push(("MF-L233", message));
    Some(font)
}

pub(crate) fn stringtable_text(
    resources: &dyn Resources,
    raw: &str,
    widget: &Widget,
    findings: &mut Vec<(&'static str, String)>,
) -> String {
    let Some(key) = raw.strip_prefix('#') else {
        return raw.to_owned();
    };
    resources.text(key).unwrap_or_else(|| {
        let message = format!(
            "text key {raw} is not in the stringtable, drawn as written: {}",
            widget.label()
        );
        findings.push(("MF-L230", message));
        raw.to_owned()
    })
}

pub(crate) fn text_of(widget: &Widget) -> Option<&str> {
    if !TEXT_CLASSES.contains(&widget.class.as_str()) {
        return None;
    }
    widget.word("text").filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use modfoundry_core::Diagnostic;
    use modfoundry_font::{
        build_font, layout_text, parse_fnt, Glyph, HAlign, RenderedFont, TextBlock, TextOptions,
    };
    use modfoundry_layout::geometry::{resolve, Screen};
    use modfoundry_layout::parse::parse;
    use modfoundry_texture::RgbaImage;

    use std::cell::Cell;

    use crate::resources::{normalize_path, MemoryResources, Resources};
    use crate::{render, RenderOptions, Rendered};

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

    fn content_findings(rendered: &Rendered) -> Vec<&Diagnostic> {
        rendered
            .findings
            .iter()
            .filter(|finding| finding.code != "MF-L235")
            .collect()
    }

    fn draw(text: &str, width: u32, height: u32, resources: &MemoryResources) -> Rendered {
        draw_with(text, width, height, resources)
    }

    fn draw_with(text: &str, width: u32, height: u32, resources: &dyn Resources) -> Rendered {
        let widgets = parse(text).unwrap();
        let layout = resolve(&widgets, text, Screen { width, height });
        let options = RenderOptions {
            background: [0, 0, 0, 255],
            show_hidden: false,
        };
        render(&layout, &widgets, text, &options, resources)
    }

    fn ink_bounds(image: &RgbaImage) -> Option<[u32; 4]> {
        let width = image.width as usize;
        let inked: Vec<(u32, u32)> = image
            .pixels
            .chunks(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[..3].iter().any(|&channel| channel > 0))
            .map(|(index, _)| ((index % width) as u32, (index / width) as u32))
            .collect();
        let x0 = inked.iter().map(|p| p.0).min()?;
        let y0 = inked.iter().map(|p| p.1).min()?;
        let x1 = inked.iter().map(|p| p.0).max()? + 1;
        let y1 = inked.iter().map(|p| p.1).max()? + 1;
        Some([x0, y0, x1, y1])
    }

    #[test]
    fn literal_text_is_drawn_in_its_box() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert!(ink_bounds(&rendered.image).is_some(), "no ink");
        assert_eq!(rendered.drawn, 1);
    }

    fn label_with_text(text: &str) -> String {
        format!("TextWidgetClass Label {{\n position 0 0\n size 1 1\n text \"{text}\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}}")
    }

    #[test]
    fn a_stringtable_key_draws_its_text() {
        let mut resources = resources_with_font();
        resources.texts.insert("title".to_owned(), "AB".to_owned());
        let keyed = draw(&label_with_text("#title"), 200, 100, &resources);
        let literal = draw(&label_with_text("AB"), 200, 100, &resources);
        assert!(
            keyed.image.pixels == literal.image.pixels,
            "keyed text differs from the literal text"
        );
    }

    #[test]
    fn a_missing_stringtable_key_is_reported_and_drawn_as_written() {
        let rendered = draw(&label_with_text("#AB"), 200, 100, &resources_with_font());
        let findings = content_findings(&rendered);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L230");
        assert_eq!(
            finding.message,
            "text key #AB is not in the stringtable, drawn as written: Label"
        );
    }

    #[test]
    fn a_style_names_the_font_when_no_font_is_given() {
        let mut resources = resources_with_font();
        resources.font_styles.insert(
            ("TextWidget".to_owned(), "Body".to_owned()),
            FONT.to_owned(),
        );
        let styled = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n style Body\n color 1 1 1 1\n}";
        let by_style = draw(styled, 200, 100, &resources);
        let by_font = draw(&label_with_text("AB"), 200, 100, &resources);
        assert!(
            by_style.image.pixels == by_font.image.pixels,
            "style font differs from the font property"
        );
    }

    #[test]
    fn a_text_without_a_font_uses_the_normal_style_with_a_finding() {
        let mut resources = resources_with_font();
        resources.font_styles.insert(
            ("TextWidget".to_owned(), "Normal".to_owned()),
            FONT.to_owned(),
        );
        let unnamed =
            "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n color 1 1 1 1\n}";
        let normal = draw(unnamed, 200, 100, &resources);
        let by_font = draw(&label_with_text("AB"), 200, 100, &resources);
        assert!(
            normal.image.pixels == by_font.image.pixels,
            "Normal style font differs from the font property"
        );
        let findings = content_findings(&normal);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", normal.findings.len());
        };
        assert_eq!(finding.code, "MF-L233");
        assert_eq!(
            finding.message,
            "no font named, drawn with the Normal style font: Label"
        );
    }

    #[test]
    fn a_class_without_a_normal_style_falls_back_to_its_default_named_one() {
        let mut resources = resources_with_font();
        resources.font_styles.insert(
            ("ButtonWidget".to_owned(), "Default".to_owned()),
            FONT.to_owned(),
        );
        let unnamed =
            "ButtonWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n color 1 1 1 1\n}";
        let rendered = draw(unnamed, 200, 100, &resources);
        let findings = content_findings(&rendered);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L233");
        assert_eq!(
            finding.message,
            "no font named, drawn with the Default style font: Label"
        );
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_missing_font_is_an_outline_with_a_finding() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n font \"gui/fonts/Missing\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert_eq!(pixel(&rendered.image, 0, 50), [255, 255, 255, 255]);
        assert_eq!(pixel(&rendered.image, 100, 50), [0, 0, 0, 255]);
        let [finding] = rendered.findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L231");
        assert_eq!(
            finding.message,
            "font gui/fonts/Missing was not found, shown as an outline: Label"
        );
    }

    #[test]
    fn an_unreadable_font_file_is_an_outline_with_its_error() {
        let mut resources = resources_with_font();
        resources.files.insert(
            "gui/fonts/Bad16.fnt".to_owned(),
            b"not a font file at all".to_vec(),
        );
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n font \"gui/fonts/Bad16\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources);
        assert_eq!(pixel(&rendered.image, 0, 50), [255, 255, 255, 255]);
        let [finding] = rendered.findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L232");
        assert_eq!(
            finding.message,
            "font gui/fonts/Bad16 cannot be read (the font file does not start with FORM), shown as an outline: Label"
        );
    }

    #[test]
    fn a_compressed_atlas_is_an_outline_with_its_error() {
        let built = build_font(
            "Test16",
            &RenderedFont {
                size: 6,
                ascent: 6,
                descent: 0,
                glyphs: vec![space(3), glyph(0x41, 5), glyph(0x42, 5)],
                kerning: vec![],
            },
        )
        .unwrap();
        let mut atlas = built.texture;
        atlas[0x54..0x58].copy_from_slice(b"DXT3");
        let mut resources = MemoryResources::default();
        resources.files.insert(format!("{FONT}.fnt"), built.fnt);
        resources.files.insert(format!("{FONT}.edds"), atlas);
        let rendered = draw(&label_with_text("AB"), 200, 100, &resources);
        assert_eq!(pixel(&rendered.image, 0, 50), [255, 255, 255, 255]);
        let [finding] = rendered.findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L232");
        assert!(finding
            .message
            .starts_with(&format!("font {FONT} cannot be read (")));
        assert!(finding.message.contains("unsupported .edds pixel format"));
    }

    fn insert_font(resources: &mut MemoryResources, stem: &str, glyph_width: u16) {
        let a = Glyph {
            codepoint: 0x41,
            width: glyph_width,
            height: 6,
            coverage: vec![255; glyph_width as usize * 6],
            left: 0,
            top: 6,
            advance: glyph_width as i16 + 1,
        };
        let font = RenderedFont {
            size: 6,
            ascent: 6,
            descent: 0,
            glyphs: vec![space(3), a],
            kerning: vec![],
        };
        let built = build_font(stem, &font).unwrap();
        resources.files.insert(format!("{stem}.fnt"), built.fnt);
        resources
            .files
            .insert(format!("{stem}.edds"), built.texture);
    }

    #[test]
    fn a_family_name_picks_the_listed_size_nearest_the_text_size_with_ties_up() {
        let mut resources = MemoryResources::default();
        insert_font(&mut resources, "gui/fonts/Fam16", 4);
        insert_font(&mut resources, "gui/fonts/Fam24", 8);
        resources.files.insert(
            "gui/fonts/Fam.xml".to_owned(),
            br#"<Fonts><font size="16"/><font size="24"/></Fonts>"#.to_vec(),
        );
        let widget = |font: &str| {
            format!("TextWidgetClass Label {{\n position 0 0\n size 1 0.25\n text \"A\"\n font \"{font}\"\n color 1 1 1 1\n}}")
        };
        let family = draw(&widget("gui/fonts/Fam"), 100, 100, &resources);
        let sized = draw(&widget("gui/fonts/Fam24"), 100, 100, &resources);
        assert!(
            family.image.pixels == sized.image.pixels,
            "family did not pick the size 24 font"
        );
        assert!(content_findings(&family).is_empty());
    }

    fn ink_height(rendered: &Rendered) -> u32 {
        let [_, y0, _, y1] = ink_bounds(&rendered.image).expect("no ink");
        y1 - y0
    }

    #[test]
    fn exact_text_size_is_scaled_by_the_screen_height() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"exact text\" 1\n \"exact text size\" 20\n color 1 1 1 1\n}";
        let full = draw(text, 200, 1080, &resources_with_font());
        let half = draw(text, 200, 540, &resources_with_font());
        assert!(
            (ink_height(&full) as i32 - 20).abs() <= 1,
            "1080 high: {}",
            ink_height(&full)
        );
        assert!(
            (ink_height(&half) as i32 - 10).abs() <= 1,
            "540 high: {}",
            ink_height(&half)
        );
    }

    #[test]
    fn text_size_is_the_box_height_times_the_text_proportion() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"text_proportion\" 0.5\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert!(
            (ink_height(&rendered) as i32 - 50).abs() <= 1,
            "ink height {}",
            ink_height(&rendered)
        );
    }

    fn layout_block(resources: &MemoryResources, text: &str, em_px: f32) -> TextBlock {
        let font = parse_fnt(&resources.files[&format!("{FONT}.fnt")]).unwrap();
        let options = TextOptions {
            em_px,
            wrap_width: None,
            halign: HAlign::Left,
        };
        layout_text(&font, text, &options)
    }

    #[test]
    fn horizontal_alignment_places_the_text_block_in_its_box() {
        let resources = resources_with_font();
        let width = layout_block(&resources, "A", 80.0).width;
        let cases = [
            ("left", 0.0),
            ("center", (200.0 - width) / 2.0),
            ("right", 200.0 - width),
        ];
        for (halign, left) in cases {
            let text = format!("TextWidgetClass Label {{\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"text halign\" {halign}\n color 1 1 1 1\n}}");
            let rendered = draw(&text, 200, 100, &resources);
            let [x0, ..] = ink_bounds(&rendered.image).expect("no ink");
            assert!(
                (x0 as f32 - left).abs() <= 1.0,
                "{halign}: ink starts at {x0}, expected {left}"
            );
        }
    }

    #[test]
    fn vertical_alignment_places_the_text_block_in_its_box() {
        let resources = resources_with_font();
        let height = layout_block(&resources, "A", 80.0).height;
        let cases = [
            ("top", 0.0),
            ("center", (100.0 - height) / 2.0),
            ("bottom", 100.0 - height),
        ];
        for (valign, top) in cases {
            let text = format!("TextWidgetClass Label {{\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"text valign\" {valign}\n color 1 1 1 1\n}}");
            let rendered = draw(&text, 200, 100, &resources);
            let [_, y0, _, _] = ink_bounds(&rendered.image).expect("no ink");
            assert!(
                (y0 as f32 - top).abs() <= 1.0,
                "{valign}: ink starts at {y0}, expected {top}"
            );
        }
    }

    #[test]
    fn a_multiline_text_wraps_at_the_box_width() {
        let resources = resources_with_font();
        let text = |class: &str| {
            format!("{class} Label {{\n position 0 0\n size 1 1\n text \"A A A\"\n font \"gui/fonts/Test16\"\n \"text_proportion\" 0.5\n color 1 1 1 1\n}}")
        };
        let wrapped = draw(&text("MultilineTextWidgetClass"), 500, 400, &resources);
        let single = draw(&text("TextWidgetClass"), 500, 400, &resources);
        assert!(
            (ink_height(&wrapped) as i32 - 2 * ink_height(&single) as i32).abs() <= 2,
            "wrapped {} against one line {}",
            ink_height(&wrapped),
            ink_height(&single)
        );
    }

    #[test]
    fn no_wrap_keeps_a_multiline_text_on_one_line() {
        let resources = resources_with_font();
        let text = "MultilineTextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A A A\"\n font \"gui/fonts/Test16\"\n \"text_proportion\" 0.5\n \"no wrap\" 1\n color 1 1 1 1\n}";
        let single = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A A A\"\n font \"gui/fonts/Test16\"\n \"text_proportion\" 0.5\n color 1 1 1 1\n}";
        assert_eq!(
            ink_height(&draw(text, 500, 400, &resources)),
            ink_height(&draw(single, 500, 400, &resources))
        );
    }

    #[test]
    fn text_color_replaces_the_widget_colour_for_rgb() {
        let resources = resources_with_font();
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"text color\" 1 0 0 1\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources);
        assert_eq!(pixel(&rendered.image, 20, 40), [255, 0, 0, 255]);
    }

    #[test]
    fn hidden_text_is_drawn_faded_when_hidden_widgets_are_shown() {
        let resources = resources_with_font();
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n visible 0\n color 1 1 1 1\n}";
        let widgets = parse(text).unwrap();
        let layout = resolve(
            &widgets,
            text,
            Screen {
                width: 200,
                height: 100,
            },
        );
        let options = RenderOptions {
            background: [0, 0, 0, 255],
            show_hidden: true,
        };
        let rendered = render(&layout, &widgets, text, &options, &resources);
        assert_eq!(pixel(&rendered.image, 20, 40), [89, 89, 89, 255]);
    }

    #[test]
    fn text_is_cut_to_a_clipping_parent() {
        let resources = resources_with_font();
        let text = "FrameWidgetClass Parent {\n clipchildren 1\n position 0 0\n size 0.5 0.5\n {\n TextWidgetClass Label {\n  position 0 0\n  size 1 1\n  text \"A\"\n  font \"gui/fonts/Test16\"\n  color 1 1 1 1\n }\n }\n}";
        let rendered = draw(text, 200, 100, &resources);
        let [_, _, right, bottom] = ink_bounds(&rendered.image).expect("no ink");
        assert!(
            right <= 100,
            "ink ends at {right}, past the parent's 100 px"
        );
        assert!(
            bottom <= 50,
            "ink ends at {bottom}, past the parent's 50 px"
        );
    }

    #[test]
    fn rich_text_markup_is_drawn_as_plain_text_with_a_finding() {
        let resources = resources_with_font();
        let text = "RichTextWidgetClass Label {\n position 0 0\n size 1 1\n text \"<b>A</b>\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources);
        let findings = content_findings(&rendered);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L234");
        assert_eq!(
            finding.message,
            "rich text markup is drawn as plain text: Label"
        );
    }

    #[test]
    fn approximated_text_is_reported_once_with_the_widget_count() {
        let resources = resources_with_font();
        let widget = |name: &str| {
            format!("TextWidgetClass {name} {{\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}}")
        };
        let text = [widget("First"), widget("Second"), widget("Third")].join("\n");
        let rendered = draw(&text, 200, 100, &resources);
        let [finding] = rendered.findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L235");
        assert_eq!(
            finding.message,
            "text size and placement are approximated; 3 text widgets drawn"
        );
    }

    #[test]
    fn a_text_with_no_font_at_all_is_an_outline_with_a_finding() {
        let text =
            "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"AB\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert_eq!(pixel(&rendered.image, 0, 50), [255, 255, 255, 255]);
        let findings = content_findings(&rendered);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", findings.len());
        };
        assert_eq!(finding.code, "MF-L231");
        assert_eq!(
            finding.message,
            "font Normal style of TextWidget was not found, shown as an outline: Label"
        );
    }

    #[test]
    fn text_offset_moves_the_text_in_exact_units() {
        let resources = resources_with_font();
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n text \"A\"\n font \"gui/fonts/Test16\"\n \"exact text\" 1\n \"exact text size\" 20\n \"text_offset\" 20 0\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 1080, &resources);
        let [x0, ..] = ink_bounds(&rendered.image).expect("no ink");
        assert!((x0 as i32 - 20).abs() <= 1, "ink starts at {x0}");
    }

    #[test]
    fn no_approximation_finding_without_drawn_text() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 1 1\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert!(rendered.findings.is_empty());
    }

    #[test]
    fn a_button_caption_is_drawn_from_its_text() {
        let text = "ButtonWidgetClass Button {\n position 0 0\n size 1 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert!(ink_bounds(&rendered.image).is_some(), "no ink");
    }
    struct Strict(MemoryResources);

    impl Resources for Strict {
        fn imageset(
            &self,
            name: &str,
        ) -> Option<std::rc::Rc<modfoundry_layout::imageset::Imageset>> {
            self.0.imageset(name)
        }
        fn file(&self, path: &str) -> Option<Vec<u8>> {
            (path == normalize_path(path))
                .then(|| self.0.file(path))
                .flatten()
        }
        fn text(&self, key: &str) -> Option<String> {
            self.0.text(key)
        }
        fn font_style(&self, class: &str, style: &str) -> Option<String> {
            self.0.font_style(class, style)
        }

        fn style_color(&self, class: &str, style: &str) -> Option<u32> {
            self.0.style_color(class, style)
        }
    }

    #[test]
    fn a_font_path_is_normalised_before_the_port_is_asked() {
        let strict = Strict(resources_with_font());
        for written in [
            "\\gui\\fonts\\Test16",
            "{ABCDEF}gui/fonts/Test16",
            "/gui/fonts/Test16",
        ] {
            let text = format!("TextWidgetClass Label {{\n position 0 0\n size 1 1\n text \"AB\"\n font \"{written}\"\n color 1 1 1 1\n}}");
            let rendered = draw_with(&text, 200, 100, &strict);
            assert!(
                content_findings(&rendered).is_empty(),
                "{written}: {:?}",
                rendered.findings
            );
            assert!(ink_bounds(&rendered.image).is_some(), "{written}");
        }
    }

    struct Counting {
        inner: MemoryResources,
        font_reads: Cell<usize>,
    }

    impl Resources for Counting {
        fn imageset(
            &self,
            name: &str,
        ) -> Option<std::rc::Rc<modfoundry_layout::imageset::Imageset>> {
            self.inner.imageset(name)
        }
        fn file(&self, path: &str) -> Option<Vec<u8>> {
            if path.ends_with(".fnt") || path.ends_with(".xml") {
                self.font_reads.set(self.font_reads.get() + 1);
            }
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

    #[test]
    fn a_font_is_looked_up_once_for_many_widgets() {
        let counting = Counting {
            inner: resources_with_font(),
            font_reads: Cell::new(0),
        };
        let one = "TextWidgetClass Label {\n size 1 0.3\n text \"AB\"\n font \"gui/fonts/Test16\"\n color 1 1 1 1\n}\n";
        let rendered = draw_with(&one.repeat(5), 200, 100, &counting);
        assert_eq!(rendered.drawn, 5);
        assert!(
            counting.font_reads.get() <= 2,
            "{} reads",
            counting.font_reads.get()
        );
    }

    #[test]
    fn text_pushed_wholly_outside_its_box_is_not_counted_as_drawn() {
        let text = "TextWidgetClass Label {\n position 0 0\n size 0.5 1\n text \"AB\"\n font \"gui/fonts/Test16\"\n text_offset 5000 0\n color 1 1 1 1\n}";
        let rendered = draw(text, 200, 100, &resources_with_font());
        assert_eq!(rendered.drawn, 0);
    }

    #[test]
    fn a_very_long_text_is_cut_with_a_finding() {
        let long = "A".repeat(3000);
        let rendered = draw(&label_with_text(&long), 200, 100, &resources_with_font());
        let findings = content_findings(&rendered);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {}", rendered.findings.len());
        };
        assert_eq!(finding.code, "MF-L236");
        assert_eq!(
            finding.message,
            "text longer than 2000 characters is cut: Label"
        );
    }
}
