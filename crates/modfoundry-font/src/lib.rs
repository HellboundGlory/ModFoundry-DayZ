//! ModFoundry DayZ fonts: rendered and field glyphs to FNT3, FNT5 and .edds.

use anyhow::{bail, ensure, Result};
use modfoundry_texture::edds::encode_edds;
use modfoundry_texture::mips::{edds_level_count, mip_chain};
use modfoundry_texture::RgbaImage;

mod field;
mod fnt5;
mod render;

pub use field::render_field_font;
pub use fnt5::build_field_font;
pub use render::{render_font, CharSet, Sizing};

const MAX_SIDE: u32 = 4096;

/// One rendered glyph. `coverage` is `width * height` alpha bytes, rows top to bottom.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub codepoint: u16,
    pub width: u16,
    pub height: u16,
    pub coverage: Vec<u8>,
    /// Ink box left edge relative to the pen, pixels.
    pub left: i16,
    /// Rows of ink above the baseline.
    pub top: i16,
    /// Pen advance, whole pixels.
    pub advance: i16,
}

/// A whole font at one size: the cell's ascent and descent in pixels, descent positive.
#[derive(Debug, Clone)]
pub struct RenderedFont {
    pub size: u32,
    pub ascent: u16,
    pub descent: u16,
    pub glyphs: Vec<Glyph>,
    pub kerning: Vec<(u16, u16, i32)>,
}

/// One distance-field glyph. `field` is `cell * cell` RGB texels, rows top to bottom, with the
/// `width` x `height` ink box centred in the tile.
#[derive(Debug, Clone)]
pub struct FieldGlyph {
    pub codepoint: u16,
    pub width: u16,
    pub height: u16,
    /// Ink box left edge relative to the pen, pixels at `em`.
    pub left: i16,
    /// Ink box top above the baseline, pixels at `em`.
    pub top: i16,
    pub advance: i16,
    pub field: Vec<[u8; 3]>,
}

/// A whole multi-channel distance-field font, baked once at `em` pixels.
#[derive(Debug, Clone)]
pub struct FieldFont {
    pub em: u32,
    /// Distance-field range in pixels: the field goes from 0 to 255 across this width.
    pub range: u16,
    pub cap_height: f32,
    /// Side of the square tile every glyph's field sits in.
    pub cell: u16,
    pub glyphs: Vec<FieldGlyph>,
    pub kerning: Vec<(u16, u16, i32)>,
}

/// The two files a font becomes: the `.fnt` bytes and the `.edds` bytes.
#[derive(Debug, Clone)]
pub struct BuiltFont {
    pub fnt: Vec<u8>,
    pub texture: Vec<u8>,
}

/// `name` goes into HEAD, the file stem plus size, e.g. "saira22".
pub fn build_font(name: &str, font: &RenderedFont) -> Result<BuiltFont> {
    ensure!(!font.glyphs.is_empty(), "a font needs at least one glyph");
    ensure!(
        name.len() <= 254,
        "the font name is {} bytes, more than 254",
        name.len()
    );
    ensure!(
        !name.as_bytes().contains(&0),
        "the font name contains a NUL byte"
    );

    let mut glyphs: Vec<&Glyph> = font.glyphs.iter().collect();
    glyphs.sort_by_key(|glyph| glyph.codepoint);
    for pair in glyphs.windows(2) {
        ensure!(
            pair[0].codepoint != pair[1].codepoint,
            "two glyphs share U+{:04X}",
            pair[0].codepoint
        );
    }
    for glyph in &glyphs {
        ensure!(
            glyph.codepoint >= 0x20,
            "U+{:04X} is below U+0020, which the game skips",
            glyph.codepoint
        );
        ensure!(
            glyph.coverage.len() == usize::from(glyph.width) * usize::from(glyph.height),
            "the U+{:04X} glyph has {} coverage bytes for a {}x{} cell",
            glyph.codepoint,
            glyph.coverage.len(),
            glyph.width,
            glyph.height
        );
        let right = right_bearing(glyph);
        ensure!(
            i16::try_from(right).is_ok(),
            "the U+{:04X} glyph's right bearing {right} does not fit 16 bits",
            glyph.codepoint
        );
    }
    let mut pairs: Vec<(u16, u16)> = font.kerning.iter().map(|&(l, r, _)| (l, r)).collect();
    pairs.sort_unstable();
    if let Some(pair) = pairs.windows(2).find(|pair| pair[0] == pair[1]) {
        bail!(
            "two kerning pairs for U+{:04X} U+{:04X}",
            pair[0].0,
            pair[0].1
        );
    }
    ensure!(
        glyphs.iter().any(|glyph| glyph.codepoint == 0x20),
        "the font has no U+0020 glyph"
    );

    let (ascent, cell_height) = cell_metrics(font, &glyphs);
    // A cell needs 1 free pixel above it and 2 below.
    ensure!(
        cell_height + 3 <= MAX_SIDE,
        "the cells are {cell_height} pixels tall, too tall for a {MAX_SIDE} pixel texture"
    );
    let widths: Vec<u16> = glyphs.iter().map(|glyph| glyph.width).collect();
    let (width, height, placements) = choose_atlas(&widths, cell_height)?;
    let cell_height = u16::try_from(cell_height)
        .expect("a chosen layout is taller than its cells and no taller than the limit");
    let atlas = render_atlas(&glyphs, &placements, ascent, width, height);
    Ok(BuiltFont {
        fnt: write_fnt(name, font, &glyphs, cell_height, &placements),
        texture: encode_edds(&mip_chain(atlas, edds_level_count(width, height)))?,
    })
}

fn cell_metrics(font: &RenderedFont, glyphs: &[&Glyph]) -> (i32, u32) {
    let ascent = glyphs
        .iter()
        .map(|glyph| i32::from(glyph.top))
        .max()
        .unwrap_or(0)
        .max(i32::from(font.ascent));
    let descent = glyphs
        .iter()
        .map(|glyph| i32::from(glyph.height) - i32::from(glyph.top))
        .max()
        .unwrap_or(0)
        .max(i32::from(font.descent));
    (ascent, (ascent + descent) as u32)
}

struct Placement {
    x: u32,
    y: u32,
}

fn choose_atlas(widths: &[u16], cell_height: u32) -> Result<(u32, u32, Vec<Placement>)> {
    let widest = widths.iter().copied().max().map(u32::from).unwrap_or(0);
    let first = (widest + 3).next_power_of_two();
    let mut best: Option<(u32, u32, Vec<Placement>)> = None;
    let mut width = first;
    while width <= MAX_SIDE {
        // FontToTga's textures are always square or twice as wide as tall.
        let (placements, used) = pack(widths, width, cell_height);
        let height = used.next_power_of_two().max(width / 2);
        let area = u64::from(width) * u64::from(height);
        let better = best.as_ref().is_none_or(|(best_width, best_height, _)| {
            area < u64::from(*best_width) * u64::from(*best_height)
        });
        if height <= width && better {
            best = Some((width, height, placements));
        }
        width *= 2;
    }
    match best {
        Some(layout) => Ok(layout),
        None if first > MAX_SIDE => bail!(
            "a glyph {widest} pixels wide needs a {first} pixel texture, past the {MAX_SIDE} pixel limit"
        ),
        None => {
            let (_, used) = pack(widths, MAX_SIDE, cell_height);
            let height = used.next_power_of_two();
            bail!(
                "the glyphs need a {MAX_SIDE}x{height} texture, past the {MAX_SIDE} pixel limit"
            )
        }
    }
}

fn pack(widths: &[u16], atlas_width: u32, cell_height: u32) -> (Vec<Placement>, u32) {
    let mut placements = Vec::with_capacity(widths.len());
    let mut x = 1;
    let mut y = 1;
    for &width in widths {
        let width = u32::from(width);
        if x + width + 2 > atlas_width {
            x = 1;
            y += cell_height + 1;
        }
        placements.push(Placement { x, y });
        x += width + 1;
    }
    (placements, y + cell_height + 2)
}

fn render_atlas(
    glyphs: &[&Glyph],
    placements: &[Placement],
    ascent: i32,
    width: u32,
    height: u32,
) -> RgbaImage {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[..3].fill(255);
    }
    for (glyph, placement) in glyphs.iter().zip(placements) {
        let offset = (ascent - i32::from(glyph.top)) as u32;
        for row in 0..u32::from(glyph.height) {
            for column in 0..u32::from(glyph.width) {
                let alpha = glyph.coverage[(row * u32::from(glyph.width) + column) as usize];
                let x = placement.x + column;
                let y = placement.y + offset + row;
                pixels[((y * width + x) * 4 + 3) as usize] = alpha;
            }
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn write_fnt(
    name: &str,
    font: &RenderedFont,
    glyphs: &[&Glyph],
    cell_height: u16,
    placements: &[Placement],
) -> Vec<u8> {
    let mut head = Vec::new();
    push_u32(&mut head, (name.len() + 1) as u32);
    head.extend_from_slice(name.as_bytes());
    head.push(0);
    push_u32(&mut head, font.size);
    push_u32(&mut head, 0);

    let codepoints: Vec<u16> = glyphs.iter().map(|glyph| glyph.codepoint).collect();
    let ranges = code_point_ranges(&codepoints);
    let mut glps = Vec::new();
    push_u32(&mut glps, 0);
    push_u32(&mut glps, 0);
    push_u32(&mut glps, glyphs.len() as u32);
    push_u32(&mut glps, ranges.len() as u32);
    for (first, count) in ranges {
        push_u16(&mut glps, first);
        push_u16(&mut glps, count);
    }

    let mut tcrd = Vec::new();
    for (glyph, placement) in glyphs.iter().zip(placements) {
        push_u16(&mut tcrd, placement.x as u16);
        push_u16(&mut tcrd, placement.y as u16);
        push_u16(&mut tcrd, glyph.width);
        push_u16(&mut tcrd, cell_height);
        push_i16(&mut tcrd, glyph.left);
        push_i16(&mut tcrd, right_bearing(glyph) as i16);
    }

    write_container(
        b"FNT3",
        [
            (b"HEAD", head),
            (b"GLPS", glps),
            (b"TCRD", tcrd),
            (b"KERN", write_kern(&font.kerning, &codepoints)),
        ],
    )
}

fn write_container<const N: usize>(version: &[u8; 4], chunks: [(&[u8; 4], Vec<u8>); N]) -> Vec<u8> {
    let mut fnt = Vec::new();
    fnt.extend_from_slice(b"FORM");
    fnt.extend_from_slice(&0u32.to_be_bytes());
    fnt.extend_from_slice(version);
    for (tag, payload) in chunks {
        fnt.extend_from_slice(tag);
        fnt.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        fnt.extend_from_slice(&payload);
    }
    let length = (fnt.len() - 8) as u32;
    fnt[4..8].copy_from_slice(&length.to_be_bytes());
    fnt
}

fn right_bearing(glyph: &Glyph) -> i32 {
    i32::from(glyph.advance) - i32::from(glyph.left) - i32::from(glyph.width)
}

fn write_kern(kerning: &[(u16, u16, i32)], codepoints: &[u16]) -> Vec<u8> {
    let in_font = |codepoint: u16| codepoints.binary_search(&codepoint).is_ok();
    let mut pairs: Vec<(u16, u16, i32)> = kerning
        .iter()
        .copied()
        .filter(|&(left, right, amount)| amount != 0 && in_font(left) && in_font(right))
        .collect();
    pairs.sort_unstable_by_key(|&(left, right, _)| (left, right));
    let mut kern = Vec::with_capacity(pairs.len() * 8);
    for (left, right, amount) in pairs {
        push_u16(&mut kern, left);
        push_u16(&mut kern, right);
        kern.extend_from_slice(&amount.to_le_bytes());
    }
    kern
}

fn code_point_ranges(codepoints: &[u16]) -> Vec<(u16, u16)> {
    let mut ranges: Vec<(u16, u16)> = Vec::new();
    for &codepoint in codepoints {
        match ranges.last_mut() {
            Some((first, count))
                if u32::from(*first) + u32::from(*count) == u32::from(codepoint) =>
            {
                *count += 1;
            }
            _ => ranges.push((codepoint, 1)),
        }
    }
    ranges
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_le_bytes());
}
