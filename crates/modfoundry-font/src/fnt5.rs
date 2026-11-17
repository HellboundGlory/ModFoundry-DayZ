use anyhow::{bail, ensure, Result};
use modfoundry_texture::edds::encode_edds;
use modfoundry_texture::RgbaImage;

use crate::{
    code_point_ranges, push_i16, push_u16, push_u32, write_container, write_kern, BuiltFont,
    FieldFont, FieldGlyph, MAX_SIDE,
};

/// `name` goes into HEAD, the file stem plus size, e.g. "saira32".
pub fn build_field_font(name: &str, font: &FieldFont) -> Result<BuiltFont> {
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
    ensure!(font.em > 0, "an em of 0 pixels draws nothing");
    ensure!(
        i16::try_from(font.range).is_ok(),
        "a range of {} pixels does not fit 16 bits",
        font.range
    );

    let mut glyphs: Vec<&FieldGlyph> = font.glyphs.iter().collect();
    glyphs.sort_by_key(|glyph| glyph.codepoint);
    for pair in glyphs.windows(2) {
        ensure!(
            pair[0].codepoint != pair[1].codepoint,
            "two glyphs share U+{:04X}",
            pair[0].codepoint
        );
    }
    let cell = usize::from(font.cell);
    for glyph in &glyphs {
        ensure!(
            glyph.codepoint >= 0x20,
            "U+{:04X} is below U+0020, which the game skips",
            glyph.codepoint
        );
        ensure!(
            glyph.width.max(glyph.height) <= font.cell,
            "the U+{:04X} glyph is {}x{} pixels, larger than its {} pixel cell",
            glyph.codepoint,
            glyph.width,
            glyph.height,
            font.cell
        );
        ensure!(
            glyph.field.len() == cell * cell,
            "the U+{:04X} glyph has {} field texels for a {cell}x{cell} cell",
            glyph.codepoint,
            glyph.field.len()
        );
    }
    for codepoint in [0x20, 0x25A1] {
        ensure!(
            glyphs.iter().any(|glyph| glyph.codepoint == codepoint),
            "the font has no U+{codepoint:04X} glyph"
        );
    }

    let (width, height) = choose_atlas(glyphs.len(), font.cell)?;
    let tiles = tile_positions(glyphs.len(), font.cell, width);
    Ok(BuiltFont {
        fnt: write_fnt5(name, font, &glyphs, &tiles),
        texture: encode_edds(&[render_atlas(&glyphs, font.cell, width, height, &tiles)])?,
    })
}

fn choose_atlas(count: usize, cell: u16) -> Result<(u32, u32)> {
    let step = u32::from(cell) + 2;
    let mut height = 1;
    while height <= MAX_SIDE {
        if tiles_per_side(height, step).pow(2) >= count {
            return Ok((height, height));
        }
        if height * 2 <= MAX_SIDE
            && tiles_per_side(height * 2, step) * tiles_per_side(height, step) >= count
        {
            return Ok((height * 2, height));
        }
        height *= 2;
    }
    bail!("{count} glyphs in {cell} pixel cells do not fit a {MAX_SIDE} pixel texture")
}

fn tiles_per_side(side: u32, step: u32) -> usize {
    (side.saturating_add(1) / step) as usize
}

fn tile_positions(count: usize, cell: u16, width: u32) -> Vec<(u32, u32)> {
    let step = u32::from(cell) + 2;
    let columns = tiles_per_side(width, step) as u32;
    (0..count as u32)
        .map(|index| (1 + index % columns * step, 1 + index / columns * step))
        .collect()
}

fn render_atlas(
    glyphs: &[&FieldGlyph],
    cell: u16,
    width: u32,
    height: u32,
    tiles: &[(u32, u32)],
) -> RgbaImage {
    let cell = u32::from(cell);
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    for (glyph, &(x, y)) in glyphs.iter().zip(tiles) {
        for row in 0..cell {
            for column in 0..cell {
                let texel = glyph.field[(row * cell + column) as usize];
                let at = (((y + row) * width + x + column) * 4) as usize;
                pixels[at..at + 3].copy_from_slice(&texel);
            }
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn write_fnt5(
    name: &str,
    font: &FieldFont,
    glyphs: &[&FieldGlyph],
    tiles: &[(u32, u32)],
) -> Vec<u8> {
    let mut head = Vec::new();
    push_u32(&mut head, (name.len() + 1) as u32);
    head.extend_from_slice(name.as_bytes());
    head.push(0);
    push_u32(&mut head, font.em);
    push_u32(&mut head, 0);
    head.push(2);
    push_u32(&mut head, u32::from(font.cell));
    head.extend_from_slice(&font.cap_height.to_le_bytes());
    head.extend_from_slice(&(font.em as f32).to_le_bytes());
    push_i16(&mut head, font.range as i16);
    head.push(0);
    head.push(0);
    head.extend_from_slice(&(font.em as f32).to_le_bytes());

    let codepoints: Vec<u16> = glyphs.iter().map(|glyph| glyph.codepoint).collect();
    let ranges = code_point_ranges(&codepoints);
    let mut glps = vec![0; 8];
    push_u32(&mut glps, glyphs.len() as u32);
    push_u32(&mut glps, ranges.len() as u32);
    for (first, count) in ranges {
        push_u32(&mut glps, u32::from(first));
        push_u16(&mut glps, count);
        push_u16(&mut glps, 0);
    }

    let mut tcrd = Vec::with_capacity(glyphs.len() * 14);
    for (glyph, &(x, y)) in glyphs.iter().zip(tiles) {
        push_u16(&mut tcrd, x as u16);
        push_u16(&mut tcrd, y as u16);
        push_u16(&mut tcrd, glyph.width);
        push_u16(&mut tcrd, glyph.height);
        push_i16(&mut tcrd, glyph.left);
        push_i16(&mut tcrd, glyph.top);
        push_i16(&mut tcrd, glyph.advance);
    }

    write_container(
        b"FNT5",
        [
            (b"HEAD", head),
            (b"GLPS", glps),
            (b"TCRD", tcrd),
            (b"KERN", write_kern(&font.kerning, &codepoints)),
        ],
    )
}
