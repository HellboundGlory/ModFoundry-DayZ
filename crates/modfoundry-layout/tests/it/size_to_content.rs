use modfoundry_layout::geometry::{resolve_with, Layout, Rect, Screen, WidgetBox};
use modfoundry_layout::parse::parse;

use crate::size_to_text::FakeText;

fn lay(text: &str) -> Layout {
    resolve_with(
        &parse(text).unwrap(),
        text,
        Screen {
            width: 1920,
            height: 1080,
        },
        &FakeText,
    )
}

fn find<'a>(layout: &'a Layout, name: &str) -> &'a WidgetBox {
    layout
        .boxes
        .iter()
        .find(|item| item.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no box named {name}"))
}

fn assert_rect(actual: Rect, x: f32, y: f32, w: f32, h: f32) {
    assert!(
        (actual.x - x).abs() < 0.01
            && (actual.y - y).abs() < 0.01
            && (actual.w - w).abs() < 0.01
            && (actual.h - h).abs() < 0.01,
        "{actual:?} is not {x} {y} {w} {h}"
    );
}

fn panel(name: &str, w: u32, h: u32) -> String {
    format!("PanelWidgetClass {name} {{\n hexactsize 1\n vexactsize 1\n size {w} {h}\n}}")
}

#[test]
fn size_to_content_v_sizes_each_row_to_its_own_tallest_child() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 2\n Padding 2\n Margin 5\n \"Size To Content V\" 1\n {{\n {}\n {}\n }}\n}}",
        panel("Top", 50, 10),
        panel("Bottom", 50, 20),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 39.0);
    assert_rect(find(&layout, "Top").rect, 2.0, 2.0, 50.0, 10.0);
    assert_rect(find(&layout, "Bottom").rect, 2.0, 17.0, 50.0, 20.0);
}

#[test]
fn relative_children_keep_the_written_grid_height() {
    let relative = |name| format!("PanelWidgetClass {name} {{\n size 1 1\n}}");
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 2\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {}\n {}\n }}\n}}",
        relative("Top"),
        relative("Bottom"),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 100.0);
}

#[test]
fn size_to_content_h_sizes_each_column_to_its_own_widest_child() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 2\n Rows 1\n Padding 0\n Margin 4\n \"Size To Content H\" 1\n {{\n {}\n {}\n }}\n}}",
        panel("Left", 10, 10),
        panel("Right", 30, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 44.0, 100.0);
    assert_rect(find(&layout, "Right").rect, 14.0, 0.0, 30.0, 10.0);
}

#[test]
fn wrap_size_to_content_v_takes_the_height_of_its_rows() {
    let text = format!(
        "WrapSpacerWidgetClass Wrap {{\n hexactsize 1\n vexactsize 1\n size 100 200\n Padding 2\n Margin 4\n \"Size To Content V\" 1\n {{\n {}\n {}\n {}\n }}\n}}",
        panel("A", 40, 10),
        panel("B", 40, 10),
        panel("C", 40, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Wrap").rect, 0.0, 0.0, 100.0, 28.0);
}

#[test]
fn wrap_size_to_content_h_takes_the_width_of_its_widest_row() {
    let text = format!(
        "WrapSpacerWidgetClass Wrap {{\n hexactsize 1\n vexactsize 1\n size 100 200\n Padding 2\n Margin 4\n \"Size To Content H\" 1\n {{\n {}\n {}\n {}\n }}\n}}",
        panel("A", 40, 10),
        panel("B", 40, 10),
        panel("C", 40, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Wrap").rect, 0.0, 0.0, 88.0, 200.0);
}

#[test]
fn bottom_anchored_spacer_keeps_its_bottom_edge_when_it_shrinks() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n valign bottom_ref\n Columns 1\n Rows 2\n Padding 2\n Margin 5\n \"Size To Content V\" 1\n {{\n {}\n {}\n }}\n}}",
        panel("Top", 50, 10),
        panel("Bottom", 50, 20),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 1041.0, 200.0, 39.0);
}

#[test]
fn nested_size_to_content_grid_adds_its_final_height_once() {
    let inner = format!(
        "GridSpacerWidgetClass Inner {{\n size 1 1\n Columns 1\n Rows 2\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {}\n {}\n }}\n}}",
        panel("Top", 50, 10),
        panel("Bottom", 50, 20),
    );
    let text = format!(
        "GridSpacerWidgetClass Outer {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {inner}\n }}\n}}"
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Outer").rect, 0.0, 0.0, 200.0, 30.0);
    assert_rect(find(&layout, "Inner").rect, 0.0, 0.0, 200.0, 30.0);
    assert_eq!(layout.boxes.len(), 4);
}

#[test]
fn spacer_in_a_wrap_moves_its_sibling_by_its_final_width() {
    let spacer = format!(
        "SpacerWidgetClass Spacer {{\n hexactsize 1\n vexactsize 1\n size 200 10\n \"Size To Content H\" 1\n {{\n {}\n }}\n}}",
        panel("Inside", 50, 10),
    );
    let text = format!(
        "WrapSpacerWidgetClass Wrap {{\n hexactsize 1\n vexactsize 1\n size 300 100\n Padding 0\n Margin 0\n {{\n {spacer}\n {}\n }}\n}}",
        panel("Next", 40, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Spacer").rect, 0.0, 0.0, 50.0, 10.0);
    assert_rect(find(&layout, "Next").rect, 50.0, 0.0, 40.0, 10.0);
}

#[test]
fn ignored_hidden_child_adds_nothing_to_the_grid_height() {
    let hidden =
        "PanelWidgetClass Hidden {\n hexactsize 1\n vexactsize 1\n size 50 40\n visible 0\n}";
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n Padding 3\n Margin 4\n \"Ignore invisible\" 1\n \"Size To Content V\" 1\n {{\n {hidden}\n {}\n }}\n}}",
        panel("Shown", 50, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 16.0);
}

#[test]
fn empty_spacer_is_twice_its_padding_on_both_flagged_axes() {
    let text = "SpacerWidgetClass Empty {\n hexactsize 1\n vexactsize 1\n size 100 100\n Padding 3\n \"Size To Content H\" 1\n \"Size To Content V\" 1\n}";
    let layout = lay(text);
    assert_rect(find(&layout, "Empty").rect, 0.0, 0.0, 6.0, 6.0);
}

#[test]
fn grid_takes_the_wrapped_height_of_a_size_to_text_child() {
    let label = "MultilineTextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 64 40\n text \"abcdefgh\"\n \"size to text v\" 1\n}";
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {label}\n }}\n}}"
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 80.0);
    assert_rect(find(&layout, "Label").rect, 0.0, 0.0, 64.0, 80.0);
}

#[test]
fn panel_with_the_property_keeps_its_written_size() {
    let text = format!(
        "PanelWidgetClass Box {{\n hexactsize 1\n vexactsize 1\n size 200 100\n \"Size To Content V\" 1\n {{\n {}\n }}\n}}",
        panel("Child", 50, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Box").rect, 0.0, 0.0, 200.0, 100.0);
}

#[test]
fn assumption_hint_counts_sized_text_and_spacers_once() {
    let label = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n}";
    let spacer = |name: &str| {
        format!(
            "SpacerWidgetClass {name} {{\n hexactsize 1\n vexactsize 1\n size 100 100\n \"Size To Content V\" 1\n}}"
        )
    };
    let text = format!("{label}\n{}\n{}", spacer("First"), spacer("Second"));
    let layout = lay(&text);
    let messages: Vec<&str> = layout
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L238")
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["size to text and Size To Content are applied by assumption (the game's rule is not measured); 1 text widgets and 2 spacers sized"]
    );
}

#[test]
fn no_assumption_hint_when_nothing_is_sized() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n {{\n {}\n }}\n}}",
        panel("Child", 50, 10),
    );
    let layout = lay(&text);
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L238"));
}

#[test]
fn provisional_pass_does_not_repeat_a_nested_grid_hint() {
    let inner = "GridSpacerWidgetClass Inner {\n size 1 1\n Columns 1\n Rows 1\n Padding 2\n Margin 5\n {\n }\n}";
    let text = format!(
        "GridSpacerWidgetClass Outer {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n \"Size To Content V\" 1\n {{\n {inner}\n }}\n}}"
    );
    let layout = lay(&text);
    let hints = layout
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L205")
        .count();
    assert_eq!(hints, 1);
}

fn relative(name: &str) -> String {
    format!("PanelWidgetClass {name} {{\n size 1 1\n}}")
}

#[test]
fn a_grid_with_fewer_children_than_columns_keeps_its_written_width() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 300 100\n Columns 3\n Rows 1\n \"Size To Content H\" 1\n {{\n {}\n {}\n }}\n}}",
        relative("A"),
        relative("B"),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 300.0, 100.0);
}

#[test]
fn size_to_content_v_ignores_rows_and_takes_one_row_per_child() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n \"Size To Content V\" 1\n {{\n {}\n {}\n {}\n }}\n}}",
        panel("A", 50, 10),
        panel("B", 50, 10),
        panel("C", 50, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 30.0);
}

#[test]
fn size_to_content_v_takes_each_child_height_for_a_large_rows_count() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 100\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {}\n {}\n {}\n }}\n}}",
        panel("A", 50, 10),
        panel("B", 50, 20),
        panel("C", 50, 5),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 35.0);
    assert_rect(find(&layout, "B").rect, 0.0, 10.0, 50.0, 20.0);
    assert_rect(find(&layout, "C").rect, 0.0, 30.0, 50.0, 5.0);
}

#[test]
fn size_to_content_h_keeps_equal_rows_on_an_unflagged_axis() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 2\n Padding 0\n Margin 4\n \"Size To Content H\" 1\n {{\n {}\n {}\n }}\n}}",
        panel("Top", 10, 10),
        panel("Bottom", 10, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 10.0, 100.0);
    assert_rect(find(&layout, "Bottom").rect, 0.0, 52.0, 10.0, 10.0);
}

#[test]
fn a_two_by_two_grid_sizes_each_row_to_its_own_tallest_child() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 2\n Rows 2\n Padding 0\n Margin 0\n \"Size To Content V\" 1\n {{\n {}\n {}\n {}\n {}\n }}\n}}",
        panel("A", 20, 10),
        panel("B", 20, 40),
        panel("C", 20, 20),
        panel("D", 20, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 200.0, 60.0);
    assert_rect(find(&layout, "C").rect, 0.0, 40.0, 20.0, 20.0);
}

#[test]
fn nested_spacers_and_their_text_are_counted_once_in_the_hint() {
    let label = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"abcd\"\n \"size to text h\" 1\n}";
    let inner = format!(
        "GridSpacerWidgetClass Inner {{\n size 1 1\n Columns 1\n Rows 1\n \"Size To Content V\" 1\n {{\n {label}\n }}\n}}"
    );
    let text = format!(
        "GridSpacerWidgetClass Outer {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n \"Size To Content V\" 1\n {{\n {inner}\n }}\n}}"
    );
    let layout = lay(&text);
    let hint = layout
        .findings
        .iter()
        .find(|finding| finding.code == "MF-L238")
        .expect("the hint");
    assert!(
        hint.message.ends_with("1 text widgets and 2 spacers sized"),
        "{}",
        hint.message
    );
}

#[test]
fn unmeasured_text_under_a_spacer_is_counted_once() {
    let label = "TextWidgetClass Label {\n hexactsize 1\n vexactsize 1\n size 300 40\n text \"nope\"\n \"size to text h\" 1\n}";
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n \"Size To Content V\" 1\n {{\n {label}\n }}\n}}"
    );
    let layout = lay(&text);
    let hint = layout
        .findings
        .iter()
        .find(|finding| finding.code == "MF-L239")
        .expect("the hint");
    assert!(hint.message.contains(" 1 widgets"), "{}", hint.message);
}

#[test]
fn a_provisional_pass_does_not_shift_the_paint_order_of_later_widgets() {
    let spacer = format!(
        "SpacerWidgetClass Spacer {{\n hexactsize 1\n vexactsize 1\n size 100 100\n \"Size To Content V\" 1\n {{\n {}\n }}\n}}",
        panel("Inside", 10, 10),
    );
    let front = "PanelWidgetClass Front {\n size 1 1\n priority 5\n}";
    let back = "PanelWidgetClass Back {\n size 1 1\n priority 1\n}";
    let layout = lay(&format!("{spacer}\n{front}\n{back}"));
    assert!(find(&layout, "Front").draw_order > find(&layout, "Back").draw_order);
}

#[test]
fn size_to_content_h_takes_one_column_per_child_not_per_columns() {
    let text = format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 100\n Rows 100\n Padding 0\n Margin 4\n \"Size To Content H\" 1\n {{\n {}\n {}\n {}\n }}\n}}",
        panel("A", 10, 10),
        panel("B", 20, 10),
        panel("C", 30, 10),
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 68.0, 100.0);
    assert_rect(find(&layout, "A").rect, 0.0, 0.0, 10.0, 10.0);
    assert_rect(find(&layout, "B").rect, 14.0, 0.0, 20.0, 10.0);
    assert_rect(find(&layout, "C").rect, 38.0, 0.0, 30.0, 10.0);
}

fn grid(extra: &str, children: &[String]) -> String {
    format!(
        "GridSpacerWidgetClass Grid {{\n hexactsize 1\n vexactsize 1\n size 200 100\n {extra}\n {{\n {}\n }}\n}}",
        children.join("\n")
    )
}

#[test]
fn a_vertically_flagged_grid_keeps_equal_columns() {
    let text = grid(
        "Columns 2\n Rows 2\n \"Size To Content V\" 1",
        &[
            panel("A", 20, 10),
            panel("B", 20, 40),
            panel("C", 20, 20),
            panel("D", 20, 10),
        ],
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "B").rect, 100.0, 0.0, 20.0, 40.0);
    assert_rect(find(&layout, "D").rect, 100.0, 40.0, 20.0, 10.0);
}

#[test]
fn a_grid_flagged_on_both_axes_sizes_columns_and_rows_by_their_children() {
    let text = grid(
        "Columns 2\n Rows 2\n \"Size To Content H\" 1\n \"Size To Content V\" 1",
        &[
            panel("A", 10, 10),
            panel("B", 30, 40),
            panel("C", 20, 20),
            panel("D", 10, 10),
        ],
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 50.0, 60.0);
    assert_rect(find(&layout, "B").rect, 20.0, 0.0, 30.0, 40.0);
    assert_rect(find(&layout, "C").rect, 0.0, 40.0, 20.0, 20.0);
    assert_rect(find(&layout, "D").rect, 20.0, 40.0, 10.0, 10.0);
}

#[test]
fn an_ignored_hidden_child_in_the_middle_takes_no_slot() {
    let hidden =
        "PanelWidgetClass Hidden {\n hexactsize 1\n vexactsize 1\n size 99 99\n visible 0\n}"
            .to_owned();
    let text = grid(
        "Columns 2\n \"Ignore invisible\" 1\n \"Size To Content H\" 1\n \"Size To Content V\" 1",
        &[
            panel("A", 20, 10),
            hidden,
            panel("B", 30, 10),
            panel("C", 20, 10),
        ],
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 50.0, 20.0);
    assert_rect(find(&layout, "B").rect, 20.0, 0.0, 30.0, 10.0);
    assert_rect(find(&layout, "C").rect, 0.0, 10.0, 20.0, 10.0);
}

#[test]
fn a_grid_without_columns_is_one_column_wide() {
    let text = grid(
        "\"Size To Content H\" 1",
        &[panel("A", 10, 10), panel("B", 30, 10)],
    );
    let layout = lay(&text);
    assert_rect(find(&layout, "Grid").rect, 0.0, 0.0, 30.0, 100.0);
    assert_rect(find(&layout, "B").rect, 0.0, 50.0, 30.0, 10.0);
}

#[test]
fn a_vertically_flagged_grid_with_more_children_than_rows_is_not_reported() {
    let text = grid(
        "Columns 1\n Rows 1\n \"Size To Content V\" 1",
        &[panel("A", 10, 10), panel("B", 10, 10)],
    );
    let layout = lay(&text);
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L207"));
}

fn nested_grids(depth: usize) -> String {
    let mut text = panel("Leaf", 10, 10);
    for level in 0..depth {
        text = format!(
            "GridSpacerWidgetClass Grid{level} {{\n hexactsize 1\n vexactsize 1\n size 200 100\n Columns 1\n Rows 1\n Padding 5\n \"Size To Content V\" 1\n {{\n {text}\n }}\n}}"
        );
    }
    text
}

#[test]
fn deeply_nested_content_spacers_stop_sizing_and_say_so() {
    let layout = lay(&nested_grids(20));
    let finding = layout
        .findings
        .iter()
        .find(|finding| finding.code == "MF-L240")
        .expect("MF-L240");
    assert!(finding.message.contains("nested"), "{}", finding.message);
}

#[test]
fn shallow_nested_content_spacers_are_all_sized() {
    let layout = lay(&nested_grids(4));
    assert!(layout
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L240"));
    assert_eq!(find(&layout, "Grid3").rect.h, 50.0);
}

#[test]
fn the_depth_cap_counts_content_sized_ancestors_from_the_root() {
    let layout = lay(&nested_grids(12));
    assert_eq!(find(&layout, "Grid3").rect.h, 100.0);
    assert_eq!(find(&layout, "Grid4").rect.h, 110.0);
    let message = &layout
        .findings
        .iter()
        .find(|finding| finding.code == "MF-L238")
        .expect("MF-L238")
        .message;
    assert!(message.contains("8 spacers sized"), "{message}");
}

#[test]
fn a_very_deep_nest_of_content_spacers_is_laid_out_quickly() {
    let started = std::time::Instant::now();
    let layout = lay(&nested_grids(40));
    assert!(layout
        .findings
        .iter()
        .any(|finding| finding.code == "MF-L240"));
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}
