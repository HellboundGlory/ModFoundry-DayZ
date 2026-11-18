//! Lays out and draws a line of text from a parsed `.fnt` font.

use crate::fnt_read::{FntFont, FntGlyph, SdfInfo};
use modfoundry_texture::RgbaImage;
use std::ops::Range;

/// Horizontal alignment of each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// How a string is set: its height in pixels, optional wrapping and alignment.
#[derive(Debug, Clone, PartialEq)]
pub struct TextOptions {
    /// Text height in screen pixels: the em for FNT5, the cell height for FNT3.
    pub em_px: f32,
    /// Breaks lines wider than this at spaces.
    pub wrap_width: Option<f32>,
    /// Aligns each line inside `wrap_width`, or inside the widest line when `None`.
    pub halign: HAlign,
}

/// One glyph on the block, its quad relative to the block's top-left.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedGlyph {
    pub codepoint: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Atlas rectangle in texels: x, y, w, h.
    pub src: [f32; 4],
}

/// A laid-out string.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub glyphs: Vec<PlacedGlyph>,
    /// Advance width of the widest line.
    pub width: f32,
    /// Lines times `em_px`.
    pub height: f32,
    pub lines: usize,
}

/// Places each glyph of `text` in lines, wrapping and aligning them as `options` says.
pub fn layout_text(font: &FntFont, text: &str, options: &TextOptions) -> TextBlock {
    let body = text.strip_suffix('\n').unwrap_or(text);
    if body.trim_matches('\r').is_empty() {
        return TextBlock {
            glyphs: Vec::new(),
            width: 0.0,
            height: 0.0,
            lines: 0,
        };
    }
    let scale = scale_of(font, options.em_px);
    let lines: Vec<Line> = body
        .split('\n')
        .flat_map(|paragraph| paragraph_lines(font, paragraph, scale, options.wrap_width))
        .collect();
    let width = lines.iter().map(|line| line.width).fold(0.0, f32::max);
    let box_width = options.wrap_width.unwrap_or(width);
    let mut glyphs = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_top = index as f32 * options.em_px;
        let left = align_offset(options.halign, box_width, line.width);
        glyphs.extend(line.placements.iter().filter_map(|&(glyph, pen)| {
            glyph.map(|glyph| place(font, glyph, scale, left + pen, line_top))
        }));
    }
    TextBlock {
        glyphs,
        width,
        height: lines.len() as f32 * options.em_px,
        lines: lines.len(),
    }
}

struct Run<'a> {
    codepoint: u32,
    /// `None` for a space the font has no glyph for: it advances the pen and draws nothing.
    glyph: Option<&'a FntGlyph>,
    advance: f32,
    space: bool,
}

struct Line<'a> {
    placements: Vec<(Option<&'a FntGlyph>, f32)>,
    width: f32,
}

fn paragraph_lines<'a>(
    font: &'a FntFont,
    paragraph: &str,
    scale: f32,
    wrap_width: Option<f32>,
) -> Vec<Line<'a>> {
    let runs: Vec<Run> = paragraph
        .chars()
        .filter(|&ch| ch != '\r')
        .filter_map(|ch| run_for(font, ch))
        .collect();
    line_ranges(font, &runs, scale, wrap_width)
        .into_iter()
        .map(|range| {
            let line_runs = &runs[range];
            let pens = pen_positions(font, line_runs, scale);
            let width = pens.last().copied().unwrap_or(0.0);
            Line {
                placements: line_runs.iter().map(|run| run.glyph).zip(pens).collect(),
                width,
            }
        })
        .collect()
}

fn line_ranges(
    font: &FntFont,
    runs: &[Run],
    scale: f32,
    wrap_width: Option<f32>,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let (mut start, mut last_end, mut index) = (0, 0, 0);
    while index < runs.len() {
        if runs[index].space {
            index += 1;
            continue;
        }
        let end = runs[index..]
            .iter()
            .position(|run| run.space)
            .map_or(runs.len(), |offset| index + offset);
        let line_has_word = last_end > start;
        let overflows = wrap_width.is_some_and(|limit| {
            line_has_word && advance_width(font, &runs[start..end], scale) > limit
        });
        if overflows {
            ranges.push(start..last_end);
            start = index;
        }
        last_end = end;
        index = end;
    }
    ranges.push(start..runs.len());
    ranges
}

fn run_for(font: &FntFont, ch: char) -> Option<Run<'_>> {
    let codepoint = u32::from(ch);
    if ch == ' ' && font.glyph(codepoint).is_none() {
        return Some(Run {
            codepoint,
            glyph: None,
            advance: missing_space_advance(font),
            space: true,
        });
    }
    font.glyph_or_fallback(codepoint).map(|glyph| Run {
        codepoint,
        glyph: Some(glyph),
        advance: f32::from(glyph.advance),
        space: ch == ' ',
    })
}

/// The advance of a space the font has no glyph for: its size for FNT3, a quarter em for FNT5.
fn missing_space_advance(font: &FntFont) -> f32 {
    match &font.sdf {
        Some(sdf) => sdf.em / 4.0,
        None => font.size as f32,
    }
}

/// Pen x before each run, then the total advance as the last entry.
fn pen_positions(font: &FntFont, runs: &[Run], scale: f32) -> Vec<f32> {
    let mut positions = Vec::with_capacity(runs.len() + 1);
    let mut pen = 0.0;
    let mut previous: Option<u32> = None;
    for run in runs {
        if let Some(left) = previous {
            pen += font.kern(left, run.codepoint) as f32 * scale;
        }
        positions.push(pen);
        pen += run.advance * scale;
        previous = Some(run.codepoint);
    }
    positions.push(pen);
    positions
}

fn advance_width(font: &FntFont, runs: &[Run], scale: f32) -> f32 {
    pen_positions(font, runs, scale)
        .last()
        .copied()
        .unwrap_or(0.0)
}

/// Offset of a line or block of `line_width` inside a box of `box_width`.
pub fn align_offset(halign: HAlign, box_width: f32, line_width: f32) -> f32 {
    match halign {
        HAlign::Left => 0.0,
        HAlign::Center => (box_width - line_width) / 2.0,
        HAlign::Right => box_width - line_width,
    }
}

fn scale_of(font: &FntFont, em_px: f32) -> f32 {
    match &font.sdf {
        Some(sdf) => em_px / sdf.em,
        None => em_px / font.line_height(),
    }
}

fn place(font: &FntFont, glyph: &FntGlyph, scale: f32, pen: f32, line_top: f32) -> PlacedGlyph {
    let (x, y) = (f32::from(glyph.x), f32::from(glyph.y));
    let (w, h) = (f32::from(glyph.w), f32::from(glyph.h));
    let left = pen + f32::from(glyph.bearing_x) * scale;
    match &font.sdf {
        Some(sdf) => {
            let baseline = line_top + scale * (sdf.em + sdf.cap_height) / 2.0;
            let top = baseline - f32::from(glyph.bearing_y.unwrap_or(0)) * scale;
            let cell = sdf.cell as f32;
            PlacedGlyph {
                codepoint: glyph.codepoint,
                x: left,
                y: top,
                w: w * scale,
                h: h * scale,
                src: [x + (cell - w) / 2.0, y + (cell - h) / 2.0, w, h],
            }
        }
        None => PlacedGlyph {
            codepoint: glyph.codepoint,
            x: left,
            y: line_top,
            w: w * scale,
            h: h * scale,
            src: [x, y, w, h],
        },
    }
}

/// `clip` is a pixel rectangle `[x0, y0, x1, y1]` (exclusive end); `origin` is the block's top-left on the target.
pub fn draw_text(
    target: &mut RgbaImage,
    atlas: &RgbaImage,
    font: &FntFont,
    block: &TextBlock,
    origin: (f32, f32),
    color: [f32; 4],
    clip: Option<[i32; 4]>,
) {
    if atlas.width == 0 || atlas.height == 0 {
        return;
    }
    let (width, height) = (target.width as i32, target.height as i32);
    let bounds = match clip {
        Some([x0, y0, x1, y1]) => [x0.max(0), y0.max(0), x1.min(width), y1.min(height)],
        None => [0, 0, width, height],
    };
    let paint = Paint {
        origin,
        color,
        bounds,
    };
    for glyph in &block.glyphs {
        if glyph.w <= 0.0 || glyph.h <= 0.0 {
            continue;
        }
        let scale = glyph.h / glyph.src[3];
        let edge = font
            .sdf
            .as_ref()
            .map(|sdf| EDGE_SHARPNESS * sdf_range(sdf) * scale);
        draw_glyph(target, atlas, glyph, edge, &paint);
    }
}

const EDGE_SHARPNESS: f32 = 1.5 / std::f32::consts::SQRT_2;

fn sdf_range(sdf: &SdfInfo) -> f32 {
    let range = f32::from(sdf.range);
    if sdf.em > 40.0 {
        (sdf.em + range) / 3.0
    } else {
        range
    }
}

struct Paint {
    origin: (f32, f32),
    color: [f32; 4],
    bounds: [i32; 4],
}

fn draw_glyph(
    target: &mut RgbaImage,
    atlas: &RgbaImage,
    glyph: &PlacedGlyph,
    edge: Option<f32>,
    paint: &Paint,
) {
    let left = paint.origin.0 + glyph.x;
    let top = paint.origin.1 + glyph.y;
    let [min_x, min_y, max_x, max_y] = paint.bounds;
    let x0 = min_x.max(left.floor() as i32);
    let y0 = min_y.max(top.floor() as i32);
    let x1 = max_x.min((left + glyph.w).ceil() as i32);
    let y1 = max_y.min((top + glyph.h).ceil() as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            if cx < left || cx >= left + glyph.w || cy < top || cy >= top + glyph.h {
                continue;
            }
            let u = glyph.src[0] + (cx - left) / glyph.w * glyph.src[2];
            let v = glyph.src[1] + (cy - top) / glyph.h * glyph.src[3];
            let texel = sample(atlas, glyph.src, u, v, edge.is_some());
            let coverage = match edge {
                Some(gain) => edge_coverage(texel, gain),
                None => texel[3] / 255.0,
            };
            blend(target, x, y, paint.color, coverage * paint.color[3]);
        }
    }
}

fn edge_coverage(texel: [f32; 4], gain: f32) -> f32 {
    let [r, g, b, _] = texel;
    let median = r.max(g).min(b).max(r.min(g));
    let distance = median / 255.0 - 0.5;
    (distance * gain + 0.5).clamp(0.0, 1.0)
}

/// Bilinear sample in atlas texels. Taps stay inside the glyph's rectangle, or one texel past it
/// for distance fields, whose cell padding and gap hold the field around the glyph.
fn sample(atlas: &RgbaImage, src: [f32; 4], u: f32, v: f32, field: bool) -> [f32; 4] {
    let (fx, fy) = (u - 0.5, v - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let margin = i64::from(field);
    let columns = span(src[0], src[2], atlas.width, margin);
    let rows = span(src[1], src[3], atlas.height, margin);
    let tap = |dx: i64, dy: i64| texel(atlas, x0 as i64 + dx, y0 as i64 + dy, columns, rows);
    let (top_left, top_right) = (tap(0, 0), tap(1, 0));
    let (bottom_left, bottom_right) = (tap(0, 1), tap(1, 1));
    std::array::from_fn(|channel| {
        let top = top_left[channel] + (top_right[channel] - top_left[channel]) * tx;
        let bottom = bottom_left[channel] + (bottom_right[channel] - bottom_left[channel]) * tx;
        top + (bottom - top) * ty
    })
}

fn span(start: f32, len: f32, limit: u32, margin: i64) -> (i64, i64) {
    let top = i64::from(limit) - 1;
    let first = (start.floor() as i64 - margin).clamp(0, top);
    let last = ((start + len).ceil() as i64 - 1 + margin).clamp(first, top);
    (first, last)
}

fn texel(atlas: &RgbaImage, x: i64, y: i64, columns: (i64, i64), rows: (i64, i64)) -> [f32; 4] {
    let x = x.clamp(columns.0, columns.1) as usize;
    let y = y.clamp(rows.0, rows.1) as usize;
    let at = (y * atlas.width as usize + x) * 4;
    let pixel = &atlas.pixels[at..at + 4];
    std::array::from_fn(|channel| f32::from(pixel[channel]))
}

fn blend(target: &mut RgbaImage, x: i32, y: i32, color: [f32; 4], coverage: f32) {
    let alpha = coverage.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let at = (y as usize * target.width as usize + x as usize) * 4;
    let pixel = &mut target.pixels[at..at + 4];
    let below_alpha = f32::from(pixel[3]) / 255.0;
    let out_alpha = alpha + below_alpha * (1.0 - alpha);
    for channel in 0..3 {
        let below = f32::from(pixel[channel]) / 255.0;
        let mixed = (color[channel] * alpha + below * below_alpha * (1.0 - alpha)) / out_alpha;
        pixel[channel] = to_byte(mixed);
    }
    pixel[3] = to_byte(out_alpha);
}

fn to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}
