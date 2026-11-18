use modfoundry_layout::geometry::{
    resolve, resolve_with, text_em_px, Layout, Rect, Screen, TextMeasure, TextSize, WidgetBox,
};
use modfoundry_layout::parse::{parse, Value, Widget};

pub(crate) struct FakeText;

impl TextMeasure for FakeText {
    fn measure(&self, widget: &Widget, em_px: f32, wrap_width: Option<f32>) -> Option<TextSize> {
        let text = text_of(widget)?;
        if text == "nope" {
            return None;
        }
        let width = text.chars().count() as f32 * 0.5 * em_px;
        let lines = match wrap_width {
            Some(wrap) => (width / wrap).ceil(),
            None => 1.0,
        };
        Some(TextSize {
            width,
            height: em_px * 1.25 * lines,
        })
    }
}

fn text_of(widget: &Widget) -> Option<&str> {
    let property = widget
        .properties
        .iter()
        .find(|property| property.key == "text")?;
    match property.values.first()? {
        Value::Word(text) | Value::Str(text) => Some(text),
        Value::Number(_) => None,
    }
}

fn lay(text: &str, width: u32, height: u32, measure: &dyn TextMeasure) -> Layout {
    resolve_with(
        &parse(text).unwrap(),
        text,
        Screen { width, height },
        measure,
    )
}

fn find<'a>(layout: &'a Layout, name: &str) -> &'a WidgetBox {
    layout
        .boxes
        .iter()
        .find(|item| item.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no box named {name}"))
}

fn assert_size(rect: Rect, w: f32, h: f32) {
    assert!(
        (rect.w - w).abs() < 0.01 && (rect.h - h).abs() < 0.01,
        "{rect:?} is not {w} x {h}"
    );
}

fn named_text_widget(name: &str, class: &str, extra: &str, text: &str) -> String {
    format!(
        "{class} {name} {{\n hexactsize 1\n vexactsize 1\n size 64 40\n text \"{text}\"\n {extra}\n}}"
    )
}

fn text_widget(class: &str, extra: &str, text: &str) -> String {
    named_text_widget("Label", class, extra, text)
}

fn two_fitted_labels(text: &str) -> String {
    let widget = |name| named_text_widget(name, "TextWidgetClass", "\"size to text h\" 1", text);
    format!("{}\n{}", widget("First"), widget("Second"))
}

#[test]
fn size_to_text_h_sets_the_width_to_the_measured_width() {
    let text = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert_size(find(&layout, "Label").rect, 64.0, 40.0);
}

#[test]
fn size_to_text_v_takes_the_wrapped_height_only_when_the_text_wraps() {
    let cases = [
        ("MultilineTextWidgetClass", "\"size to text v\" 1", 80.0),
        (
            "MultilineTextWidgetClass",
            "\"size to text v\" 1\n \"no wrap\" 1",
            40.0,
        ),
        ("TextWidgetClass", "\"size to text v\" 1", 40.0),
        ("TextWidgetClass", "\"size to text v\" 1\n wrap 1", 80.0),
    ];
    for (class, extra, height) in cases {
        let text = text_widget(class, extra, "abcdefgh");
        let layout = lay(&text, 1920, 1080, &FakeText);
        let rect = find(&layout, "Label").rect;
        assert!(
            (rect.h - height).abs() < 0.01 && rect.w == 64.0,
            "{class} with {extra}: {rect:?}"
        );
    }
}

#[test]
fn right_ref_keeps_the_right_edge_of_the_written_box() {
    let text = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n halign right_ref\n \"size to text h\" 1\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    let rect = find(&layout, "Label").rect;
    assert!((rect.x + rect.w - 1920.0).abs() < 0.01, "{rect:?}");
}

#[test]
fn measured_em_is_stored_on_the_box() {
    let cases = [
        (1080, "", 32.0),
        (1080, "\"exact text\" 1\n \"exact text size\" 30", 30.0),
        (540, "\"exact text\" 1\n \"exact text size\" 30", 15.0),
    ];
    for (screen_height, extra, em) in cases {
        let text = format!(
            "TextWidgetClass Label {{\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n {extra}\n}}"
        );
        let layout = lay(&text, 1920, screen_height, &FakeText);
        let label = find(&layout, "Label");
        assert_eq!(label.text_em_px, Some(em), "{extra} at {screen_height}");
        assert!(
            (label.rect.w - 2.0 * em).abs() < 0.01,
            "{extra} at {screen_height}: {:?}",
            label.rect
        );
    }
}

#[test]
fn a_text_child_of_a_wrap_advances_by_its_measured_width() {
    let text = "WrapSpacerWidgetClass Wrap {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 200\n {\n  TextWidgetClass First {\n   hexactsize 1\n   vexactsize 1\n   size 300 40\n   text \"abcd\"\n   \"size to text h\" 1\n  }\n  PanelWidgetClass Next {\n   hexactsize 1\n   vexactsize 1\n   size 100 40\n  }\n }\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert!((find(&layout, "Next").rect.x - 64.0).abs() < 0.01);
}

#[test]
fn unmeasured_text_is_reported_once_for_all_its_widgets() {
    let text = two_fitted_labels("nope");
    let layout = lay(&text, 1920, 1080, &FakeText);
    let messages: Vec<&str> = layout
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L239")
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["size to text was not applied to 2 widgets: their text could not be measured"]
    );
    let first = find(&layout, "First");
    assert_size(first.rect, 64.0, 40.0);
    assert_eq!(first.text_em_px, None);
}

#[test]
fn plain_resolve_leaves_size_to_text_widgets_at_their_written_size() {
    let text = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n}";
    let layout = resolve(
        &parse(text).unwrap(),
        text,
        Screen {
            width: 1920,
            height: 1080,
        },
    );
    let label = find(&layout, "Label");
    assert_size(label.rect, 300.0, 40.0);
    assert_eq!(label.text_em_px, None);
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L238" && finding.code != "MF-L239"));
}

#[test]
fn a_button_ignores_size_to_text() {
    let text = "ButtonWidgetClass Ok {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert_size(find(&layout, "Ok").rect, 300.0, 40.0);
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L238"));
}

#[test]
fn a_text_widget_without_text_keeps_its_size_and_is_not_reported() {
    let text = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n \"size to text h\" 1\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert_size(find(&layout, "Label").rect, 300.0, 40.0);
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L238" && finding.code != "MF-L239"));
}

struct NotANumber;

impl TextMeasure for NotANumber {
    fn measure(&self, _: &Widget, _: f32, _: Option<f32>) -> Option<TextSize> {
        Some(TextSize {
            width: f32::NAN,
            height: -1.0,
        })
    }
}

#[test]
fn a_measure_that_is_not_a_size_is_treated_as_unmeasured() {
    let text = text_widget("TextWidgetClass", "\"size to text h\" 1", "abcd");
    let layout = lay(&text, 1920, 1080, &NotANumber);
    let label = find(&layout, "Label");
    assert_size(label.rect, 64.0, 40.0);
    assert_eq!(label.text_em_px, None);
    assert!(layout
        .findings
        .iter()
        .any(|finding| finding.code == "MF-L239"));
}

#[test]
fn fitted_width_never_exceeds_the_written_width_when_text_wraps() {
    let cases = [
        (
            "MultilineTextWidgetClass",
            "\"size to text h\" 1\n \"size to text v\" 1",
            (64.0, 80.0),
        ),
        ("TextWidgetClass", "\"size to text h\" 1", (128.0, 40.0)),
        (
            "TextWidgetClass",
            "\"size to text h\" 1\n wrap 1",
            (64.0, 40.0),
        ),
    ];
    for (class, extra, (w, h)) in cases {
        let text = text_widget(class, extra, "abcdefgh");
        let layout = lay(&text, 1920, 1080, &FakeText);
        let rect = find(&layout, "Label").rect;
        assert!(
            (rect.w - w).abs() < 0.01 && (rect.h - h).abs() < 0.01,
            "{class} with {extra}: {rect:?}"
        );
    }
}

#[test]
fn a_text_child_of_a_grid_is_fitted_inside_its_cell() {
    let text = "GridSpacerWidgetClass Grid {\n hexactsize 1\n vexactsize 1\n size 300 200\n Columns 1\n Rows 1\n {\n  TextWidgetClass Label {\n   hexactsize 1\n   vexactsize 1\n   size 300 40\n   text \"abcd\"\n   \"size to text h\" 1\n  }\n }\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert_size(find(&layout, "Label").rect, 64.0, 40.0);
}

#[test]
fn the_assumption_hint_counts_every_sized_widget_once() {
    let text = two_fitted_labels("abcd");
    let layout = lay(&text, 1920, 1080, &FakeText);
    let messages: Vec<&str> = layout
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L238")
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["size to text and Size To Content are applied by assumption (the game's rule is not measured); 2 text widgets and 0 spacers sized"]
    );
}

#[test]
fn a_huge_exact_text_size_is_clamped_to_a_finite_size() {
    let widgets = parse(
        "TextWidgetClass Label {\n size 1 1\n text \"AB\"\n \"exact text\" 1\n \"exact text size\" 999999\n}",
    )
    .unwrap();
    assert_eq!(text_em_px(&widgets[0], 100.0, 1080), 4096.0);
}

#[test]
fn a_wrapping_text_written_without_a_width_takes_its_measured_width() {
    let text = "MultilineTextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 0 40\n text \"abcdefgh\"\n \"size to text h\" 1\n}";
    let layout = lay(text, 1920, 1080, &FakeText);
    assert_size(find(&layout, "Label").rect, 128.0, 40.0);
}
