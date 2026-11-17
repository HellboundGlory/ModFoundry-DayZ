//! Rasterising a font file into the glyphs `build_font` writes to FNT3.

use anyhow::{ensure, Result};
use swash::scale::{Render, ScaleContext, Scaler, Source};
use swash::zeno::{Format, Placement};
use swash::{FontRef, GlyphMetrics};
use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};
use ttf_parser::{Face, GlyphId, Tag};

use crate::{Glyph, RenderedFont};

const SPACE: u16 = 0x20;
const MAX_TEXTURE_AREA: u64 = 4096 * 4096;
pub(crate) const MISSING_BOX: u16 = 0x25A1;

const DEFAULT_RANGES: [(u16, u16); 5] = [
    (0x20, 0x7E),
    (0xA0, 0x17F),
    (0x400, 0x4FF),
    (0x2000, 0x206F),
    (0x20A0, 0x20CF),
];

/// The code points a font keeps. Both sets always hold U+0020 and U+25A1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharSet {
    /// Mapped Basic Latin, Latin-1, Latin Extended-A, Cyrillic, punctuation and currency.
    Default,
    /// Every mapped BMP code point from U+0020 up except U+007F to U+009F.
    All,
}

/// What a bake's size measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sizing {
    /// The cell height, as vanilla's fonts: the em shrinks until the font's Windows ascent
    /// and descent fill the cell, and ink outside them is cut.
    Line,
    /// The em, as DayZ Tools' FontToTga: the cell grows to hold every glyph.
    Em,
}

/// Renders `font` at `size` pixels, hinted, with `sizing` saying what the size measures.
pub fn render_font(font: &[u8], size: u32, chars: CharSet, sizing: Sizing) -> Result<RenderedFont> {
    ensure!(
        (1..=512).contains(&size),
        "the size {size} is outside 1 to 512 pixels"
    );
    let not_a_font = || anyhow::anyhow!("the file is not a font");
    let face = Face::parse(font, 0).map_err(|_| not_a_font())?;
    let font_ref = FontRef::from_index(font, 0).ok_or_else(not_a_font)?;
    let charmap = font_ref.charmap();
    let code_points = code_points(&charmap, chars);

    let (ascent_units, descent_units) = vertical_units(&face);
    let units_per_em = f32::from(face.units_per_em());
    // `scale` is pixels per font unit.
    let (scale, ascent, descent) = match sizing {
        Sizing::Line => {
            ensure!(
                ascent_units + descent_units >= units_per_em / 2.0,
                "the font's Windows ascent and descent are under half its em, so a line-sized \
                 bake would draw it at more than twice the size; use --sizing em"
            );
            let scale = size as f32 / (ascent_units + descent_units);
            let ascent = ((ascent_units * scale).round() as u16).min(size as u16);
            (scale, ascent, size as u16 - ascent)
        }
        Sizing::Em => {
            let scale = size as f32 / units_per_em;
            (
                scale,
                (ascent_units * scale).ceil() as u16,
                (descent_units * scale).ceil() as u16,
            )
        }
    };
    let em = scale * units_per_em;
    let glyph_metrics = font_ref.glyph_metrics(&[]).scale(em);
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font_ref).size(em).hint(true).build();
    let mut render = Render::new(&[Source::Outline]);
    render.format(Format::Alpha);

    let mut glyphs = Vec::with_capacity(code_points.len());
    let mut mapped = Vec::with_capacity(code_points.len());
    let mut ink_area = 0u64;
    for codepoint in code_points {
        let glyph_id = charmap.map(codepoint);
        let mut glyph = if glyph_id == 0 {
            let cell = (sizing == Sizing::Line).then_some((ascent, descent));
            unmapped_glyph(codepoint, em, cell)
        } else {
            mapped.push((codepoint, glyph_id));
            ink_glyph(&render, &mut scaler, &glyph_metrics, codepoint, glyph_id)
        };
        if sizing == Sizing::Line {
            glyph = cut_to_cell(glyph, ascent, descent);
        }
        // Stop early: the atlas holds at least every glyph's ink box.
        ink_area += u64::from(glyph.width) * u64::from(glyph.height);
        ensure!(
            ink_area <= MAX_TEXTURE_AREA,
            "the glyphs at {size} pixels need more than a 4096x4096 texture; \
             use a smaller size or the default characters"
        );
        glyphs.push(glyph);
    }

    Ok(RenderedFont {
        size,
        ascent,
        descent,
        glyphs,
        kerning: kern_pairs(&face, &mapped, scale),
    })
}

/// `cell` is the bake's ascent and descent when the box must fit inside the cell.
fn unmapped_glyph(codepoint: u16, em: f32, cell: Option<(u16, u16)>) -> Glyph {
    if codepoint == MISSING_BOX {
        drawn_box(em, cell)
    } else {
        empty(codepoint, (em / 4.0).round() as i16)
    }
}

/// Keeps the rows from `ascent` above the baseline down to `descent` below it.
fn cut_to_cell(glyph: Glyph, ascent: u16, descent: u16) -> Glyph {
    let rows_above_cell = (glyph.top - ascent as i16).max(0);
    let bottom = glyph.top - glyph.height as i16;
    let rows_below_cell = (-(descent as i16) - bottom).max(0);
    if rows_above_cell == 0 && rows_below_cell == 0 {
        return glyph;
    }
    let kept = glyph.height as i16 - rows_above_cell - rows_below_cell;
    if kept <= 0 {
        return empty(glyph.codepoint, glyph.advance);
    }
    let width = usize::from(glyph.width);
    let first = rows_above_cell as usize * width;
    Glyph {
        coverage: glyph.coverage[first..first + kept as usize * width].to_vec(),
        height: kept as u16,
        top: glyph.top - rows_above_cell,
        ..glyph
    }
}

fn empty(codepoint: u16, advance: i16) -> Glyph {
    Glyph {
        codepoint,
        width: 0,
        height: 0,
        coverage: Vec::new(),
        left: 0,
        top: 0,
        advance,
    }
}

/// Sized like FontToTga's box: two thirds of the em, sitting one row below the baseline, or
/// capped to the cell's ascent and descent so a line-sized bake does not cut its border.
fn drawn_box(em: f32, cell: Option<(u16, u16)>) -> Glyph {
    let below = cell.map_or(1, |(_, descent)| descent.min(1));
    let mut width = (em * 2.0 / 3.0).round() as u16;
    if let Some((ascent, _)) = cell {
        width = width.min(ascent + below);
    }
    let height = width;
    let (columns, rows) = (usize::from(width), usize::from(height));
    let mut coverage = vec![0; columns * rows];
    for row in 0..rows {
        for column in 0..columns {
            let border = row == 0 || column == 0 || row == rows - 1 || column == columns - 1;
            if border {
                coverage[row * columns + column] = 255;
            }
        }
    }
    Glyph {
        codepoint: MISSING_BOX,
        width,
        height,
        coverage,
        left: 1,
        top: height as i16 - below as i16,
        advance: width as i16 + 2,
    }
}

fn ink_glyph(
    render: &Render,
    scaler: &mut Scaler,
    metrics: &GlyphMetrics,
    codepoint: u16,
    glyph_id: u16,
) -> Glyph {
    let advance = metrics.advance_width(glyph_id).round() as i16;
    let Some(image) = render.render(scaler, glyph_id) else {
        return empty(codepoint, advance);
    };
    let Some((left, top, right, bottom)) = ink_bounds(&image.data, &image.placement) else {
        return empty(codepoint, advance);
    };
    let columns = image.placement.width as usize;
    let mut coverage = Vec::with_capacity((right - left + 1) * (bottom - top + 1));
    for row in top..=bottom {
        coverage.extend_from_slice(&image.data[row * columns + left..=row * columns + right]);
    }
    Glyph {
        codepoint,
        width: (right - left + 1) as u16,
        height: (bottom - top + 1) as u16,
        coverage,
        left: image.placement.left as i16 + left as i16,
        top: image.placement.top as i16 - top as i16,
        advance,
    }
}

fn ink_bounds(data: &[u8], placement: &Placement) -> Option<(usize, usize, usize, usize)> {
    let columns = placement.width as usize;
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for row in 0..placement.height as usize {
        for column in 0..columns {
            if data[row * columns + column] != 0 {
                bounds = Some(match bounds {
                    None => (column, row, column, row),
                    Some((left, top, right, bottom)) => (
                        left.min(column),
                        top.min(row),
                        right.max(column),
                        bottom.max(row),
                    ),
                });
            }
        }
    }
    bounds
}

/// The Windows ascent and descent in font units, descent positive; hhea without OS/2.
fn vertical_units(face: &Face) -> (f32, f32) {
    match face.tables().os2 {
        Some(os2) => (
            f32::from(os2.windows_ascender()),
            -f32::from(os2.windows_descender()),
        ),
        None => (
            f32::from(face.tables().hhea.ascender),
            -f32::from(face.tables().hhea.descender),
        ),
    }
}

pub(crate) fn code_points(charmap: &swash::Charmap, chars: CharSet) -> Vec<u16> {
    let mut set = std::collections::BTreeSet::new();
    match chars {
        CharSet::Default => {
            for (first, last) in DEFAULT_RANGES {
                for codepoint in first..=last {
                    if charmap.map(codepoint) != 0 {
                        set.insert(codepoint);
                    }
                }
            }
        }
        CharSet::All => charmap.enumerate(|codepoint, glyph| {
            let in_bmp = (0x20..=0xFFFF).contains(&codepoint);
            if glyph != 0 && in_bmp && !(0x7F..=0x9F).contains(&codepoint) {
                set.insert(codepoint as u16);
            }
        }),
    }
    set.insert(SPACE);
    set.insert(MISSING_BOX);
    set.into_iter().collect()
}

pub(crate) fn kern_pairs(face: &Face, glyphs: &[(u16, u16)], scale: f32) -> Vec<(u16, u16, i32)> {
    match gpos_pair_lookups(face) {
        Some(lookups) => gpos_pairs(&lookups, glyphs, scale),
        None => legacy_pairs(face, glyphs, scale),
    }
}

// A pair sums the first matching adjustment of each `kern` lookup.
fn gpos_pair_lookups<'a>(face: &'a Face<'a>) -> Option<Vec<Vec<PairAdjustment<'a>>>> {
    let gpos = face.tables().gpos?;
    let mut indices: Vec<u16> = gpos
        .features
        .into_iter()
        .filter(|feature| feature.tag == Tag::from_bytes(b"kern"))
        .flat_map(|feature| feature.lookup_indices.into_iter())
        .collect();
    if indices.is_empty() {
        return None;
    }
    indices.sort_unstable();
    indices.dedup();
    Some(
        indices
            .into_iter()
            .filter_map(|index| gpos.lookups.get(index))
            .map(|lookup| {
                lookup
                    .subtables
                    .into_iter::<PositioningSubtable>()
                    .filter_map(|subtable| match subtable {
                        PositioningSubtable::Pair(pair) => Some(pair),
                        _ => None,
                    })
                    .collect()
            })
            .collect(),
    )
}

fn gpos_pairs(
    lookups: &[Vec<PairAdjustment>],
    glyphs: &[(u16, u16)],
    scale: f32,
) -> Vec<(u16, u16, i32)> {
    let mut pairs = Vec::new();
    for &(left, left_glyph) in glyphs {
        let first = GlyphId(left_glyph);
        let covering: Vec<&[PairAdjustment]> = lookups
            .iter()
            .filter(|lookup| {
                lookup
                    .iter()
                    .any(|subtable| subtable.coverage().contains(first))
            })
            .map(Vec::as_slice)
            .collect();
        for &(right, right_glyph) in glyphs {
            let second = GlyphId(right_glyph);
            let units: i32 = covering
                .iter()
                .filter_map(|lookup| {
                    lookup
                        .iter()
                        .find_map(|subtable| pair_adjustment(subtable, first, second))
                })
                .map(i32::from)
                .sum();
            let amount = (units as f32 * scale).round() as i32;
            if amount != 0 {
                pairs.push((left, right, amount));
            }
        }
    }
    pairs
}

fn pair_adjustment(subtable: &PairAdjustment, first: GlyphId, second: GlyphId) -> Option<i16> {
    let (value, _) = match subtable {
        PairAdjustment::Format1 { coverage, sets } => {
            sets.get(coverage.get(first)?)?.get(second)?
        }
        PairAdjustment::Format2 {
            coverage,
            classes,
            matrix,
        } => {
            coverage.get(first)?;
            matrix.get((classes.0.get(first), classes.1.get(second)))?
        }
    };
    Some(value.x_advance)
}

fn legacy_pairs(face: &Face, glyphs: &[(u16, u16)], scale: f32) -> Vec<(u16, u16, i32)> {
    let Some(kern) = face.tables().kern else {
        return Vec::new();
    };
    let subtables: Vec<_> = kern
        .subtables
        .into_iter()
        .filter(|subtable| subtable.horizontal)
        .collect();
    let mut pairs = Vec::new();
    for &(left, left_glyph) in glyphs {
        for &(right, right_glyph) in glyphs {
            let value = subtables.iter().find_map(|subtable| {
                subtable.glyphs_kerning(GlyphId(left_glyph), GlyphId(right_glyph))
            });
            if let Some(value) = value {
                let amount = (f32::from(value) * scale).round() as i32;
                if amount != 0 {
                    pairs.push((left, right, amount));
                }
            }
        }
    }
    pairs
}
