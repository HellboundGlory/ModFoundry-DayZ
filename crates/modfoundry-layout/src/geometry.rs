//! Where every widget of a layout sits on a screen, in screen pixels.

use std::collections::HashMap;

use serde::Serialize;

use modfoundry_core::{Diagnostic, Severity};

use crate::parse::{Pos, Value, Widget};
use crate::source::Source;

/// The screen a layout is resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

/// A box in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One resolved widget.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WidgetBox {
    /// Position in `Layout::boxes`: depth-first, file order.
    pub index: usize,
    pub parent: Option<usize>,
    pub depth: u32,
    pub class: String,
    pub name: Option<String>,
    pub line: u32,
    pub column: u32,
    pub rect: Rect,
    /// The box the widget's pixels are cut to; `None` when nothing clips it.
    pub clip: Option<Rect>,
    /// False when the widget or an ancestor has `visible 0`.
    pub visible: bool,
    /// Red, green, blue, alpha, each 0 to 1. Opaque white when `color` is missing or short; the renderer
    /// applies the style's default colour to filled styles that have no `color`.
    pub color: [f32; 4],
    /// 0 is painted first.
    pub draw_order: u32,
    /// The text size the box was measured with, set only when size to text changed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_em_px: Option<f32>,
}

/// The size of a block of text, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSize {
    pub width: f32,
    pub height: f32,
}

/// Measures the text of a widget for size to text.
pub trait TextMeasure {
    /// The size of the widget's text at `em_px`, wrapped at `wrap_width` when given; `None` when it cannot be measured.
    fn measure(&self, widget: &Widget, em_px: f32, wrap_width: Option<f32>) -> Option<TextSize>;

    /// False for a measurer that answers nothing, so size to text is skipped without findings.
    fn measures(&self) -> bool {
        true
    }
}

/// Measures nothing, so size to text leaves every widget at its written size.
pub struct NoText;

impl TextMeasure for NoText {
    fn measure(&self, _widget: &Widget, _em_px: f32, _wrap_width: Option<f32>) -> Option<TextSize> {
        None
    }

    fn measures(&self) -> bool {
        false
    }
}

/// A resolved layout: every widget box plus the findings for what is not drawn as the game draws it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub screen: Screen,
    pub boxes: Vec<WidgetBox>,
    pub findings: Vec<Diagnostic>,
}

/// Resolves the widgets of a parsed layout, read from `text`, onto a screen.
pub fn resolve(widgets: &[Widget], text: &str, screen: Screen) -> Layout {
    resolve_with(widgets, text, screen, &NoText)
}

/// Resolves like `resolve`, measuring text with `measure` for size to text.
pub fn resolve_with(
    widgets: &[Widget],
    text: &str,
    screen: Screen,
    measure: &dyn TextMeasure,
) -> Layout {
    let page = Frame {
        parent: None,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: screen.width as f32,
            h: screen.height as f32,
        },
        visible: true,
        clip: None,
        depth: 0,
    };
    let mut resolver = Resolver {
        scale: design_scale(screen.height),
        screen_height: screen.height,
        measure,
        source: Source::new(text),
        boxes: Vec::new(),
        priorities: Vec::new(),
        findings: Vec::new(),
        sized_text: 0,
        sized_spacers: 0,
        unmeasured_text: 0,
        fit_capped: false,
        fit_depth: 0,
        tracks: HashMap::new(),
    };
    for widget in widgets {
        resolver.place(widget, &page);
    }
    resolver.text_size_findings();
    let mut boxes = resolver.boxes;
    for (order, index) in paint_order(&boxes, &resolver.priorities)
        .into_iter()
        .enumerate()
    {
        boxes[index].draw_order = order as u32;
    }
    Layout {
        screen,
        boxes,
        findings: resolver.findings,
    }
}

fn paint_order(boxes: &[WidgetBox], priorities: &[f32]) -> Vec<usize> {
    let mut roots = Vec::new();
    let mut children = vec![Vec::new(); boxes.len()];
    for item in boxes {
        match item.parent {
            Some(parent) => children[parent].push(item.index),
            None => roots.push(item.index),
        }
    }
    let by_priority = |list: &mut Vec<usize>| {
        list.sort_by(|a, b| priorities[*a].total_cmp(&priorities[*b]));
    };
    by_priority(&mut roots);
    children.iter_mut().for_each(by_priority);
    let mut order = Vec::with_capacity(boxes.len());
    for root in roots {
        paint_subtree(root, &children, &mut order);
    }
    order
}

fn paint_subtree(index: usize, children: &[Vec<usize>], order: &mut Vec<usize>) {
    order.push(index);
    for &child in &children[index] {
        paint_subtree(child, children, order);
    }
}

/// Parent index of the boxes a provisional placement makes; no real box has it.
const PROVISIONAL_PARENT: usize = usize::MAX;

/// Content-sized spacers nested deeper than this keep their written size: each level places its children twice.
const MAX_CONTENT_FIT_DEPTH: usize = 8;

/// The resolver's counters and box list before a provisional placement, to return to.
#[derive(Clone, Copy)]
struct Checkpoint {
    boxes: usize,
    findings: usize,
    sized_text: usize,
    sized_spacers: usize,
    unmeasured_text: usize,
}

/// The parent a widget is placed in.
#[derive(Clone, Copy)]
struct Frame {
    parent: Option<usize>,
    rect: Rect,
    visible: bool,
    clip: Option<Rect>,
    depth: u32,
}

struct Resolver<'a> {
    scale: f32,
    screen_height: u32,
    measure: &'a dyn TextMeasure,
    source: Source<'a>,
    boxes: Vec<WidgetBox>,
    priorities: Vec<f32>,
    findings: Vec<Diagnostic>,
    sized_text: usize,
    sized_spacers: usize,
    unmeasured_text: usize,
    /// Set inside provisional passes too, so a checkpoint rollback must not clear it.
    fit_capped: bool,
    fit_depth: usize,
    tracks: HashMap<(u32, u32), GridTracks>,
}

#[derive(Default)]
struct GridTracks {
    columns: Vec<f32>,
    rows: Vec<f32>,
}

#[derive(Clone, Copy)]
struct Track {
    start: f32,
    size: f32,
}

impl Resolver<'_> {
    fn check(&mut self, widget: &Widget) {
        if let Some(script) = widget.word("scriptclass").filter(|class| !class.is_empty()) {
            let message = format!(
                "script class {script} may move or resize this widget: {}",
                widget.label()
            );
            self.report("MF-L201", Severity::Warning, &widget.pos, message);
        }
        if aspect_of(widget) == Some("outside") {
            let message = format!(
                "fixaspect outside is drawn as filling the parent (measured); a clipped cover looks the same: {}",
                widget.label()
            );
            self.report("MF-L202", Severity::Hint, &widget.pos, message);
        }
        self.check_unmeasured(widget);
        self.check_spacer(widget);
        if aspect_of(widget) == Some("inside") {
            let (w, h) = number_pair(widget, "size");
            if !(w > 0.0 && h > 0.0) {
                let message = format!("fixaspect inside needs a size ratio: {}", widget.label());
                self.report("MF-L209", Severity::Hint, &widget.pos, message);
            }
        }
        if widget.values("size").is_empty() && aspect_of(widget) != Some("outside") {
            let message = format!("no size written, drawn as 0 x 0: {}", widget.label());
            self.report("MF-L208", Severity::Hint, &widget.pos, message);
        }
    }

    fn check_spacer(&mut self, widget: &Widget) {
        match widget.class.as_str() {
            "GridSpacerWidgetClass" => self.check_grid(widget),
            "WrapSpacerWidgetClass" => self.check_wrap(widget),
            _ => {}
        }
    }

    fn check_wrap(&mut self, widget: &Widget) {
        let spaced = ["Margin", "Padding"]
            .iter()
            .any(|key| number_at(widget.values(key), 0).unwrap_or(0.0) != 0.0);
        let positioned =
            spacer_children(widget).any(|child| number_pair(child, "position") != (0.0, 0.0));
        if spaced || positioned {
            let message = format!(
                "wrap spacing and child position are not measured: {}",
                widget.label()
            );
            self.report("MF-L206", Severity::Hint, &widget.pos, message);
        }
    }

    fn check_grid(&mut self, widget: &Widget) {
        let margin = number_at(widget.values("Margin"), 0).unwrap_or(0.0);
        let padding = number_at(widget.values("Padding"), 0).unwrap_or(0.0);
        if margin != padding {
            let message = format!(
                "which of Margin and Padding is the gap was not measured (tooltips: Padding is the outer margin): {}",
                widget.label()
            );
            self.report("MF-L205", Severity::Hint, &widget.pos, message);
        }
        let columns = grid_columns(widget);
        if columns.is_none() {
            self.grid_shape(widget, "Columns missing or below 1");
        }
        if flag(widget, "Ignore invisible") {
            self.grid_shape(widget, "Ignore invisible 1");
        }
        let cells = grid_cell_count(widget);
        let columns = columns.unwrap_or(1.0);
        let fits_rows = flag(widget, "Size To Content V")
            || cells as f32 <= columns * grid_rows(widget, columns, cells);
        if !fits_rows {
            self.grid_shape(widget, "more children than Columns times Rows");
        }
    }

    fn grid_shape(&mut self, widget: &Widget, what: &str) {
        let message = format!("grid shape not measured: {what}: {}", widget.label());
        self.report("MF-L207", Severity::Hint, &widget.pos, message);
    }

    fn check_unmeasured(&mut self, widget: &Widget) {
        self.check_word(
            widget,
            "halign",
            &["left", "left_ref", "center_ref", "right_ref"],
        );
        self.check_word(
            widget,
            "valign",
            &["top", "top_ref", "center_ref", "bottom_ref"],
        );
        self.check_word(
            widget,
            "fixaspect",
            &["none", "inside", "outside", "fixwidth"],
        );
        let exact_size = ["hexactsize", "vexactsize"]
            .iter()
            .any(|key| flag(widget, key));
        let exact_position = ["hexactpos", "vexactpos"]
            .iter()
            .any(|key| flag(widget, key));
        let aspect = aspect_of(widget).filter(|aspect| RATIOS.contains(aspect));
        if let Some(aspect) = aspect {
            let exact = match (exact_size, exact_position) {
                (true, _) => "sizes",
                (false, true) => "positions",
                (false, false) => return,
            };
            self.unmeasured(widget, &format!("fixaspect {aspect} with exact {exact}"));
        }
    }

    fn check_word(&mut self, widget: &Widget, key: &str, known: &[&str]) {
        if let Some(value) = widget.word(key).filter(|value| !known.contains(value)) {
            self.unmeasured(widget, &format!("{key} value {value}"));
        }
    }

    fn unmeasured(&mut self, widget: &Widget, what: &str) {
        let message = format!("not measured in the game: {what}: {}", widget.label());
        self.report("MF-L203", Severity::Hint, &widget.pos, message);
    }

    fn report(&mut self, code: &str, severity: Severity, pos: &Pos, message: String) {
        let span = self.source.span_from(pos);
        self.findings
            .push(Diagnostic::at(code, severity, span, message));
    }

    fn place(&mut self, widget: &Widget, frame: &Frame) {
        if widget.class == "ScriptParamsClass" {
            return;
        }
        let (size, text_em_px) = self.sized(widget, frame.rect);
        let rect = rect_of(widget, frame.rect, self.scale, size);
        self.place_at(widget, frame, rect, text_em_px);
    }

    fn sized(&mut self, widget: &Widget, parent: Rect) -> ((f32, f32), Option<f32>) {
        let written = size(widget, parent, self.scale);
        if sizes_to_content(widget) {
            return (self.content_fit(widget, written), None);
        }
        if !self.measure.measures() || !size_to_text(widget) || !has_text(widget) {
            return (written, None);
        }
        match self.text_size(widget, written) {
            Some((fitted, em)) => {
                self.sized_text += 1;
                (fitted, Some(em))
            }
            None => {
                self.unmeasured_text += 1;
                (written, None)
            }
        }
    }

    fn text_size(&self, widget: &Widget, written: (f32, f32)) -> Option<((f32, f32), f32)> {
        let em = text_em_px(widget, written.1, self.screen_height);
        let fitted = self.fit_text(widget, written, em)?;
        Some((fitted, em))
    }

    fn fit_text(&self, widget: &Widget, (mut w, mut h): (f32, f32), em: f32) -> Option<(f32, f32)> {
        if flag(widget, "size to text h") {
            let measured = self.measured(widget, em, None)?.width;
            w = if wraps_text(widget) && w > 0.0 {
                measured.min(w)
            } else {
                measured
            };
        }
        if flag(widget, "size to text v") {
            let wrap_width = wraps_text(widget).then_some(w);
            h = self.measured(widget, em, wrap_width)?.height;
        }
        Some((w, h))
    }

    fn measured(&self, widget: &Widget, em: f32, wrap_width: Option<f32>) -> Option<TextSize> {
        self.measure
            .measure(widget, em, wrap_width)
            .filter(|size| size.width.is_finite() && size.height.is_finite())
            .filter(|size| size.width >= 0.0 && size.height >= 0.0)
    }

    fn text_size_findings(&mut self) {
        if self.sized_text + self.sized_spacers > 0 {
            let message = format!(
                "size to text and Size To Content are applied by assumption (the game's rule is not measured); {} text widgets and {} spacers sized",
                self.sized_text, self.sized_spacers
            );
            self.findings
                .push(Diagnostic::new("MF-L238", Severity::Hint, message));
        }
        if self.fit_capped {
            let message = format!(
                "Size To Content was not applied to spacers nested more than {MAX_CONTENT_FIT_DEPTH} deep"
            );
            self.findings
                .push(Diagnostic::new("MF-L240", Severity::Hint, message));
        }
        if self.unmeasured_text > 0 {
            let message = format!(
                "size to text was not applied to {} widgets: their text could not be measured",
                self.unmeasured_text
            );
            self.findings
                .push(Diagnostic::new("MF-L239", Severity::Hint, message));
        }
    }

    fn place_at(&mut self, widget: &Widget, frame: &Frame, rect: Rect, text_em_px: Option<f32>) {
        self.check(widget);
        if sizes_to_content(widget) && self.fit_depth < MAX_CONTENT_FIT_DEPTH {
            self.sized_spacers += 1;
        }
        let visible = frame.visible && !is_hidden(widget);
        let index = self.boxes.len();
        // Adding 0.0 turns -0 into 0, which `total_cmp` would otherwise sort first.
        self.priorities
            .push(number_at(widget.values("priority"), 0).unwrap_or(0.0) + 0.0);
        self.boxes.push(WidgetBox {
            index,
            parent: frame.parent,
            depth: frame.depth,
            class: widget.class.clone(),
            name: widget.name.clone(),
            line: widget.pos.line,
            column: widget.pos.column,
            rect,
            clip: frame.clip,
            visible,
            color: color_of(widget),
            draw_order: 0,
            text_em_px,
        });
        let clip = if number_at(widget.values("clipchildren"), 0) == Some(1.0) {
            Some(cut(rect, frame.clip))
        } else {
            frame.clip
        };
        let children = Frame {
            parent: Some(index),
            rect,
            visible,
            clip,
            depth: frame.depth + 1,
        };
        let content_sized = sizes_to_content(widget);
        self.fit_depth += usize::from(content_sized);
        self.place_children(widget, &children);
        self.fit_depth -= usize::from(content_sized);
    }

    fn place_children(&mut self, widget: &Widget, frame: &Frame) {
        match widget.class.as_str() {
            "GridSpacerWidgetClass" => self.place_grid(widget, frame),
            "WrapSpacerWidgetClass" => self.place_wrap(widget, frame),
            _ => {
                for child in &widget.children {
                    self.place(child, frame);
                }
            }
        }
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            boxes: self.boxes.len(),
            findings: self.findings.len(),
            sized_text: self.sized_text,
            sized_spacers: self.sized_spacers,
            unmeasured_text: self.unmeasured_text,
        }
    }

    fn rollback(&mut self, at: Checkpoint) {
        self.boxes.truncate(at.boxes);
        self.priorities.truncate(at.boxes);
        self.findings.truncate(at.findings);
        self.sized_text = at.sized_text;
        self.sized_spacers = at.sized_spacers;
        self.unmeasured_text = at.unmeasured_text;
    }

    fn content_fit(&mut self, widget: &Widget, written: (f32, f32)) -> (f32, f32) {
        if self.fit_depth >= MAX_CONTENT_FIT_DEPTH {
            self.fit_capped = true;
            return written;
        }
        self.fit_depth += 1;
        let fitted = self.fit_to_children(widget, written);
        self.fit_depth -= 1;
        fitted
    }

    fn fit_to_children(&mut self, widget: &Widget, written: (f32, f32)) -> (f32, f32) {
        let key = track_key(widget);
        self.tracks.remove(&key);
        let at = self.checkpoint();
        let frame = Frame {
            parent: Some(PROVISIONAL_PARENT),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: written.0,
                h: written.1,
            },
            visible: true,
            clip: None,
            depth: 0,
        };
        self.place_children(widget, &frame);
        let (content, tracks) = self.content_bounds(widget, at.boxes);
        self.rollback(at);
        if let Some(tracks) = tracks {
            self.tracks.insert(key, tracks);
        }
        let fitted_w = if flag(widget, "Size To Content H") {
            content.0
        } else {
            written.0
        };
        let fitted_h = if flag(widget, "Size To Content V") {
            content.1
        } else {
            written.1
        };
        (fitted_w, fitted_h)
    }

    fn content_bounds(&self, widget: &Widget, from: usize) -> ((f32, f32), Option<GridTracks>) {
        let placed: Vec<&WidgetBox> = self.boxes[from..]
            .iter()
            .filter(|item| item.parent == Some(PROVISIONAL_PARENT))
            .collect();
        let gap = spacing(widget, "Margin", self.scale);
        let pad = spacing(widget, "Padding", self.scale);
        let ((w, h), tracks) = match widget.class.as_str() {
            "GridSpacerWidgetClass" => {
                let tracks = grid_tracks(widget, &placed);
                let extent = (
                    tracks_extent(&tracks.columns, gap),
                    tracks_extent(&tracks.rows, gap),
                );
                (extent, Some(tracks))
            }
            "WrapSpacerWidgetClass" => (flow_content(&placed, (pad, pad)), None),
            _ => (flow_content(&placed, (0.0, 0.0)), None),
        };
        (((w + 2.0 * pad).max(0.0), (h + 2.0 * pad).max(0.0)), tracks)
    }

    fn place_wrap(&mut self, widget: &Widget, frame: &Frame) {
        let gap = spacing(widget, "Margin", self.scale);
        let inner = inset(frame.rect, spacing(widget, "Padding", self.scale));
        let (mut x, mut y, mut row_height) = (inner.x, inner.y, 0.0_f32);
        let mut row_has_child = false;
        for child in spacer_children(widget) {
            let ((w, h), text_em_px) = self.sized(child, inner);
            if row_has_child && x + w > inner.x + inner.w + WRAP_SLACK {
                x = inner.x;
                y += row_height + gap;
                row_height = 0.0;
            }
            self.place_at(child, frame, Rect { x, y, w, h }, text_em_px);
            x += w + gap;
            row_height = row_height.max(h);
            row_has_child = true;
        }
    }

    fn grid_axes(&self, widget: &Widget, inner: Rect, gap: f32) -> (Vec<Track>, Vec<Track>) {
        let cells = grid_cell_count(widget);
        let columns = grid_column_count(widget);
        let rows = grid_rows(widget, columns, cells);
        let used_columns = (cells as f32).min(columns).max(1.0) as usize;
        let used_rows = (cells as f32 / columns).ceil().max(1.0) as usize;
        let stored = self.tracks.get(&track_key(widget));
        let flag_h = flag(widget, "Size To Content H");
        let flag_v = flag(widget, "Size To Content V");
        let column_count = if flag_h {
            used_columns
        } else {
            columns as usize
        };
        let column_tracks = match stored.filter(|_| flag_h) {
            Some(tracks) => stacked_tracks(&tracks.columns, gap),
            None => equal_tracks(column_count, used_columns, inner.w, gap),
        };
        let row_count = if flag_v { used_rows } else { rows as usize };
        let row_tracks = match stored.filter(|_| flag_v) {
            Some(tracks) => stacked_tracks(&tracks.rows, gap),
            None => equal_tracks(row_count, used_rows, inner.h, gap),
        };
        (column_tracks, row_tracks)
    }

    fn place_grid(&mut self, widget: &Widget, frame: &Frame) {
        let ignore_hidden = flag(widget, "Ignore invisible");
        let children: Vec<&Widget> = spacer_children(widget).collect();
        let columns = grid_column_count(widget) as usize;
        let gap = spacing(widget, "Margin", self.scale);
        let inner = inset(frame.rect, spacing(widget, "Padding", self.scale));
        let (column_tracks, row_tracks) = self.grid_axes(widget, inner, gap);
        let mut slot = 0;
        for child in children {
            if !takes_cell(child, ignore_hidden) {
                self.place(child, frame);
                continue;
            }
            let column = column_tracks[slot % columns];
            let row = row_tracks[slot / columns];
            let cell = Rect {
                x: inner.x + column.start,
                y: inner.y + row.start,
                w: column.size,
                h: row.size,
            };
            self.place(
                child,
                &Frame {
                    rect: cell,
                    ..*frame
                },
            );
            slot += 1;
        }
    }
}

const CALIBRATION_HEIGHT: f32 = 1080.0;

/// Pixels per layout unit: exact sizes are written for a 1080-high screen.
pub fn design_scale(screen_height: u32) -> f32 {
    screen_height as f32 / CALIBRATION_HEIGHT
}

const DEFAULT_TEXT_PROPORTION: f32 = 0.8;
const MAX_EM_PX: f32 = 4096.0;
/// Larger numbers are treated as missing so sums of them stay finite.
const MAX_NUMBER: f32 = 1.0e6;
const WRAP_SLACK: f32 = 0.01;
const RATIOS: [&str; 3] = ["inside", "outside", "fixwidth"];

#[derive(Clone, Copy)]
enum Anchor {
    Start,
    Center,
    End,
}

fn rect_of(widget: &Widget, parent: Rect, scale: f32, (w, h): (f32, f32)) -> Rect {
    let (x, y) = position(widget, parent, scale);
    Rect {
        x: anchored(
            parent.x,
            parent.w,
            x,
            w,
            anchor(widget.word("halign"), "right_ref"),
        ),
        y: anchored(
            parent.y,
            parent.h,
            y,
            h,
            anchor(widget.word("valign"), "bottom_ref"),
        ),
        w,
        h,
    }
}

fn inset(rect: Rect, amount: f32) -> Rect {
    Rect {
        x: rect.x + amount,
        y: rect.y + amount,
        w: (rect.w - 2.0 * amount).max(0.0),
        h: (rect.h - 2.0 * amount).max(0.0),
    }
}

fn sizes_to_content(widget: &Widget) -> bool {
    matches!(
        widget.class.as_str(),
        "GridSpacerWidgetClass" | "WrapSpacerWidgetClass" | "SpacerWidgetClass"
    ) && (flag(widget, "Size To Content H") || flag(widget, "Size To Content V"))
}

/// The extent of children laid out from `origin`, the content box's top left, to their far edges.
fn flow_content(placed: &[&WidgetBox], origin: (f32, f32)) -> (f32, f32) {
    let (right, bottom) = placed.iter().fold(origin, |(right, bottom), item| {
        (
            right.max(item.rect.x + item.rect.w),
            bottom.max(item.rect.y + item.rect.h),
        )
    });
    (right - origin.0, bottom - origin.1)
}

fn track_key(widget: &Widget) -> (u32, u32) {
    (widget.pos.line, widget.pos.column)
}

fn grid_tracks(widget: &Widget, placed: &[&WidgetBox]) -> GridTracks {
    let columns = grid_column_count(widget) as usize;
    let mut tracks = GridTracks::default();
    for (slot, item) in grid_slots(widget, placed).enumerate() {
        widen(&mut tracks.columns, slot % columns, item.rect.w);
        widen(&mut tracks.rows, slot / columns, item.rect.h);
    }
    tracks
}

fn grid_slots<'a>(
    widget: &'a Widget,
    placed: &'a [&'a WidgetBox],
) -> impl Iterator<Item = &'a WidgetBox> + 'a {
    let ignore_hidden = flag(widget, "Ignore invisible");
    spacer_children(widget)
        .zip(placed.iter().copied())
        .filter(move |(child, _)| takes_cell(child, ignore_hidden))
        .map(|(_, item)| item)
}

fn widen(sizes: &mut Vec<f32>, index: usize, size: f32) {
    if sizes.len() <= index {
        sizes.resize(index + 1, 0.0);
    }
    sizes[index] = sizes[index].max(size);
}

fn tracks_extent(sizes: &[f32], gap: f32) -> f32 {
    sizes.iter().sum::<f32>() + gap * sizes.len().saturating_sub(1) as f32
}

fn equal_tracks(count: usize, needed: usize, len: f32, gap: f32) -> Vec<Track> {
    let size = ((len - gap * (count as f32 - 1.0)) / count as f32).max(0.0);
    (0..needed)
        .map(|index| Track {
            start: index as f32 * (size + gap),
            size,
        })
        .collect()
}

fn stacked_tracks(sizes: &[f32], gap: f32) -> Vec<Track> {
    let mut start = 0.0;
    sizes
        .iter()
        .map(|&size| {
            let track = Track { start, size };
            start += size + gap;
            track
        })
        .collect()
}

fn is_hidden(widget: &Widget) -> bool {
    number_at(widget.values("visible"), 0) == Some(0.0)
}

fn spacing(widget: &Widget, key: &str, scale: f32) -> f32 {
    number_at(widget.values(key), 0).unwrap_or(0.0) * scale
}

fn grid_columns(widget: &Widget) -> Option<f32> {
    number_at(widget.values("Columns"), 0)
        .map(f32::floor)
        .filter(|columns| *columns >= 1.0)
}

fn spacer_children(widget: &Widget) -> impl Iterator<Item = &Widget> {
    widget
        .children
        .iter()
        .filter(|child| child.class != "ScriptParamsClass")
}

fn grid_column_count(widget: &Widget) -> f32 {
    grid_columns(widget).unwrap_or(1.0)
}

fn takes_cell(child: &Widget, ignore_hidden: bool) -> bool {
    !(ignore_hidden && is_hidden(child))
}

fn grid_cells(widget: &Widget) -> impl Iterator<Item = &Widget> {
    let ignore_hidden = flag(widget, "Ignore invisible");
    spacer_children(widget).filter(move |child| takes_cell(child, ignore_hidden))
}

fn grid_cell_count(widget: &Widget) -> usize {
    grid_cells(widget).count()
}

fn grid_rows(widget: &Widget, columns: f32, children: usize) -> f32 {
    number_at(widget.values("Rows"), 0)
        .map(f32::floor)
        .filter(|rows| *rows >= 1.0)
        .unwrap_or_else(|| (children as f32 / columns).ceil())
        .max(1.0)
}

fn color_of(widget: &Widget) -> [f32; 4] {
    let rgba = widget.values("color");
    match (
        number_at(rgba, 0),
        number_at(rgba, 1),
        number_at(rgba, 2),
        number_at(rgba, 3),
    ) {
        (Some(r), Some(g), Some(b), Some(a)) => [r, g, b, a],
        _ => [1.0; 4],
    }
}

fn cut(rect: Rect, clip: Option<Rect>) -> Rect {
    let Some(clip) = clip else {
        return rect;
    };
    let x = rect.x.max(clip.x);
    let y = rect.y.max(clip.y);
    let right = (rect.x + rect.w).min(clip.x + clip.w);
    let bottom = (rect.y + rect.h).min(clip.y + clip.h);
    if right <= x || bottom <= y {
        return Rect {
            x,
            y,
            w: 0.0,
            h: 0.0,
        };
    }
    Rect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    }
}

fn anchored(start: f32, parent_len: f32, offset: f32, len: f32, anchor: Anchor) -> f32 {
    match anchor {
        Anchor::Start => start + offset,
        Anchor::Center => start + parent_len / 2.0 + offset - len / 2.0,
        Anchor::End => start + parent_len - offset - len,
    }
}

fn anchor(word: Option<&str>, end_word: &str) -> Anchor {
    match word {
        Some("center_ref") => Anchor::Center,
        Some(word) if word == end_word => Anchor::End,
        _ => Anchor::Start,
    }
}

fn aspect_of(widget: &Widget) -> Option<&str> {
    widget.word("fixaspect").filter(|_| !is_text(widget))
}

fn is_text(widget: &Widget) -> bool {
    matches!(
        widget.class.as_str(),
        "TextWidgetClass" | "RichTextWidgetClass" | "MultilineTextWidgetClass"
    )
}

fn size_to_text(widget: &Widget) -> bool {
    matches!(
        widget.class.as_str(),
        "TextWidgetClass" | "RichTextWidgetClass" | "MultilineTextWidgetClass"
    ) && (flag(widget, "size to text h") || flag(widget, "size to text v"))
}

fn has_text(widget: &Widget) -> bool {
    widget.word("text").is_some_and(|text| !text.is_empty())
}

/// The text size in pixels: an exact size scaled by the screen height, else the box height times the proportion.
pub fn text_em_px(widget: &Widget, box_height: f32, screen_height: u32) -> f32 {
    let exact = number_at(widget.values("exact text size"), 0)
        .filter(|_| flag(widget, "exact text"))
        .map(|size| size * screen_height as f32 / CALIBRATION_HEIGHT);
    let em = exact.unwrap_or_else(|| {
        box_height
            * number_at(widget.values("text_proportion"), 0).unwrap_or(DEFAULT_TEXT_PROPORTION)
    });
    if em.is_finite() {
        em.clamp(0.0, MAX_EM_PX)
    } else {
        0.0
    }
}

/// Whether the text wraps at the box width: multiline text, or `wrap 1`, unless `no wrap 1`.
pub fn wraps_text(widget: &Widget) -> bool {
    if flag(widget, "no wrap") {
        return false;
    }
    widget.class == "MultilineTextWidgetClass" || flag(widget, "wrap")
}

fn size(widget: &Widget, parent: Rect, scale: f32) -> (f32, f32) {
    let (w, h) = number_pair(widget, "size");
    match aspect_of(widget) {
        Some("inside") if w > 0.0 && h > 0.0 => {
            let k = (parent.w / w).min(parent.h / h);
            (w * k, h * k)
        }
        Some("outside") => (parent.w, parent.h),
        Some("fixwidth") => (
            axis_len(w, flag(widget, "hexactsize"), parent.h, scale),
            axis_len(h, flag(widget, "vexactsize"), parent.h, scale),
        ),
        _ => (
            axis_len(w, flag(widget, "hexactsize"), parent.w, scale),
            axis_len(h, flag(widget, "vexactsize"), parent.h, scale),
        ),
    }
}

fn position(widget: &Widget, parent: Rect, scale: f32) -> (f32, f32) {
    let (x, y) = number_pair(widget, "position");
    (
        axis_len(x, flag(widget, "hexactpos"), parent.w, scale),
        axis_len(y, flag(widget, "vexactpos"), parent.h, scale),
    )
}

fn axis_len(number: f32, exact: bool, parent_len: f32, scale: f32) -> f32 {
    if exact {
        number * scale
    } else {
        number * parent_len
    }
}

fn number_pair(widget: &Widget, key: &str) -> (f32, f32) {
    let values = widget.values(key);
    (
        number_at(values, 0).unwrap_or(0.0),
        number_at(values, 1).unwrap_or(0.0),
    )
}

fn flag(widget: &Widget, key: &str) -> bool {
    number_at(widget.values(key), 0) == Some(1.0)
}

fn number_at(values: &[Value], at: usize) -> Option<f32> {
    match values.get(at)? {
        Value::Number(text) => text
            .parse()
            .ok()
            .filter(|number: &f32| number.abs() <= MAX_NUMBER),
        Value::Word(_) | Value::Str(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    const EPSILON: f32 = 0.01;

    fn lay(text: &str, width: u32, height: u32) -> Layout {
        resolve(&parse(text).unwrap(), text, Screen { width, height })
    }

    fn assert_rect(actual: Rect, x: f32, y: f32, w: f32, h: f32) {
        for (got, want) in [(actual.x, x), (actual.y, y), (actual.w, w), (actual.h, h)] {
            assert!(
                (got - want).abs() < EPSILON,
                "{actual:?} is not {x} {y} {w} {h}"
            );
        }
    }

    #[test]
    fn fraction_size_is_a_share_of_the_screen() {
        let layout = lay("PanelWidgetClass Box {\n size 0.2 0.2\n}", 1920, 1080);
        assert_rect(layout.boxes[0].rect, 0.0, 0.0, 384.0, 216.0);
    }

    #[test]
    fn exact_position_is_pixels_scaled_by_height() {
        let layout = lay(
            "PanelWidgetClass Box {\n hexactpos 1\n vexactpos 1\n position 100 300\n size 0.1 0.1\n}",
            1280,
            720,
        );
        assert_rect(layout.boxes[0].rect, 66.67, 200.0, 128.0, 72.0);
    }

    fn find<'a>(layout: &'a Layout, name: &str) -> &'a WidgetBox {
        layout
            .boxes
            .iter()
            .find(|item| item.name.as_deref() == Some(name))
            .unwrap()
    }

    #[test]
    fn children_are_placed_against_the_parent_box() {
        let layout = lay(
            "PanelWidgetClass Outer {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 1200 100\n size 400 300\n {\n PanelWidgetClass Inner {\n  position 0.1 0.1\n  size 0.5 0.5\n }\n }\n}",
            1920,
            1080,
        );
        assert_rect(find(&layout, "Inner").rect, 1240.0, 130.0, 200.0, 150.0);
        assert_eq!(find(&layout, "Inner").parent, Some(0));
    }

    #[test]
    fn hidden_parent_hides_its_children_but_keeps_their_boxes() {
        let layout = lay(
            "PanelWidgetClass Outer {\n visible 0\n size 0.5 0.5\n {\n PanelWidgetClass Inner {\n  size 0.1 0.1\n }\n }\n}\nPanelWidgetClass Shown {\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        assert!(!find(&layout, "Outer").visible);
        assert!(!find(&layout, "Inner").visible);
        assert!(find(&layout, "Shown").visible);
        assert_rect(find(&layout, "Inner").rect, 0.0, 0.0, 96.0, 54.0);
    }

    #[test]
    fn clipchildren_cuts_children_to_the_parent_box() {
        let layout = lay(
            "FrameWidgetClass ClipOn {\n clipchildren 1\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 1200 450\n size 200 100\n {\n PanelWidgetClass ClipOnChild {\n  hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 150 50\n size 200 100\n }\n }\n}\nFrameWidgetClass ClipOff {\n clipchildren 0\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 1450 450\n size 200 100\n {\n PanelWidgetClass ClipOffChild {\n  hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 150 50\n size 200 100\n }\n }\n}",
            1920,
            1080,
        );
        assert_eq!(find(&layout, "ClipOn").clip, None);
        assert_rect(
            find(&layout, "ClipOnChild").clip.unwrap(),
            1200.0,
            450.0,
            200.0,
            100.0,
        );
        assert_eq!(find(&layout, "ClipOffChild").clip, None);
    }

    #[test]
    fn nested_clips_intersect() {
        let layout = lay(
            "FrameWidgetClass Outer {\n clipchildren 1\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 200 200\n {\n FrameWidgetClass Middle {\n  clipchildren 1\n  hexactpos 1\n  vexactpos 1\n  hexactsize 1\n  vexactsize 1\n  position 100 100\n  size 200 200\n  {\n  PanelWidgetClass Inner {\n   hexactpos 1\n   vexactpos 1\n   hexactsize 1\n   vexactsize 1\n   position 50 50\n   size 200 200\n  }\n  }\n }\n }\n}",
            1920,
            1080,
        );
        assert_rect(
            find(&layout, "Inner").clip.unwrap(),
            100.0,
            100.0,
            100.0,
            100.0,
        );
    }

    #[test]
    fn disjoint_clips_leave_a_zero_size_clip() {
        let layout = lay(
            "FrameWidgetClass Outer {\n clipchildren 1\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 200 200\n {\n FrameWidgetClass Far {\n  clipchildren 1\n  hexactpos 1\n  vexactpos 1\n  hexactsize 1\n  vexactsize 1\n  position 300 300\n  size 100 100\n  {\n  PanelWidgetClass Inner {\n   hexactsize 1\n   vexactsize 1\n   size 10 10\n  }\n  }\n }\n }\n}",
            1920,
            1080,
        );
        let clip = find(&layout, "Inner").clip.unwrap();
        assert_eq!((clip.w, clip.h), (0.0, 0.0));
    }

    #[test]
    fn higher_priority_sibling_is_painted_later() {
        let layout = lay(
            "PanelWidgetClass High {\n priority 200\n size 0.1 0.1\n}\nPanelWidgetClass Low {\n priority 100\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        assert!(find(&layout, "High").draw_order > find(&layout, "Low").draw_order);
    }

    #[test]
    fn parent_is_painted_before_its_children() {
        let layout = lay(
            "PanelWidgetClass Outer {\n priority 500\n size 0.5 0.5\n {\n PanelWidgetClass Inner {\n  size 0.1 0.1\n }\n }\n}\nPanelWidgetClass Plain {\n priority 300\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        let order = |name: &str| find(&layout, name).draw_order;
        assert!(order("Outer") < order("Inner"));
        assert!(order("Plain") < order("Outer"));
    }

    #[test]
    fn equal_priority_keeps_file_order() {
        let layout = lay(
            "PanelWidgetClass First {\n size 0.1 0.1\n}\nPanelWidgetClass Second {\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        assert!(find(&layout, "First").draw_order < find(&layout, "Second").draw_order);
    }

    #[test]
    fn fixaspect_sizes_follow_the_parent_box() {
        let cases = [
            ("inside", 96.0, 894.96, 230.4, 46.08),
            ("outside", 96.0, 810.0, 230.4, 216.0),
            ("fixwidth", 157.2, 907.2, 108.0, 21.6),
        ];
        for (aspect, x, y, w, h) in cases {
            let text = format!(
                "FrameWidgetClass Parent {{\n position 0.05 0.75\n size 0.12 0.2\n {{\n PanelWidgetClass Aspect {{\n  halign center_ref\n  valign center_ref\n  position 0 0\n  size 0.5 0.1\n  fixaspect {aspect}\n }}\n }}\n}}"
            );
            assert_rect(find(&lay(&text, 1920, 1080), "Aspect").rect, x, y, w, h);
        }
    }

    #[test]
    fn fixwidth_keeps_exact_sizes_in_pixels() {
        let text = "FrameWidgetClass Parent {\n size 1 1\n {\n ButtonWidgetClass Button {\n  size 38 38\n  hexactsize 1\n  vexactsize 1\n  fixaspect fixwidth\n }\n }\n}";
        for (width, height, size) in [(1920, 1080, 38.0), (1280, 720, 38.0 * 720.0 / 1080.0)] {
            let rect = find(&lay(text, width, height), "Button").rect;
            assert_rect(rect, rect.x, rect.y, size, size);
        }
    }

    #[test]
    fn text_widgets_keep_their_written_size_under_fixaspect() {
        for class in [
            "TextWidgetClass",
            "MultilineTextWidgetClass",
            "RichTextWidgetClass",
        ] {
            let text = format!(
                "FrameWidgetClass Parent {{\n size 1 1\n {{\n {class} Label {{\n  position 100 364\n  size 444 100\n  hexactpos 1\n  vexactpos 1\n  hexactsize 1\n  vexactsize 1\n  fixaspect outside\n }}\n }}\n}}"
            );
            let layout = lay(&text, 1920, 1080);
            assert_rect(find(&layout, "Label").rect, 100.0, 364.0, 444.0, 100.0);
            assert!(layout.findings.is_empty(), "{:?}", layout.findings);
        }
    }

    #[test]
    fn a_text_widget_with_no_size_is_reported_even_with_fixaspect() {
        let layout = lay("TextWidgetClass Label {\n fixaspect outside\n}", 1920, 1080);
        let codes: Vec<_> = layout.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["MF-L208"]);
    }

    #[test]
    fn color_is_read_or_opaque_white() {
        let cases = [
            ("color 1 0.5 0 0.25", [1.0, 0.5, 0.0, 0.25]),
            ("color 1 0.5", [1.0; 4]),
            ("size 0.1 0.1", [1.0; 4]),
        ];
        for (line, want) in cases {
            let text = format!("PanelWidgetClass Box {{\n {line}\n size 0.1 0.1\n}}");
            assert_eq!(lay(&text, 1920, 1080).boxes[0].color, want, "{line}");
        }
    }

    #[test]
    fn script_params_blocks_are_not_widgets() {
        let layout = lay(
            "PanelWidgetClass Box {\n size 0.5 0.5\n {\n ScriptParamsClass Params {\n  AlignChilds 1\n  {\n  PanelWidgetClass Hidden {\n  }\n  }\n }\n PanelWidgetClass Kid {\n  size 0.1 0.1\n }\n }\n}",
            1920,
            1080,
        );
        let names: Vec<_> = layout
            .boxes
            .iter()
            .filter_map(|item| item.name.as_deref())
            .collect();
        assert_eq!(names, ["Box", "Kid"]);
    }

    #[test]
    fn layout_json_uses_the_field_names() {
        let layout = lay("PanelWidgetClass Box {\n size 0.1 0.1\n}", 1920, 1080);
        let json = serde_json::to_value(&layout).unwrap();
        let item = &json["boxes"][0];
        assert_eq!(
            json["screen"],
            serde_json::json!({"width": 1920, "height": 1080})
        );
        assert_eq!(
            item["rect"],
            serde_json::json!({"x": 0.0, "y": 0.0, "w": 192.0, "h": 108.0})
        );
        assert!(item["clip"].is_null());
        for key in [
            "index",
            "parent",
            "depth",
            "class",
            "name",
            "line",
            "column",
            "visible",
            "color",
            "draw_order",
        ] {
            assert!(item.get(key).is_some(), "{key}");
        }
        assert!(json["findings"].is_array());
    }

    #[test]
    fn script_class_warns_with_the_widget_name() {
        let layout = lay(
            "PanelWidgetClass Moved {\n scriptclass \"SizeToChild\"\n size 0.1 0.1\n}\nPanelWidgetClass Quiet {\n scriptclass \"\"\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        assert_eq!(layout.findings.len(), 1);
        let finding = &layout.findings[0];
        assert_eq!(finding.code, "MF-L201");
        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(
            finding.message,
            "script class SizeToChild may move or resize this widget: Moved"
        );
        assert_eq!(finding.span.map(|span| span.start), Some(0));
    }

    #[test]
    fn fixaspect_outside_gets_a_hint() {
        let layout = lay(
            "PanelWidgetClass Cover {\n size 0.5 0.1\n fixaspect outside\n}",
            1920,
            1080,
        );
        assert_eq!(layout.findings.len(), 1);
        let finding = &layout.findings[0];
        assert_eq!(finding.code, "MF-L202");
        assert_eq!(finding.severity, Severity::Hint);
        assert_eq!(
            finding.message,
            "fixaspect outside is drawn as filling the parent (measured); a clipped cover looks the same: Cover"
        );
    }

    #[test]
    fn unmeasured_settings_get_a_hint() {
        let cases = [
            (
                "hexactsize 1\n vexactsize 1\n size 200 100\n fixaspect inside",
                "fixaspect inside with exact sizes",
            ),
            (
                "hexactpos 1\n vexactpos 1\n size 200 100\n fixaspect inside",
                "fixaspect inside with exact positions",
            ),
            ("size 0.1 0.1\n halign bottom", "halign value bottom"),
            ("size 0.1 0.1\n valign middle", "valign value middle"),
            (
                "size 0.1 0.1\n fixaspect sideways",
                "fixaspect value sideways",
            ),
            (
                "hexactsize 1\n size 200 0.1\n fixaspect sideways",
                "fixaspect value sideways",
            ),
        ];
        for (body, what) in cases {
            let text = format!("PanelWidgetClass Box {{\n {body}\n}}");
            let layout = lay(&text, 1920, 1080);
            assert_eq!(layout.findings.len(), 1, "{what}");
            assert_eq!(layout.findings[0].code, "MF-L203");
            assert_eq!(layout.findings[0].severity, Severity::Hint);
            assert_eq!(
                layout.findings[0].message,
                format!("not measured in the game: {what}: Box")
            );
        }
    }

    #[test]
    fn spacers_get_no_hint() {
        let grid = lay(&grid_text("Columns 1", "100 100", 1), 1920, 1080);
        assert!(grid.findings.is_empty());
        let wrap = lay(&wrap_text("", "100 100", &[("A", "10 10")]), 1920, 1080);
        assert!(wrap.findings.is_empty());
    }

    fn findings_of(layout: &Layout) -> Vec<(&str, &str)> {
        layout
            .findings
            .iter()
            .map(|item| (item.code.as_str(), item.message.as_str()))
            .collect()
    }

    #[test]
    fn grid_hints_when_margin_and_padding_differ() {
        let differ = lay(
            &grid_text("Columns 1\n Margin 10\n Padding 0", "100 100", 1),
            1920,
            1080,
        );
        assert_eq!(
            findings_of(&differ),
            [(
                "MF-L205",
                "which of Margin and Padding is the gap was not measured (tooltips: Padding is the outer margin): Grid"
            )]
        );
        assert_eq!(differ.findings[0].severity, Severity::Hint);
        let same = lay(
            &grid_text("Columns 1\n Margin 10\n Padding 10", "100 100", 1),
            1920,
            1080,
        );
        assert!(same.findings.is_empty());
    }

    #[test]
    fn wrap_hints_when_spacing_or_child_position_is_set() {
        let positioned = "WrapSpacerWidgetClass Wrap {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 200\n {\n PanelWidgetClass A {\n  hexactsize 1\n  vexactsize 1\n  position 5 0\n  size 120 60\n }\n }\n}";
        let texts = [
            wrap_text("Margin 5", "300 200", &[("A", "120 60")]),
            wrap_text("Padding 5", "300 200", &[("A", "120 60")]),
            positioned.to_string(),
        ];
        for text in texts {
            assert_eq!(
                findings_of(&lay(&text, 1920, 1080)),
                [(
                    "MF-L206",
                    "wrap spacing and child position are not measured: Wrap"
                )]
            );
        }
        let plain = lay(&wrap_text("", "300 200", &[("A", "120 60")]), 1920, 1080);
        assert!(plain.findings.is_empty());
    }

    #[test]
    fn grid_hints_when_its_shape_is_not_measured() {
        let cases = [
            ("", "Columns missing or below 1", 1),
            ("Columns 0", "Columns missing or below 1", 1),
            (
                "Columns 1\n Rows 1",
                "more children than Columns times Rows",
                2,
            ),
            (
                "Columns 1\n \"Ignore invisible\" 1",
                "Ignore invisible 1",
                1,
            ),
        ];
        for (props, what, children) in cases {
            let layout = lay(&grid_text(props, "300 100", children), 1920, 1080);
            let message = format!("grid shape not measured: {what}: Grid");
            assert_eq!(
                findings_of(&layout),
                [("MF-L207", message.as_str())],
                "{props}"
            );
        }
    }

    #[test]
    fn widget_without_a_size_gets_a_hint() {
        let layout = lay("PanelWidgetClass Box {\n color 1 1 1 1\n}", 1920, 1080);
        assert_eq!(
            findings_of(&layout),
            [("MF-L208", "no size written, drawn as 0 x 0: Box")]
        );
    }

    #[test]
    fn fixaspect_inside_needs_a_size_ratio() {
        let layout = lay(
            "PanelWidgetClass Box {\n size 0 0.1\n fixaspect inside\n}",
            1920,
            1080,
        );
        assert_eq!(
            findings_of(&layout),
            [("MF-L209", "fixaspect inside needs a size ratio: Box")]
        );
    }

    #[test]
    fn size_flags_are_per_axis() {
        let cases = [
            ("1 1", "200 100", 133.33, 66.67),
            ("0 0", "0.2 0.2", 256.0, 144.0),
            ("0 1", "0.2 100", 256.0, 66.67),
            ("1 0", "200 0.2", 133.33, 144.0),
        ];
        for (flags, size, w, h) in cases {
            let (hexact, vexact) = flags.split_once(' ').unwrap();
            let text = format!(
                "PanelWidgetClass Box {{\n hexactsize {hexact}\n vexactsize {vexact}\n size {size}\n}}"
            );
            let rect = lay(&text, 1280, 720).boxes[0].rect;
            assert_rect(rect, 0.0, 0.0, w, h);
        }
    }

    #[test]
    fn overflowing_numbers_count_as_missing() {
        let layout = lay("PanelWidgetClass Box {\n size 1e999 0.1\n}", 1920, 1080);
        let rect = layout.boxes[0].rect;
        assert_eq!((rect.w, rect.h), (0.0, 108.0));
    }

    #[test]
    fn negative_zero_priority_keeps_file_order() {
        let layout = lay(
            "PanelWidgetClass Plain {\n size 0.1 0.1\n}\nPanelWidgetClass NegZero {\n priority -0\n size 0.1 0.1\n}",
            1920,
            1080,
        );
        assert!(find(&layout, "Plain").draw_order < find(&layout, "NegZero").draw_order);
    }

    #[test]
    fn text_shorter_than_the_widgets_still_resolves() {
        let text =
            "PanelWidgetClass A {\n size 0.1 0.1\n}\nPanelWidgetClass B {\n size 0.1 0.1\n scriptclass \"X\"\n}\n";
        let screen = Screen {
            width: 1920,
            height: 1080,
        };
        let layout = resolve(&parse(text).unwrap(), "", screen);
        assert_eq!(layout.findings.len(), 1);
    }

    #[test]
    fn huge_numbers_keep_every_box_finite() {
        let text = "PanelWidgetClass Big {\n position 3e38 3e38\n size 3e38 3e38\n halign center_ref\n clipchildren 1\n {\n PanelWidgetClass Kid {\n  position 3e38 0\n  size 0.5 0.5\n  halign center_ref\n }\n }\n}";
        for widget in lay(text, 1920, 1080).boxes {
            let all = [widget.rect].into_iter().chain(widget.clip);
            for rect in all {
                assert!([rect.x, rect.y, rect.w, rect.h]
                    .iter()
                    .all(|n| n.is_finite()));
            }
        }
    }

    #[test]
    fn anchors_place_the_box_against_the_parent_edge() {
        let cases = [
            ("left_ref", "top_ref", 50.0, 30.0),
            ("center_ref", "center_ref", 910.0, 520.0),
            ("right_ref", "bottom_ref", 1670.0, 950.0),
        ];
        for (halign, valign, x, y) in cases {
            let text = format!(
                "PanelWidgetClass Box {{\n halign {halign}\n valign {valign}\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 50 30\n size 200 100\n}}"
            );
            assert_rect(lay(&text, 1920, 1080).boxes[0].rect, x, y, 200.0, 100.0);
        }
    }

    fn grid_text(props: &str, size: &str, children: usize) -> String {
        let cells: String = (0..children)
            .map(|i| format!(" PanelWidgetClass Cell{i} {{\n  size 1 1\n }}\n"))
            .collect();
        format!(
            "GridSpacerWidgetClass Grid {{\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size {size}\n {props}\n {{\n{cells} }}\n}}"
        )
    }

    #[test]
    fn grid_children_take_cells_in_a_row() {
        let layout = lay(&grid_text("Columns 3\n Rows 1", "300 100", 3), 1920, 1080);
        for (i, x) in [0.0, 100.0, 200.0].into_iter().enumerate() {
            assert_rect(
                find(&layout, &format!("Cell{i}")).rect,
                x,
                0.0,
                100.0,
                100.0,
            );
        }
    }

    fn assert_cells(layout: &Layout, cells: &[[f32; 4]]) {
        for (i, &[x, y, w, h]) in cells.iter().enumerate() {
            assert_rect(find(layout, &format!("Cell{i}")).rect, x, y, w, h);
        }
    }

    #[test]
    fn grid_rows_come_from_rows_or_the_child_count() {
        let cases: [(&str, &str, usize, &[[f32; 4]]); 2] = [
            (
                "Columns 2\n Rows 3",
                "200 300",
                2,
                &[[0.0, 0.0, 100.0, 100.0], [100.0, 0.0, 100.0, 100.0]],
            ),
            (
                "Columns 2",
                "200 200",
                3,
                &[
                    [0.0, 0.0, 100.0, 100.0],
                    [100.0, 0.0, 100.0, 100.0],
                    [0.0, 100.0, 100.0, 100.0],
                ],
            ),
        ];
        for (props, size, children, cells) in cases {
            let layout = lay(&grid_text(props, size, children), 1920, 1080);
            assert_cells(&layout, cells);
        }
    }

    #[test]
    fn grid_gap_is_margin_between_cells() {
        let layout = lay(
            &grid_text("Columns 2\n Rows 1\n Margin 10", "210 100", 2),
            1920,
            1080,
        );
        assert_cells(
            &layout,
            &[[0.0, 0.0, 100.0, 100.0], [110.0, 0.0, 100.0, 100.0]],
        );
    }

    #[test]
    fn grid_padding_insets_the_cells() {
        let layout = lay(
            &grid_text("Columns 2\n Rows 1\n Padding 10", "220 120", 2),
            1920,
            1080,
        );
        assert_cells(
            &layout,
            &[[10.0, 10.0, 100.0, 100.0], [110.0, 10.0, 100.0, 100.0]],
        );
    }

    #[test]
    fn grid_child_past_the_last_cell_starts_an_extra_row() {
        let layout = lay(&grid_text("Columns 2\n Rows 1", "200 100", 3), 1920, 1080);
        assert_cells(
            &layout,
            &[
                [0.0, 0.0, 100.0, 100.0],
                [100.0, 0.0, 100.0, 100.0],
                [0.0, 100.0, 100.0, 100.0],
            ],
        );
    }

    #[test]
    fn grid_padding_larger_than_the_box_leaves_no_size() {
        let layout = lay(
            &grid_text("Columns 1\n Padding 100", "150 150", 1),
            1920,
            1080,
        );
        assert_cells(&layout, &[[100.0, 100.0, 0.0, 0.0]]);
    }

    fn hidden_grid_text(ignore_hidden: &str) -> String {
        format!(
            "GridSpacerWidgetClass Grid {{\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 100\n Columns 3\n Rows 1\n{ignore_hidden}\n {{\n PanelWidgetClass A {{\n  size 1 1\n }}\n PanelWidgetClass B {{\n  visible 0\n  size 1 1\n }}\n PanelWidgetClass C {{\n  size 1 1\n }}\n }}\n}}"
        )
    }

    #[test]
    fn hidden_grid_child_keeps_its_cell_by_default() {
        let layout = lay(&hidden_grid_text(""), 1920, 1080);
        assert_rect(find(&layout, "B").rect, 100.0, 0.0, 100.0, 100.0);
        assert_rect(find(&layout, "C").rect, 200.0, 0.0, 100.0, 100.0);
    }

    #[test]
    fn ignored_hidden_grid_child_takes_no_cell() {
        let layout = lay(&hidden_grid_text("\n \"Ignore invisible\" 1"), 1920, 1080);
        assert_rect(find(&layout, "A").rect, 0.0, 0.0, 100.0, 100.0);
        assert_rect(find(&layout, "C").rect, 100.0, 0.0, 100.0, 100.0);
        assert_rect(find(&layout, "B").rect, 0.0, 0.0, 300.0, 100.0);
    }

    #[test]
    fn grid_child_is_sized_and_anchored_inside_its_cell() {
        let text = "GridSpacerWidgetClass Grid {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 100\n Columns 3\n Rows 1\n {\n PanelWidgetClass Corner {\n  size 0.5 0.5\n  halign right_ref\n  valign bottom_ref\n }\n }\n}";
        let layout = lay(text, 1920, 1080);
        assert_rect(find(&layout, "Corner").rect, 50.0, 50.0, 50.0, 50.0);
    }

    #[test]
    fn grid_child_keeps_its_own_children_inside_its_cell() {
        let text = "GridSpacerWidgetClass Grid {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 100\n Columns 3\n Rows 1\n {\n PanelWidgetClass First {\n  size 1 1\n }\n PanelWidgetClass Second {\n  size 1 1\n  {\n  PanelWidgetClass Inner {\n   size 0.5 0.5\n  }\n  }\n }\n }\n}";
        let layout = lay(text, 1920, 1080);
        assert_rect(find(&layout, "Inner").rect, 100.0, 0.0, 50.0, 50.0);
        assert_eq!(
            find(&layout, "Inner").parent,
            Some(find(&layout, "Second").index)
        );
    }

    fn wrap_text(props: &str, size: &str, children: &[(&str, &str)]) -> String {
        let kids: String = children
            .iter()
            .map(|(name, child_size)| {
                format!(" PanelWidgetClass {name} {{\n  hexactsize 1\n  vexactsize 1\n  size {child_size}\n }}\n")
            })
            .collect();
        format!(
            "WrapSpacerWidgetClass Wrap {{\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size {size}\n {props}\n {{\n{kids} }}\n}}"
        )
    }

    #[test]
    fn wrap_flows_children_left_to_right_then_down() {
        let text = wrap_text(
            "",
            "300 200",
            &[("A", "120 60"), ("B", "120 60"), ("C", "120 60")],
        );
        let layout = lay(&text, 1920, 1080);
        assert_rect(find(&layout, "A").rect, 0.0, 0.0, 120.0, 60.0);
        assert_rect(find(&layout, "B").rect, 120.0, 0.0, 120.0, 60.0);
        assert_rect(find(&layout, "C").rect, 0.0, 60.0, 120.0, 60.0);
    }

    #[test]
    fn wrap_rows_take_the_height_of_their_tallest_child() {
        let text = wrap_text(
            "",
            "300 200",
            &[
                ("A", "120 60"),
                ("B", "120 100"),
                ("C", "150 40"),
                ("D", "100 30"),
            ],
        );
        let layout = lay(&text, 1920, 1080);
        assert_rect(find(&layout, "C").rect, 0.0, 100.0, 150.0, 40.0);
        assert_rect(find(&layout, "D").rect, 150.0, 100.0, 100.0, 30.0);
    }

    #[test]
    fn wrap_margin_is_the_gap_between_items_and_rows() {
        let text = wrap_text(
            "Margin 10",
            "300 200",
            &[("A", "120 60"), ("B", "120 60"), ("C", "120 60")],
        );
        let layout = lay(&text, 1920, 1080);
        assert_rect(find(&layout, "B").rect, 130.0, 0.0, 120.0, 60.0);
        assert_rect(find(&layout, "C").rect, 0.0, 70.0, 120.0, 60.0);
    }

    #[test]
    fn wrap_ignores_child_position_and_insets_by_padding() {
        let text = "WrapSpacerWidgetClass Wrap {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 0 0\n size 300 200\n Padding 10\n {\n PanelWidgetClass A {\n  hexactsize 1\n  vexactsize 1\n  position 50 50\n  size 120 60\n }\n }\n}";
        let layout = lay(text, 1920, 1080);
        assert_rect(find(&layout, "A").rect, 10.0, 10.0, 120.0, 60.0);
    }

    #[test]
    fn wrap_keeps_a_first_child_wider_than_the_box_on_its_own_row() {
        let text = wrap_text("", "100 100", &[("A", "150 40"), ("B", "50 40")]);
        let layout = lay(&text, 1920, 1080);
        assert_rect(find(&layout, "A").rect, 0.0, 0.0, 150.0, 40.0);
        assert_rect(find(&layout, "B").rect, 0.0, 40.0, 50.0, 40.0);
    }

    #[test]
    fn wrap_breaks_only_past_the_edge_by_more_than_the_slack() {
        let fits = wrap_text("", "100 100", &[("A", "50 40"), ("B", "50 40")]);
        let past = wrap_text("", "100 100", &[("A", "50 40"), ("B", "50.05 40")]);
        assert_rect(
            find(&lay(&fits, 1920, 1080), "B").rect,
            50.0,
            0.0,
            50.0,
            40.0,
        );
        assert_rect(
            find(&lay(&past, 1920, 1080), "B").rect,
            0.0,
            40.0,
            50.05,
            40.0,
        );
    }

    #[test]
    fn a_cover_without_a_size_is_not_reported_as_empty() {
        let layout = lay(
            "PanelWidgetClass Cover {\n fixaspect outside\n}",
            1920,
            1080,
        );
        assert!(layout.findings.iter().all(|item| item.code != "MF-L208"));
    }
}
