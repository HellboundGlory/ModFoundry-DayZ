//! Draws one box: its picture or style, then its text, with the findings for what is not drawn as the game draws it.

use modfoundry_core::{Diagnostic, Severity};
use modfoundry_layout::geometry::WidgetBox;
use modfoundry_layout::parse::{Value, Widget};
use modfoundry_layout::source::Source;
use modfoundry_layout::styles::StyleImages;

use crate::canvas::Canvas;
use crate::context::DrawContext;
use crate::image_paint::{self, SlotPicture};
use crate::resources::Resources;
use crate::slice_paint;
use crate::text_paint::TextPainter;
use crate::textures::Textures;

const HIDDEN_ALPHA: f32 = 0.35;
const REFERENCE_HEIGHT: f32 = 1080.0;

const SLOTS: [&str; 9] = [
    "Center",
    "Left",
    "Right",
    "Top",
    "Bottom",
    "LeftTop",
    "RightTop",
    "LeftBottom",
    "RightBottom",
];

const OUTSIDE_THE_GAME: [&str; 8] = [
    "ItemPreviewWidgetClass",
    "PlayerPreviewWidgetClass",
    "MapWidgetClass",
    "VideoWidgetClass",
    "RenderTargetWidgetClass",
    "CanvasWidgetClass",
    "HtmlWidgetClass",
    "ServerBrowserWidgetClass",
];

const CONTROLS: [&str; 6] = [
    "TextListboxWidgetClass",
    "XComboBoxWidgetClass",
    "EditBoxWidgetClass",
    "CheckBoxWidgetClass",
    "SliderWidgetClass",
    "ProgressBarWidgetClass",
];

const TEXT: [&str; 3] = [
    "TextWidgetClass",
    "RichTextWidgetClass",
    "MultilineTextWidgetClass",
];

#[derive(Debug, PartialEq)]
enum Treatment {
    Nothing,
    Fill,
    Outline,
    Placeholder,
    Slices(StyleImages),
}

pub(crate) struct Painter<'a> {
    canvas: Canvas,
    source: Source<'a>,
    show_hidden: bool,
    resources: &'a dyn Resources,
    textures: Textures,
    text: TextPainter,
    screen_height: u32,
    findings: Vec<Diagnostic>,
    drawn: usize,
    hidden_skipped: usize,
    slices: usize,
}

impl<'a> Painter<'a> {
    pub(crate) fn new(
        canvas: Canvas,
        text: &'a str,
        show_hidden: bool,
        resources: &'a dyn Resources,
        screen_height: u32,
    ) -> Self {
        Self {
            canvas,
            source: Source::new(text),
            show_hidden,
            resources,
            textures: Textures::default(),
            text: TextPainter::new(),
            screen_height,
            findings: Vec::new(),
            drawn: 0,
            hidden_skipped: 0,
            slices: 0,
        }
    }

    pub(crate) fn paint(&mut self, item: &WidgetBox, widget: &Widget) {
        let Some(factor) = self.visibility_factor(item) else {
            return;
        };
        let mut tint = item.color;
        tint[3] *= factor;
        let mut painted = self.paint_picture(item, widget, tint);
        let outcome = self.text.paint(
            &mut DrawContext {
                canvas: &mut self.canvas,
                resources: self.resources,
                textures: &mut self.textures,
                screen_height: self.screen_height,
            },
            item,
            widget,
            tint,
        );
        self.report(widget, outcome.findings);
        painted |= outcome.painted;
        self.check_rotation(widget);
        if painted {
            self.drawn += 1;
        }
    }

    pub(crate) fn skip_unmatched(&mut self, item: &WidgetBox) {
        self.visibility_factor(item);
    }

    pub(crate) fn finish(self) -> (Canvas, Vec<Diagnostic>, usize, usize) {
        let Self {
            canvas,
            text,
            mut findings,
            drawn,
            hidden_skipped,
            slices,
            ..
        } = self;
        findings.extend(text.finish());
        if slices > 0 {
            let message = format!(
                "styled panel images are drawn at their atlas size times screen height / 1080; the game's size is not measured; {slices} widgets drawn"
            );
            findings.push(Diagnostic::new("MF-L237", Severity::Hint, message));
        }
        (canvas, findings, drawn, hidden_skipped)
    }

    fn context(&mut self) -> DrawContext<'_> {
        DrawContext {
            canvas: &mut self.canvas,
            resources: self.resources,
            textures: &mut self.textures,
            screen_height: self.screen_height,
        }
    }

    fn report(&mut self, widget: &Widget, findings: Vec<(&'static str, String)>) {
        for (code, message) in findings {
            self.finding(code, widget, message);
        }
    }

    fn paint_picture(&mut self, item: &WidgetBox, widget: &Widget, tint: [f32; 4]) -> bool {
        let picture = image_paint::paint(&mut self.context(), item, widget, tint);
        if let Some(picture) = picture {
            self.report(widget, picture.findings);
            return picture.painted;
        }
        self.paint_style(item, widget, tint)
    }

    fn paint_style(&mut self, item: &WidgetBox, widget: &Widget, rgba: [f32; 4]) -> bool {
        match self.treatment(widget) {
            Treatment::Nothing => false,
            Treatment::Fill => self.fill_style(item, widget, rgba),
            Treatment::Outline => self.canvas.outline(item.rect, item.clip, rgba),
            Treatment::Placeholder => self.canvas.placeholder(item.rect, item.clip, rgba),
            Treatment::Slices(images) => self.paint_slices(item, widget, &images, rgba),
        }
    }

    fn fill_style(&mut self, item: &WidgetBox, widget: &Widget, rgba: [f32; 4]) -> bool {
        let rgba = self.styled_tint(widget, rgba);
        self.canvas.fill(item.rect, item.clip, rgba)
    }

    fn paint_slices(
        &mut self,
        item: &WidgetBox,
        widget: &Widget,
        images: &StyleImages,
        rgba: [f32; 4],
    ) -> bool {
        let scale = self.screen_height as f32 / REFERENCE_HEIGHT;
        let tint = self.styled_tint(widget, rgba);
        let loaded = self.load_slots(widget, images);
        let size = |name: &str| {
            loaded
                .iter()
                .find(|(slot, _)| *slot == name)
                .map_or([0.0; 2], |(_, picture)| {
                    picture.size.map(|side| side * scale)
                })
        };
        let mut painted = false;
        for (name, rect) in slice_paint::place(item.rect, size) {
            let Some((_, picture)) = loaded.iter().find(|(slot, _)| *slot == name) else {
                continue;
            };
            painted |=
                slice_paint::draw_slot(&mut self.context(), rect, item.clip, picture, scale, tint);
        }
        if painted {
            self.slices += 1;
        }
        painted
    }

    fn load_slots(
        &mut self,
        widget: &Widget,
        images: &StyleImages,
    ) -> Vec<(&'static str, SlotPicture)> {
        let mut loaded = Vec::new();
        for slot in SLOTS {
            let Some((_, name)) = images.slots.iter().find(|(named, _)| named == slot) else {
                continue;
            };
            let result = image_paint::load_slot(&mut self.context(), &images.imageset, name);
            match result {
                Ok(picture) => loaded.push((slot, picture)),
                Err(reason) => {
                    let style = widget.word("style").unwrap_or_default();
                    let message = format!(
                        "image {name} of style {style} is not drawn: {reason}: {}",
                        widget.label()
                    );
                    self.finding("MF-L214", widget, message);
                }
            }
        }
        loaded
    }

    /// The widget's colour times the style's default colour, unless the widget writes its own `color`.
    fn styled_tint(&self, widget: &Widget, rgba: [f32; 4]) -> [f32; 4] {
        match self.style_default(widget) {
            Some(style) => std::array::from_fn(|at| rgba[at] * style[at]),
            None => rgba,
        }
    }

    fn style_default(&self, widget: &Widget) -> Option<[f32; 4]> {
        if widget
            .properties
            .iter()
            .any(|property| property.key == "color")
        {
            return None;
        }
        let style = widget.word("style")?;
        self.resources
            .style_color(class_name(widget), style)
            .map(argb)
    }

    fn visibility_factor(&mut self, item: &WidgetBox) -> Option<f32> {
        if item.visible {
            Some(1.0)
        } else if self.show_hidden {
            Some(HIDDEN_ALPHA)
        } else {
            self.hidden_skipped += 1;
            None
        }
    }

    fn treatment(&mut self, widget: &Widget) -> Treatment {
        let class = widget.class.as_str();
        if OUTSIDE_THE_GAME.contains(&class) {
            let message = format!(
                "{class} cannot be drawn outside the game: {}",
                widget.label()
            );
            self.finding("MF-L211", widget, message);
            return Treatment::Placeholder;
        }
        if CONTROLS.contains(&class) {
            let message = format!(
                "control drawn as an outline placeholder: {}",
                widget.label()
            );
            self.finding("MF-L212", widget, message);
            return Treatment::Outline;
        }
        if TEXT.contains(&class) {
            return Treatment::Nothing;
        }
        match widget.word("style") {
            None | Some("blank" | "Empty") => Treatment::Nothing,
            Some("rover_sim_colorable" | "Colorable") => Treatment::Fill,
            Some(style) => self.styled_treatment(widget, style),
        }
    }

    fn styled_treatment(&mut self, widget: &Widget, style: &str) -> Treatment {
        match self.resources.style_images(class_name(widget), style) {
            Some(images) if images.slots.is_empty() => Treatment::Nothing,
            Some(images)
                if images
                    .slots
                    .iter()
                    .all(|(slot, _)| SLOTS.contains(&slot.as_str())) =>
            {
                Treatment::Slices(images)
            }
            _ => {
                let message = format!(
                    "style {style} is not drawn, shown as an outline: {}",
                    widget.label()
                );
                self.finding("MF-L210", widget, message);
                Treatment::Outline
            }
        }
    }

    fn check_rotation(&mut self, widget: &Widget) {
        if rotated(widget) {
            let message = format!("rotation is not drawn: {}", widget.label());
            self.finding("MF-L213", widget, message);
        }
    }

    fn finding(&mut self, code: &str, widget: &Widget, message: String) {
        let span = self.source.span_from(&widget.pos);
        self.findings
            .push(Diagnostic::at(code, Severity::Hint, span, message));
    }
}

/// The widget class as the `.styles` files name it: no `Class` suffix.
fn class_name(widget: &Widget) -> &str {
    widget.class.strip_suffix("Class").unwrap_or(&widget.class)
}

/// The text of the first value of `key`, whatever its kind.
pub(crate) fn first_text<'a>(widget: &'a Widget, key: &str) -> Option<&'a str> {
    widget.values(key).first().map(|value| match value {
        Value::Number(text) | Value::Word(text) | Value::Str(text) => text.as_str(),
    })
}

/// Every numeric value of `key`, skipping words and strings.
pub(crate) fn numbers(widget: &Widget, key: &str) -> Vec<f32> {
    widget
        .values(key)
        .iter()
        .filter_map(|value| match value {
            Value::Number(text) => text.parse().ok(),
            Value::Word(_) | Value::Str(_) => None,
        })
        .collect()
}

fn rotated(widget: &Widget) -> bool {
    numbers(widget, "rotation")
        .iter()
        .any(|&angle| angle != 0.0)
}

fn argb(value: u32) -> [f32; 4] {
    let channel = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.0;
    [channel(16), channel(8), channel(0), channel(24)]
}

#[cfg(test)]
mod tests {
    use modfoundry_layout::geometry::{resolve, Screen};
    use modfoundry_layout::parse::parse;
    use modfoundry_layout::styles::StyleImages;

    use crate::{render, MemoryResources, RenderOptions, Rendered};

    const SPACER: &str =
        "WrapSpacerWidgetClass Bar {\n position 0 0\n size 1 1\n style Colorable\n}";
    const BLACK: [u8; 4] = [0, 0, 0, 255];

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
        let options = RenderOptions {
            background: BLACK,
            show_hidden: false,
        };
        render(&layout, &widgets, text, &options, resources)
    }

    fn style_colour(colour: u32) -> MemoryResources {
        let mut resources = MemoryResources::default();
        resources.style_colors.insert(
            ("WrapSpacerWidget".to_string(), "Colorable".to_string()),
            colour,
        );
        resources
    }

    #[test]
    fn a_colorable_spacer_takes_a_transparent_style_colour_and_draws_nothing() {
        let rendered = draw(SPACER, &style_colour(0));
        assert_eq!(rendered.drawn, 0);
        assert!(rendered.findings.is_empty());
        assert!(rendered.image.pixels.chunks(4).all(|pixel| pixel == BLACK));
    }

    #[test]
    fn a_written_colour_overrides_the_style_colour() {
        let text = "WrapSpacerWidgetClass Bar {\n position 0 0\n size 1 1\n style Colorable\n color 1 0 0 1\n}";
        let rendered = draw(text, &style_colour(0));
        assert_eq!(rendered.drawn, 1);
        assert!(rendered
            .image
            .pixels
            .chunks(4)
            .all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn a_colorable_spacer_with_an_opaque_white_style_colour_draws_white() {
        let rendered = draw(SPACER, &style_colour(4_294_967_295));
        assert_eq!(rendered.drawn, 1);
        assert!(rendered
            .image
            .pixels
            .chunks(4)
            .all(|pixel| pixel == [255, 255, 255, 255]));
    }

    #[test]
    fn a_style_missing_from_the_table_draws_opaque_white() {
        let rendered = draw(SPACER, &MemoryResources::default());
        assert_eq!(rendered.drawn, 1);
        assert!(rendered
            .image
            .pixels
            .chunks(4)
            .all(|pixel| pixel == [255, 255, 255, 255]));
    }

    #[test]
    fn a_half_transparent_style_colour_blends_over_the_background() {
        let rendered = draw(SPACER, &style_colour(0x80FF_0000));
        assert_eq!(rendered.drawn, 1);
        let pixel: [u8; 4] = rendered.image.pixels[..4].try_into().unwrap();
        assert!((127..=129).contains(&pixel[0]), "{pixel:?}");
        assert_eq!(pixel[1..3], [0, 0]);
    }

    fn panel(style: &str) -> String {
        format!("PanelWidgetClass Panel {{\n position 0 0\n size 1 1\n style {style}\n}}")
    }

    fn panel_images(slots: &[(&str, &str)]) -> MemoryResources {
        let mut resources = MemoryResources::default();
        resources.style_images.insert(
            ("PanelWidget".to_string(), "DayZDefaultPanel".to_string()),
            StyleImages {
                imageset: "gui".to_string(),
                slots: slots
                    .iter()
                    .map(|(slot, image)| (slot.to_string(), image.to_string()))
                    .collect(),
            },
        );
        resources
    }

    #[test]
    fn a_style_with_no_normal_images_draws_nothing() {
        let rendered = draw(&panel("DayZDefaultPanel"), &panel_images(&[]));
        assert_eq!(rendered.drawn, 0);
        assert!(rendered.findings.is_empty());
        assert!(rendered.image.pixels.chunks(4).all(|pixel| pixel == BLACK));
    }

    #[test]
    fn a_style_with_a_slot_it_does_not_draw_stays_an_outline() {
        let rendered = draw(
            &panel("DayZDefaultPanel"),
            &panel_images(&[("BarCenter", "Bar")]),
        );
        let codes: Vec<&str> = rendered.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["MF-L210"]);
    }
}
