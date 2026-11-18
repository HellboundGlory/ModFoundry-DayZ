use std::collections::HashMap;

use modfoundry_layout::geometry::{resolve, Layout, Rect, Screen, WidgetBox};
use modfoundry_layout::parse::{parse, Value, Widget};
use modfoundry_layout_render::{render, NoResources, RenderOptions, Rendered};
use modfoundry_texture::RgbaImage;

const FIXTURE: &str = include_str!("../../../modfoundry-layout/tests/it/calibration_probe.layout");
const SIZES: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (1280, 960)];

#[test]
fn calibration_probe_draws_its_panels_at_each_size() {
    for (width, height) in SIZES {
        check_size(width, height);
    }
}

fn check_size(width: u32, height: u32) {
    let widgets = parse(FIXTURE).unwrap();
    let layout = resolve(&widgets, FIXTURE, Screen { width, height });
    let options = RenderOptions::default();
    let rendered = render(&layout, &widgets, FIXTURE, &options, &NoResources);
    let size = (width, height);
    assert_eq!((rendered.image.width, rendered.image.height), size);
    let hidden = layout.boxes.iter().filter(|item| !item.visible).count();
    assert_eq!(rendered.hidden_skipped, hidden, "{size:?}");
    let styles = styles_by_position(&widgets);
    let checked = check_flat_panels(&layout, &styles, &rendered);
    assert!(checked >= 10, "{size:?}: only {checked} panels checked");
    check_clip_cut(&layout, &options, &rendered);
}

fn check_flat_panels(
    layout: &Layout,
    styles: &HashMap<(u32, u32), Option<&str>>,
    rendered: &Rendered,
) -> usize {
    let mut checked = 0;
    for item in &layout.boxes {
        if !is_flat_panel(item, styles) {
            continue;
        }
        let (x, y) = centre(item.rect);
        if !on_canvas(&rendered.image, x, y) || !sees(item, x, y) {
            continue;
        }
        let covered = layout
            .boxes
            .iter()
            .any(|later| later.draw_order > item.draw_order && later.visible && sees(later, x, y));
        if covered {
            continue;
        }
        assert_eq!(
            pixel(&rendered.image, x, y),
            channels(item.color),
            "{}",
            label(item)
        );
        checked += 1;
    }
    checked
}

fn check_clip_cut(layout: &Layout, options: &RenderOptions, rendered: &Rendered) {
    let child = layout
        .boxes
        .iter()
        .find(|item| item.name.as_deref() == Some("ClipOnChild"))
        .expect("the fixture has ClipOnChild");
    let clip = child.clip.expect("ClipOnChild is clipped");
    let x = (clip.x + clip.w).round() as i64;
    let (_, y) = centre(child.rect);
    assert!(covers(child.rect, x, y) && !sees(child, x, y));
    let expected = layout
        .boxes
        .iter()
        .filter(|item| item.name.as_deref() != Some("ClipOnChild"))
        .filter(|item| item.visible && item.color[3] >= 1.0 && sees(item, x, y))
        .max_by_key(|item| item.draw_order)
        .map_or(options.background, |item| channels(item.color));
    assert_eq!(pixel(&rendered.image, x, y), expected);
}

fn styles_by_position(widgets: &[Widget]) -> HashMap<(u32, u32), Option<&str>> {
    let mut styles = HashMap::new();
    let mut pending: Vec<&Widget> = widgets.iter().collect();
    while let Some(widget) = pending.pop() {
        let style = widget
            .properties
            .iter()
            .find(|p| p.key == "style")
            .and_then(|p| match p.values.first()? {
                Value::Word(text) | Value::Str(text) => Some(text.as_str()),
                Value::Number(_) => None,
            });
        styles.insert((widget.pos.line, widget.pos.column), style);
        pending.extend(widget.children.iter());
    }
    styles
}

fn is_flat_panel(item: &WidgetBox, styles: &HashMap<(u32, u32), Option<&str>>) -> bool {
    let style = styles.get(&(item.line, item.column)).copied().flatten();
    item.visible && item.color[3] >= 1.0 && style == Some("rover_sim_colorable")
}

fn centre(rect: Rect) -> (i64, i64) {
    (
        (rect.x + rect.w / 2.0).floor() as i64,
        (rect.y + rect.h / 2.0).floor() as i64,
    )
}

fn covers(rect: Rect, x: i64, y: i64) -> bool {
    let x0 = rect.x.round() as i64;
    let y0 = rect.y.round() as i64;
    let x1 = (rect.x + rect.w).round() as i64;
    let y1 = (rect.y + rect.h).round() as i64;
    (x0..x1).contains(&x) && (y0..y1).contains(&y)
}

fn sees(item: &WidgetBox, x: i64, y: i64) -> bool {
    covers(item.rect, x, y) && item.clip.is_none_or(|clip| covers(clip, x, y))
}

fn on_canvas(image: &RgbaImage, x: i64, y: i64) -> bool {
    x >= 0 && y >= 0 && (x as u32) < image.width && (y as u32) < image.height
}

fn pixel(image: &RgbaImage, x: i64, y: i64) -> [u8; 4] {
    let at = (y as usize * image.width as usize + x as usize) * 4;
    image.pixels[at..at + 4].try_into().unwrap()
}

fn channels(color: [f32; 4]) -> [u8; 4] {
    color.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn label(item: &WidgetBox) -> &str {
    item.name.as_deref().unwrap_or(&item.class)
}
