use std::sync::OnceLock;

use epaint_default_fonts::{EMOJI_ICON, HACK_REGULAR};
use modfoundry_font::{render_field_font, CharSet, FieldFont, FieldGlyph};

fn hack() -> &'static FieldFont {
    static FONT: OnceLock<FieldFont> = OnceLock::new();
    FONT.get_or_init(|| render_field_font(HACK_REGULAR, CharSet::Default).unwrap())
}

fn emoji() -> &'static FieldFont {
    static FONT: OnceLock<FieldFont> = OnceLock::new();
    FONT.get_or_init(|| render_field_font(EMOJI_ICON, CharSet::Default).unwrap())
}

fn glyph(font: &FieldFont, codepoint: u16) -> &FieldGlyph {
    font.glyphs
        .iter()
        .find(|glyph| glyph.codepoint == codepoint)
        .unwrap_or_else(|| panic!("no U+{codepoint:04X}"))
}

/// `codepoint`'s padded box at 32 pixels per em from Hack's own numbers, and its advance.
fn scaled(codepoint: char) -> ((i16, i16, u16, u16), i16) {
    let face = ttf_parser::Face::parse(HACK_REGULAR, 0).unwrap();
    let glyph_id = face.glyph_index(codepoint).unwrap();
    let scale = 32.0 / f32::from(face.units_per_em());
    let advance = face.glyph_hor_advance(glyph_id).unwrap();
    let advance = (f32::from(advance) * scale).round() as i16;
    let Some(bbox) = face.glyph_bounding_box(glyph_id) else {
        return ((0, 0, 0, 0), advance);
    };
    let (x_min, x_max) = (f32::from(bbox.x_min) * scale, f32::from(bbox.x_max) * scale);
    let (y_min, y_max) = (f32::from(bbox.y_min) * scale, f32::from(bbox.y_max) * scale);
    (
        (
            x_min.floor() as i16 - 1,
            y_max.ceil() as i16 + 1,
            (x_max.ceil() - x_min.floor()) as u16 + 2,
            (y_max.ceil() - y_min.floor()) as u16 + 2,
        ),
        advance,
    )
}

fn texel(font: &FieldFont, glyph: &FieldGlyph, x: u16, y: u16) -> [u8; 3] {
    glyph.field[usize::from(y) * usize::from(font.cell) + usize::from(x)]
}

fn median(mut pixel: [u8; 3]) -> u8 {
    pixel.sort_unstable();
    pixel[1]
}

#[test]
fn the_box_and_advance_follow_the_scaled_outline() {
    let font = hack();

    assert_eq!((font.em, font.range, font.cell), (32, 8, 44));
    for codepoint in ['A', 'I', 'g', 'l', ' '] {
        let (box_, advance) = scaled(codepoint);
        let glyph = glyph(font, codepoint as u16);
        assert_eq!(
            (
                glyph.left,
                glyph.top,
                glyph.width,
                glyph.height,
                glyph.advance
            ),
            (box_.0, box_.1, box_.2, box_.3, advance),
            "{codepoint}"
        );
    }
}

/// Where the median crosses 128 between two texel centres, in tile pixels.
fn crossing(medians: impl Iterator<Item = u8>) -> f32 {
    let medians: Vec<f32> = medians.map(f32::from).collect();
    let at = medians
        .windows(2)
        .position(|pair| pair[0] <= 128.0 && pair[1] > 128.0)
        .expect("an edge");
    at as f32 + 0.5 + (128.0 - medians[at]) / (medians[at + 1] - medians[at])
}

#[test]
fn the_outline_sits_one_pixel_inside_the_box_centred_in_its_tile() {
    let font = hack();
    let face = ttf_parser::Face::parse(HACK_REGULAR, 0).unwrap();
    let scale = 32.0 / f32::from(face.units_per_em());
    for codepoint in ['H', 'T', 'L'] {
        let glyph = glyph(font, codepoint as u16);
        let bbox = face
            .glyph_bounding_box(face.glyph_index(codepoint).unwrap())
            .unwrap();
        let (x_min, y_max) = (f32::from(bbox.x_min) * scale, f32::from(bbox.y_max) * scale);
        let tile_x = f32::from(font.cell - glyph.width) / 2.0;
        let tile_y = f32::from(font.cell - glyph.height) / 2.0;
        let left_edge = tile_x + 1.0 + (x_min - x_min.floor());
        let top_edge = tile_y + 1.0 + (y_max.ceil() - y_max);

        let row = (tile_y + f32::from(glyph.height) * 0.75) as u16;
        let column = (left_edge + 1.0) as u16;
        let across = crossing((0..font.cell).map(|x| median(texel(font, glyph, x, row))));
        let down = crossing((0..font.cell).map(|y| median(texel(font, glyph, column, y))));

        if codepoint != 'T' {
            assert!(
                (across - left_edge).abs() < 0.3,
                "{codepoint}: {across} vs {left_edge}"
            );
        }
        assert!(
            (down - top_edge).abs() < 0.3,
            "{codepoint}: {down} vs {top_edge}"
        );
        assert!(
            median(texel(font, glyph, 0, 0)) < 128,
            "{codepoint}: the tile corner"
        );
    }
}

#[test]
fn the_box_is_drawn_when_the_font_lacks_one() {
    let font = emoji();
    let box_ = glyph(font, 0x25A1);

    // 0.6 em square, 0.08 em stroke, 0.7 em advance: 19.2 px of side, 2.56 px of stroke.
    assert_eq!(
        (box_.left, box_.top, box_.width, box_.height, box_.advance),
        (-1, 21, 22, 22, 22)
    );
    let tile = f32::from(font.cell - box_.width) / 2.0;
    let inside =
        |x: f32, y: f32| median(texel(font, box_, (tile + x) as u16, (tile + y) as u16)) > 128;
    // The stroke runs from 1 to 3.56 pixels inside the box, past the 1 pixel margin.
    let width = f32::from(box_.width);
    assert!(inside(2.3, 6.0), "the left stroke");
    assert!(inside(width - 2.3, 6.0), "the right stroke");
    assert!(inside(6.0, 2.3), "the top stroke");
    assert!(!inside(6.0, 6.0), "the hole of the ring");
    assert!(median(texel(font, box_, 0, 0)) < 128, "a tile corner");
}

#[test]
fn the_same_font_bakes_the_same_field_twice() {
    let first = render_field_font(EMOJI_ICON, CharSet::Default).unwrap();
    let second = render_field_font(EMOJI_ICON, CharSet::Default).unwrap();

    assert_eq!(first.glyphs.len(), second.glyphs.len());
    for (a, b) in first.glyphs.iter().zip(&second.glyphs) {
        assert_eq!(a.codepoint, b.codepoint);
        assert_eq!(a.field, b.field, "U+{:04X}", a.codepoint);
    }
    assert_eq!(first.kerning, second.kerning);
    assert_eq!(first.cap_height, second.cap_height);
}

#[test]
fn a_file_that_is_not_a_font_is_refused() {
    let error = render_field_font(b"not a font", CharSet::Default).unwrap_err();

    assert_eq!(error.to_string(), "the file is not a font");
}
