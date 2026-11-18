//! Draws a resolved DayZ `.layout` onto an RGBA picture, with findings for what is not drawn as the game draws it.

mod canvas;
mod context;
mod fonts;
mod image_paint;
mod paint;
mod resources;
mod slice_paint;
mod text_measure;
mod text_paint;
mod textures;

pub use resources::{normalize_path, MemoryResources, NoResources, Resources};
pub use text_measure::GameText;

use std::collections::HashMap;

use modfoundry_core::Diagnostic;
use modfoundry_layout::geometry::{Layout, WidgetBox};
use modfoundry_layout::parse::Widget;
use modfoundry_texture::RgbaImage;

use canvas::Canvas;
use paint::Painter;

/// How a layout is drawn: the backdrop and whether hidden widgets are drawn faded.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    pub background: [u8; 4],
    pub show_hidden: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            background: [32, 32, 32, 255],
            show_hidden: false,
        }
    }
}

/// A drawn layout with what was drawn and what was not.
#[derive(Debug)]
pub struct Rendered {
    pub image: RgbaImage,
    pub findings: Vec<Diagnostic>,
    pub drawn: usize,
    pub hidden_skipped: usize,
}

/// Draws the boxes of `layout` in paint order onto a picture the size of its screen.
pub fn render(
    layout: &Layout,
    widgets: &[Widget],
    text: &str,
    options: &RenderOptions,
    resources: &dyn Resources,
) -> Rendered {
    let canvas = Canvas::new(
        layout.screen.width,
        layout.screen.height,
        options.background,
    );
    let mut painter = Painter::new(
        canvas,
        text,
        options.show_hidden,
        resources,
        layout.screen.height,
    );
    let by_position = index_by_position(widgets);
    let mut boxes: Vec<&WidgetBox> = layout.boxes.iter().collect();
    boxes.sort_by_key(|item| item.draw_order);
    for item in boxes {
        match by_position.get(&(item.line, item.column)) {
            Some(widget) => painter.paint(item, widget),
            None => painter.skip_unmatched(item),
        }
    }
    let (canvas, findings, drawn, hidden_skipped) = painter.finish();
    Rendered {
        image: canvas.into_image(),
        findings,
        drawn,
        hidden_skipped,
    }
}

fn index_by_position(widgets: &[Widget]) -> HashMap<(u32, u32), &Widget> {
    let mut index = HashMap::new();
    let mut pending: Vec<&Widget> = widgets.iter().collect();
    while let Some(widget) = pending.pop() {
        index.insert((widget.pos.line, widget.pos.column), widget);
        pending.extend(widget.children.iter());
    }
    index
}

#[cfg(test)]
mod tests {
    use modfoundry_layout::geometry::Screen;
    use modfoundry_layout::parse::parse;

    use super::*;

    fn draw(text: &str, width: u32, height: u32, options: &RenderOptions) -> Rendered {
        let widgets = parse(text).unwrap();
        let layout = modfoundry_layout::geometry::resolve(&widgets, text, Screen { width, height });
        render(&layout, &widgets, text, options, &NoResources)
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn flat_panel_fills_its_box_with_its_colour() {
        let text = "PanelWidgetClass Panel {\n position 0 0\n size 0.5 0.5\n style rover_sim_colorable\n color 1 0 0 1\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 50, 50), [255, 0, 0, 255]);
        assert_eq!(pixel(&rendered.image, 150, 150), [32, 32, 32, 255]);
    }

    #[test]
    fn higher_priority_panel_covers_the_earlier_one() {
        let text = "PanelWidgetClass High {\n position 0 0\n size 0.5 0.5\n priority 2\n style rover_sim_colorable\n color 1 0 0 1\n}\nPanelWidgetClass Low {\n position 0 0\n size 1 1\n priority 1\n style rover_sim_colorable\n color 0 0 1 1\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 10, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(&rendered.image, 150, 150), [0, 0, 255, 255]);
    }

    #[test]
    fn clipchildren_cuts_a_child_to_its_parent() {
        let text = "FrameWidgetClass Parent {\n clipchildren 1\n position 0 0\n size 0.5 0.5\n {\n PanelWidgetClass Child {\n  position 0.5 0.5\n  size 1 1\n  style rover_sim_colorable\n  color 0 1 0 1\n }\n }\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 60, 60), [0, 255, 0, 255]);
        assert_eq!(pixel(&rendered.image, 120, 120), [32, 32, 32, 255]);
    }

    #[test]
    fn hidden_widget_is_skipped_and_counted() {
        let text = "PanelWidgetClass Panel {\n position 0 0\n size 1 1\n visible 0\n style rover_sim_colorable\n color 1 0 0 1\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 50, 50), [32, 32, 32, 255]);
        assert_eq!(rendered.hidden_skipped, 1);
    }

    #[test]
    fn show_hidden_draws_hidden_widgets_faded() {
        let text = "PanelWidgetClass Panel {\n position 0 0\n size 1 1\n visible 0\n style rover_sim_colorable\n color 1 1 1 1\n}";
        let options = RenderOptions {
            background: [0, 0, 0, 255],
            show_hidden: true,
        };
        let rendered = draw(text, 200, 200, &options);
        assert_eq!(pixel(&rendered.image, 50, 50), [89, 89, 89, 255]);
        assert_eq!(rendered.hidden_skipped, 0);
    }

    #[test]
    fn blank_and_unstyled_panels_draw_nothing() {
        let text = "PanelWidgetClass Blank {\n position 0 0\n size 0.5 1\n style blank\n color 1 0 0 1\n}\nPanelWidgetClass NoStyle {\n position 0.5 0\n size 0.5 1\n color 0 1 0 1\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 50, 50), [32, 32, 32, 255]);
        assert_eq!(pixel(&rendered.image, 150, 50), [32, 32, 32, 255]);
        assert_eq!(rendered.drawn, 0);
        assert!(rendered.findings.is_empty());
    }

    #[test]
    fn unknown_style_is_an_outline_with_a_finding() {
        let text =
            "PanelWidgetClass Odd {\n position 0 0\n size 1 1\n style mystery\n color 1 0 0 1\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 0, 100), [255, 0, 0, 255]);
        assert_eq!(pixel(&rendered.image, 100, 100), [32, 32, 32, 255]);
        assert_eq!(rendered.findings.len(), 1);
        let finding = &rendered.findings[0];
        assert_eq!(finding.code, "MF-L210");
        assert_eq!(
            finding.message,
            "style mystery is not drawn, shown as an outline: Odd"
        );
        assert_eq!(finding.span.map(|span| span.start), Some(0));
    }

    #[test]
    fn outside_the_game_classes_are_outlined_and_crossed() {
        for class in [
            "MapWidgetClass",
            "CanvasWidgetClass",
            "ServerBrowserWidgetClass",
        ] {
            let text = format!("{class} Shot {{\n position 0 0\n size 1 1\n style rover_sim_colorable\n color 1 0 0 1\n}}");
            let rendered = draw(&text, 200, 200, &RenderOptions::default());
            assert_eq!(
                pixel(&rendered.image, 0, 100),
                [255, 0, 0, 255],
                "{class} outline"
            );
            assert_eq!(
                pixel(&rendered.image, 100, 100),
                [255, 0, 0, 255],
                "{class} cross"
            );
            assert_eq!(rendered.findings.len(), 1, "{class}");
            assert_eq!(rendered.findings[0].code, "MF-L211");
            assert_eq!(
                rendered.findings[0].message,
                format!("{class} cannot be drawn outside the game: Shot")
            );
        }
    }

    #[test]
    fn controls_are_outlined_without_a_cross() {
        for class in [
            "SliderWidgetClass",
            "EditBoxWidgetClass",
            "ProgressBarWidgetClass",
        ] {
            let text = format!("{class} Bar {{\n position 0 0\n size 1 1\n color 0 0 1 1\n}}");
            let rendered = draw(&text, 200, 200, &RenderOptions::default());
            assert_eq!(
                pixel(&rendered.image, 0, 100),
                [0, 0, 255, 255],
                "{class} outline"
            );
            assert_eq!(
                pixel(&rendered.image, 100, 100),
                [32, 32, 32, 255],
                "{class} inside"
            );
            assert_eq!(rendered.findings.len(), 1, "{class}");
            assert_eq!(rendered.findings[0].code, "MF-L212");
            assert_eq!(
                rendered.findings[0].message,
                "control drawn as an outline placeholder: Bar"
            );
        }
    }

    #[test]
    fn text_widgets_with_a_style_draw_nothing_and_say_nothing() {
        let text = "TextWidgetClass Label {\n size 1 1\n style Normal\n color 1 0 0 1\n}";
        let widgets = parse(text).unwrap();
        let layout = modfoundry_layout::geometry::resolve(
            &widgets,
            text,
            Screen {
                width: 10,
                height: 10,
            },
        );
        let options = RenderOptions::default();
        let rendered = render(&layout, &widgets, text, &options, &NoResources);
        assert!(rendered.findings.is_empty());
        assert!(rendered
            .image
            .pixels
            .chunks(4)
            .all(|p| p[..3] == [32, 32, 32]));
    }

    #[test]
    fn a_rotation_written_as_three_numbers_is_reported_when_any_is_set() {
        let text = "PanelWidgetClass Turned {\n size 1 1\n rotation 0 0 90\n}";
        let layout = modfoundry_layout::geometry::resolve(
            &parse(text).unwrap(),
            text,
            Screen {
                width: 100,
                height: 100,
            },
        );
        let rendered = render(
            &layout,
            &parse(text).unwrap(),
            text,
            &RenderOptions::default(),
            &NoResources,
        );
        assert_eq!(rendered.findings.len(), 1);
        assert_eq!(rendered.findings[0].code, "MF-L213");
    }

    #[test]
    fn rotation_is_drawn_unrotated_with_a_finding() {
        let text = "PanelWidgetClass Turned {\n position 0 0\n size 1 1\n style rover_sim_colorable\n color 1 0 0 1\n rotation 45\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        assert_eq!(pixel(&rendered.image, 100, 100), [255, 0, 0, 255]);
        assert_eq!(rendered.findings.len(), 1);
        assert_eq!(rendered.findings[0].code, "MF-L213");
        assert_eq!(
            rendered.findings[0].message,
            "rotation is not drawn: Turned"
        );
    }

    #[test]
    fn widgets_with_the_same_name_are_told_apart_by_position() {
        let text = "PanelWidgetClass Same {\n position 0 0\n size 0.5 1\n style odd\n}\nPanelWidgetClass Same {\n position 0.5 0\n size 0.5 1\n style strange\n}";
        let rendered = draw(text, 200, 200, &RenderOptions::default());
        let [first, second] = rendered.findings.as_slice() else {
            panic!("expected two findings, got {}", rendered.findings.len());
        };
        assert_eq!(
            first.message,
            "style odd is not drawn, shown as an outline: Same"
        );
        assert_eq!(
            second.message,
            "style strange is not drawn, shown as an outline: Same"
        );
        assert_eq!(first.span.map(|span| span.start), Some(0));
        assert_eq!(
            second.span.map(|span| span.start),
            Some(text.rfind("PanelWidgetClass").unwrap())
        );
    }

    #[test]
    fn image_is_the_size_of_the_screen() {
        let text = "PanelWidgetClass Panel {\n position 0 0\n size 1 1\n style rover_sim_colorable\n color 1 0 0 1\n}";
        let rendered = draw(text, 320, 240, &RenderOptions::default());
        assert_eq!((rendered.image.width, rendered.image.height), (320, 240));
        assert_eq!(rendered.image.pixels.len(), 320 * 240 * 4);
        assert_eq!(rendered.drawn, 1);
    }

    #[test]
    fn hidden_boxes_are_counted_even_without_their_widget() {
        let text = "PanelWidgetClass Panel {\n position 0 0\n size 1 1\n visible 0\n}";
        let layout = modfoundry_layout::geometry::resolve(
            &parse(text).unwrap(),
            text,
            Screen {
                width: 200,
                height: 200,
            },
        );
        let rendered = render(&layout, &[], text, &RenderOptions::default(), &NoResources);
        assert_eq!(rendered.hidden_skipped, 1);
    }
}
