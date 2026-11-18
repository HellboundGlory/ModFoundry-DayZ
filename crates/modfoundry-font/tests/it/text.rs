use epaint_default_fonts::HACK_REGULAR;
use modfoundry_font::{
    build_field_font, draw_text, layout_text, parse_fnt, render_field_font, CharSet, FntFont,
    FntGlyph, FntVersion, HAlign, Kerning, SdfInfo, TextOptions,
};
use modfoundry_texture::edds::decode_edds;
use modfoundry_texture::RgbaImage;

fn sdf_font(glyphs: Vec<FntGlyph>, kerning: Vec<Kerning>) -> FntFont {
    FntFont {
        version: FntVersion::Fnt5,
        name: "test".into(),
        size: 10,
        sdf: Some(SdfInfo {
            multi_channel: true,
            cell: 8,
            cap_height: 6.0,
            em: 10.0,
            range: 2,
            bold: false,
            italic: false,
        }),
        glyphs,
        kerning,
    }
}

fn bitmap_font(glyphs: Vec<FntGlyph>, kerning: Vec<Kerning>) -> FntFont {
    FntFont {
        version: FntVersion::Fnt3,
        name: "test".into(),
        size: 10,
        sdf: None,
        glyphs,
        kerning,
    }
}

fn glyph(
    codepoint: u32,
    rect: [u16; 4],
    bearing_x: i16,
    bearing_y: Option<i16>,
    advance: i16,
) -> FntGlyph {
    let [x, y, w, h] = rect;
    FntGlyph {
        codepoint,
        x,
        y,
        w,
        h,
        bearing_x,
        bearing_y,
        advance,
    }
}

fn options(em_px: f32) -> TextOptions {
    TextOptions {
        em_px,
        wrap_width: None,
        halign: HAlign::Left,
    }
}

#[test]
fn an_sdf_glyph_is_placed_at_its_scaled_box() {
    let font = sdf_font(vec![glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6)], vec![]);
    for (em_px, want) in [(20.0, (2.0, 6.0, 8.0, 12.0)), (10.0, (1.0, 3.0, 4.0, 6.0))] {
        let block = layout_text(&font, "A", &options(em_px));
        let placed = &block.glyphs[0];
        assert_eq!(placed.codepoint, 0x41);
        assert_eq!(
            (placed.x, placed.y, placed.w, placed.h),
            want,
            "em_px {em_px}"
        );
        assert_eq!(placed.src, [3.0, 2.0, 4.0, 6.0], "em_px {em_px}");
    }
}

#[test]
fn a_bitmap_glyph_is_placed_at_its_scaled_cell() {
    let font = bitmap_font(vec![glyph(0x41, [0, 0, 4, 10], 1, None, 6)], vec![]);
    for (em_px, want) in [(20.0, (2.0, 0.0, 8.0, 20.0)), (10.0, (1.0, 0.0, 4.0, 10.0))] {
        let block = layout_text(&font, "A", &options(em_px));
        let placed = &block.glyphs[0];
        assert_eq!(
            (placed.x, placed.y, placed.w, placed.h),
            want,
            "em_px {em_px}"
        );
        assert_eq!(placed.src, [0.0, 0.0, 4.0, 10.0], "em_px {em_px}");
    }
}

#[test]
fn a_newline_starts_the_next_line_one_em_lower() {
    let box_a = [1, 1, 4, 6];
    let font = sdf_font(
        vec![
            glyph(0x41, box_a, 1, Some(5), 6),
            glyph(0x42, box_a, 1, Some(5), 6),
        ],
        vec![],
    );
    let block = layout_text(&font, "A\nB", &options(20.0));
    assert_eq!(block.lines, 2);
    assert_eq!(block.height, 40.0);
    let second = &block.glyphs[1];
    assert_eq!((second.x, second.y), (2.0, 26.0));
}

#[test]
fn kerning_moves_the_second_glyph_of_a_pair() {
    let box_a = [1, 1, 4, 6];
    let font = sdf_font(
        vec![
            glyph(0x41, box_a, 1, Some(5), 6),
            glyph(0x42, box_a, 1, Some(5), 6),
        ],
        vec![Kerning {
            left: 0x41,
            right: 0x42,
            amount: -2,
        }],
    );
    let block = layout_text(&font, "AB", &options(20.0));
    assert_eq!(block.glyphs[1].x, 2.0 + 12.0 - 4.0);
}

#[test]
fn a_missing_glyph_draws_the_box_fallback() {
    let box_glyph = glyph(0x25A1, [1, 1, 4, 6], 1, Some(5), 3);
    let a = glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6);
    let font = sdf_font(vec![box_glyph, a], vec![]);
    let block = layout_text(&font, "xA", &options(20.0));
    assert_eq!(block.glyphs[0].codepoint, 0x25A1);
    assert_eq!(block.glyphs[1].x, 6.0 + 2.0);
}

#[test]
fn a_font_without_a_fallback_skips_a_missing_glyph() {
    let a = glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6);
    let font = sdf_font(vec![a], vec![]);
    let block = layout_text(&font, "xA", &options(20.0));
    assert_eq!(block.glyphs.len(), 1);
    assert_eq!(block.glyphs[0].x, 2.0);
}

#[test]
fn an_empty_text_gives_an_empty_block() {
    let font = sdf_font(vec![glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6)], vec![]);
    let block = layout_text(&font, "", &options(20.0));
    assert_eq!(
        (block.glyphs.len(), block.width, block.height, block.lines),
        (0, 0.0, 0.0, 0)
    );
}

#[test]
fn carriage_returns_are_ignored() {
    let space = glyph(0x20, [0, 0, 0, 0], 0, None, 2);
    let a = glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6);
    let font = sdf_font(vec![space, a], vec![]);
    let block = layout_text(&font, "A\r", &options(20.0));
    assert_eq!((block.glyphs.len(), block.lines), (1, 1));
}

#[test]
fn a_missing_space_advances_the_pen_without_a_quad() {
    let box_a = [1, 1, 4, 6];
    let font = sdf_font(
        vec![
            glyph(0x41, box_a, 1, Some(5), 6),
            glyph(0x42, box_a, 1, Some(5), 6),
        ],
        vec![],
    );
    let block = layout_text(&font, "A B", &options(20.0));
    assert_eq!(block.glyphs.len(), 2);
    assert_eq!(block.glyphs[1].x, (6.0 + 2.5) * 2.0 + 2.0);
    assert_eq!(block.width, (6.0 + 2.5 + 6.0) * 2.0);
}

#[test]
fn a_trailing_newline_adds_no_line() {
    let font = sdf_font(vec![glyph(0x41, [1, 1, 4, 6], 1, Some(5), 6)], vec![]);
    for (text, lines) in [("A\n", 1), ("A\r\n", 1), ("A\n\nB", 3), ("\n", 0)] {
        let block = layout_text(&font, text, &options(20.0));
        assert_eq!(block.lines, lines, "{text:?}");
    }
}

#[test]
fn a_font_with_a_space_glyph_keeps_the_space_advance() {
    let box_a = [1, 1, 4, 6];
    let space = glyph(0x20, [0, 0, 0, 0], 0, None, 2);
    let font = sdf_font(
        vec![
            space,
            glyph(0x41, box_a, 1, Some(5), 6),
            glyph(0x42, box_a, 1, Some(5), 6),
        ],
        vec![],
    );
    let block = layout_text(&font, "A B", &options(20.0));
    assert_eq!(block.glyphs.len(), 3);
    assert_eq!(block.glyphs[2].x, (6.0 + 2.0) * 2.0 + 2.0);
}

fn bitmap_word_font() -> FntFont {
    bitmap_font(
        vec![
            glyph(0x61, [0, 0, 4, 10], 0, None, 4),
            glyph(0x62, [0, 0, 4, 10], 0, None, 4),
            glyph(0x63, [0, 0, 4, 10], 0, None, 4),
            glyph(0x20, [0, 0, 0, 0], 0, None, 2),
        ],
        vec![],
    )
}

fn wrapped(wrap_width: f32, halign: HAlign) -> TextOptions {
    TextOptions {
        em_px: 10.0,
        wrap_width: Some(wrap_width),
        halign,
    }
}

#[test]
fn a_wrapped_line_breaks_at_a_space() {
    let block = layout_text(&bitmap_word_font(), "aa bb cc", &wrapped(9.0, HAlign::Left));
    assert_eq!(block.lines, 3);
    assert_eq!(block.glyphs.len(), 6);
    assert_eq!((block.glyphs[2].x, block.glyphs[2].y), (0.0, 10.0));
    assert_eq!((block.glyphs[4].x, block.glyphs[4].y), (0.0, 20.0));
}

#[test]
fn a_word_wider_than_the_wrap_width_stays_whole() {
    let font = bitmap_word_font();
    let block = layout_text(&font, "ab cccccccc", &wrapped(9.0, HAlign::Left));
    assert_eq!(block.lines, 2);
    assert_eq!(block.glyphs.len(), 10);
    let block = layout_text(&font, "cccccccc", &wrapped(9.0, HAlign::Left));
    assert_eq!(block.lines, 1);
}

#[test]
fn a_dropped_break_space_does_not_count_toward_the_line_width() {
    let block = layout_text(&bitmap_word_font(), "aa bb", &wrapped(9.0, HAlign::Right));
    assert_eq!(block.glyphs[0].x, 1.0);
    assert_eq!(block.glyphs[2].x, 1.0);
}

#[test]
fn each_line_aligns_inside_the_widest_line() {
    let font = bitmap_word_font();
    for (halign, first_x) in [
        (HAlign::Left, 0.0),
        (HAlign::Center, 4.0),
        (HAlign::Right, 8.0),
    ] {
        let options = TextOptions {
            em_px: 10.0,
            wrap_width: None,
            halign,
        };
        let block = layout_text(&font, "a\nbbb", &options);
        assert_eq!(block.width, 12.0);
        assert_eq!(block.glyphs[0].x, first_x, "{halign:?}");
        assert_eq!(block.glyphs[1].x, 0.0, "{halign:?}");
    }
}

#[test]
fn alignment_uses_the_wrap_width_as_the_box_when_set() {
    let block = layout_text(
        &bitmap_word_font(),
        "a\nbbb",
        &wrapped(20.0, HAlign::Center),
    );
    assert_eq!(block.glyphs[0].x, 8.0);
    assert_eq!(block.glyphs[1].x, 4.0);
}

fn atlas(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
    let pixels = (0..height)
        .flat_map(|y| (0..width).flat_map(|x| texel(x, y)).collect::<Vec<u8>>())
        .collect();
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn blank(width: u32, height: u32) -> RgbaImage {
    RgbaImage {
        width,
        height,
        pixels: vec![0; (width * height * 4) as usize],
    }
}

fn alpha_at(image: &RgbaImage, x: u32, y: u32) -> u8 {
    image.pixels[((y * image.width + x) * 4 + 3) as usize]
}

fn pixel_at(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.pixels[at..at + 4].try_into().unwrap()
}

#[test]
fn a_bitmap_glyph_at_unit_scale_copies_the_atlas_alpha() {
    let font = bitmap_font(vec![glyph(0x41, [0, 0, 2, 2], 0, None, 2)], vec![]);
    let texels = [10, 200, 50, 255];
    let atlas = atlas(2, 2, |x, y| [0, 0, 0, texels[(y * 2 + x) as usize]]);
    let block = layout_text(&font, "A", &options(2.0));
    let mut target = blank(4, 4);
    draw_text(
        &mut target,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        None,
    );
    assert_eq!(alpha_at(&target, 0, 0), 10);
    assert_eq!(alpha_at(&target, 1, 0), 200);
    assert_eq!(alpha_at(&target, 0, 1), 50);
    assert_eq!(alpha_at(&target, 1, 1), 255);
    assert_eq!(alpha_at(&target, 2, 0), 0);
    assert_eq!(alpha_at(&target, 0, 2), 0);
    assert_eq!(pixel_at(&target, 1, 0), [255, 255, 255, 200]);
}

#[test]
fn colour_alpha_multiplies_the_glyph_alpha() {
    let font = bitmap_font(vec![glyph(0x41, [0, 0, 2, 2], 0, None, 2)], vec![]);
    let atlas = atlas(2, 2, |_, _| [0, 0, 0, 200]);
    let block = layout_text(&font, "A", &options(2.0));
    let mut target = blank(2, 2);
    draw_text(
        &mut target,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0, 1.0, 1.0, 0.5],
        None,
    );
    assert_eq!(alpha_at(&target, 0, 0), 100);
}

#[test]
fn the_clip_and_the_target_edge_cut_a_glyph() {
    let font = bitmap_font(vec![glyph(0x41, [0, 0, 2, 2], 0, None, 2)], vec![]);
    let atlas = atlas(2, 2, |_, _| [0, 0, 0, 255]);
    let block = layout_text(&font, "A", &options(2.0));

    let mut clipped = blank(4, 2);
    draw_text(
        &mut clipped,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        Some([1, 0, 2, 2]),
    );
    assert_eq!(alpha_at(&clipped, 0, 0), 0);
    assert_eq!(alpha_at(&clipped, 1, 0), 255);
    assert_eq!(alpha_at(&clipped, 2, 0), 0);

    let mut edged = blank(4, 2);
    draw_text(
        &mut edged,
        &atlas,
        &font,
        &block,
        (-1.0, 0.0),
        [1.0; 4],
        None,
    );
    assert_eq!(alpha_at(&edged, 0, 0), 255);
    assert_eq!(alpha_at(&edged, 1, 0), 0);
}

#[test]
fn a_glyph_at_a_fractional_scale_does_not_read_its_neighbour() {
    let font = bitmap_font(
        vec![
            glyph(0x41, [0, 0, 2, 2], 0, None, 2),
            glyph(0x42, [2, 0, 2, 2], 0, None, 2),
        ],
        vec![],
    );
    let atlas = atlas(4, 2, |x, _| [0, 0, 0, if x < 2 { 255 } else { 0 }]);
    let block = layout_text(&font, "A", &options(3.0));
    let mut target = blank(4, 4);
    draw_text(
        &mut target,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        None,
    );
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(alpha_at(&target, x, y), 255, "({x}, {y})");
        }
    }
    assert_eq!(alpha_at(&target, 3, 0), 0);
}

#[test]
fn an_sdf_glyph_scaled_up_fades_in_from_its_own_padding() {
    let font = sdf_font(vec![glyph(0x41, [0, 0, 4, 4], 0, Some(4), 4)], vec![]);
    let atlas = atlas(8, 8, |x, y| {
        let inside = (2..6).contains(&x) && (2..6).contains(&y);
        let value = if inside { 128 } else { 0 };
        [value, value, value, 255]
    });
    let block = layout_text(&font, "A", &options(20.0));
    let mut target = blank(20, 20);
    draw_text(
        &mut target,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        None,
    );
    assert_eq!(alpha_at(&target, 0, 10), 0);
    assert!((i32::from(alpha_at(&target, 1, 10)) - 130).abs() <= 2);
}

#[test]
fn a_large_em_softens_the_field_gain_with_its_third_of_em_plus_range() {
    let mut font = sdf_font(vec![glyph(0x41, [0, 0, 8, 8], 0, Some(8), 8)], vec![]);
    let sdf = font.sdf.as_mut().unwrap();
    sdf.em = 50.0;
    sdf.range = 8;
    let flat = atlas(8, 8, |_, _| [130, 130, 130, 255]);
    let block = layout_text(&font, "A", &options(50.0));
    let mut target = blank(8, 40);
    draw_text(
        &mut target,
        &flat,
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        None,
    );
    // k = (50 + 8) / 3, gain 1.0607 * k at scale 1: 0.5 + (130/255 - 0.5) * gain
    assert!((i32::from(alpha_at(&target, 4, 24)) - 179).abs() <= 2);
}

#[test]
fn text_blends_over_what_is_already_drawn() {
    let font = bitmap_font(vec![glyph(0x41, [0, 0, 2, 2], 0, None, 2)], vec![]);
    let atlas = atlas(2, 2, |_, _| [0, 0, 0, 128]);
    let block = layout_text(&font, "A", &options(2.0));
    let mut target = blank(2, 2);
    target
        .pixels
        .chunks_mut(4)
        .for_each(|p| p.copy_from_slice(&[0, 0, 255, 255]));
    draw_text(
        &mut target,
        &atlas,
        &font,
        &block,
        (0.0, 0.0),
        [1.0, 0.0, 0.0, 1.0],
        None,
    );
    let pixel = &target.pixels[0..4];
    assert_eq!(pixel[3], 255);
    assert!((i32::from(pixel[0]) - 128).abs() <= 1, "{pixel:?}");
    assert!((i32::from(pixel[2]) - 127).abs() <= 1, "{pixel:?}");
}

fn straight_edge_atlas() -> RgbaImage {
    let columns = [0u8, 0, 0, 0, 127, 128, 255, 255];
    atlas(8, 8, |x, _| {
        let value = columns[x as usize];
        [value, value, value, 255]
    })
}

#[test]
fn an_sdf_edge_is_half_alpha_and_ramps_monotonically() {
    let font = sdf_font(vec![glyph(0x41, [0, 0, 8, 8], 0, Some(8), 8)], vec![]);
    let block = layout_text(&font, "A", &options(10.0));
    let mut target = blank(8, 8);
    draw_text(
        &mut target,
        &straight_edge_atlas(),
        &font,
        &block,
        (0.0, 0.0),
        [1.0; 4],
        None,
    );
    let row: Vec<u8> = (0..8).map(|x| alpha_at(&target, x, 0)).collect();
    assert_eq!(row[0], 0);
    assert_eq!(row[7], 255);
    assert!(row.windows(2).all(|pair| pair[0] <= pair[1]), "{row:?}");
    for x in [4, 5] {
        assert!((f32::from(row[x]) / 255.0 - 0.5).abs() < 0.01, "{row:?}");
    }
}

#[test]
fn the_sdf_ramp_steepens_as_the_scale_grows() {
    let font = sdf_font(vec![glyph(0x41, [0, 0, 8, 8], 0, Some(8), 8)], vec![]);
    let flat = atlas(8, 8, |_, _| [153, 153, 153, 255]);
    for (em_px, want) in [(10.0, 182), (20.0, 236)] {
        let block = layout_text(&font, "A", &options(em_px));
        let mut target = blank(16, 16);
        draw_text(
            &mut target,
            &flat,
            &font,
            &block,
            (0.0, 0.0),
            [1.0; 4],
            None,
        );
        assert_eq!(alpha_at(&target, 0, 0), want, "em_px {em_px}");
    }
}

#[test]
fn a_built_font_draws_text_inside_its_box() {
    let field = render_field_font(HACK_REGULAR, CharSet::Default).unwrap();
    let built = build_field_font("hack32", &field).unwrap();
    let parsed = parse_fnt(&built.fnt).unwrap();
    let atlas = decode_edds(&built.texture).unwrap().remove(0);
    let em = parsed.sdf.as_ref().unwrap().em;
    let block = layout_text(&parsed, "Hi", &options(em));
    let margin = 4;
    let (box_width, box_height) = (block.width.ceil() as u32, block.height.ceil() as u32);
    let mut target = blank(box_width + 2 * margin, box_height + 2 * margin);
    let origin = (margin as f32, margin as f32);
    draw_text(&mut target, &atlas, &parsed, &block, origin, [1.0; 4], None);
    let mut painted_inside = false;
    for y in 0..target.height {
        for x in 0..target.width {
            let painted = alpha_at(&target, x, y) > 0;
            let within = (margin..margin + box_width).contains(&x)
                && (margin..margin + box_height).contains(&y);
            if painted && within {
                painted_inside = true;
            }
            assert!(!painted || within, "paint outside the box at ({x}, {y})");
        }
    }
    assert!(painted_inside);
}
