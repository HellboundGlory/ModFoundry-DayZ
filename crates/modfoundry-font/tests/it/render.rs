use epaint_default_fonts::{EMOJI_ICON, HACK_REGULAR, NOTO_EMOJI_REGULAR, UBUNTU_LIGHT};
use modfoundry_font::{build_font, render_font, CharSet, Glyph, RenderedFont, Sizing};

fn glyph(font: &RenderedFont, codepoint: u16) -> &Glyph {
    font.glyphs
        .iter()
        .find(|glyph| glyph.codepoint == codepoint)
        .unwrap_or_else(|| panic!("no U+{codepoint:04X}"))
}

fn kerning(font: &RenderedFont, left: u16, right: u16) -> Option<i32> {
    font.kerning
        .iter()
        .find(|&&(l, r, _)| (l, r) == (left, right))
        .map(|&(_, _, amount)| amount)
}

#[test]
fn gpos_pair_kerning_scales_to_pixels() {
    let rendered = render_font(UBUNTU_LIGHT, 100, CharSet::Default, Sizing::Em).unwrap();

    // fontTools on Ubuntu Light (em 1000): A,V -50 units from one class pair, T,ï 31 from a
    // glyph pair plus a zero class pair, /,Ѵ 58 from a glyph pair plus a class pair; at 100 px
    // a tenth of the units arrives.
    assert_eq!(kerning(&rendered, 0x41, 0x56), Some(-5));
    assert_eq!(kerning(&rendered, 0x54, 0xEF), Some(3));
    assert_eq!(kerning(&rendered, 0x2F, 0x474), Some(6));
}

#[test]
fn gpos_kerning_rounds_before_dropping_zero_pairs() {
    let rendered = render_font(UBUNTU_LIGHT, 22, CharSet::Default, Sizing::Em).unwrap();

    // At 22 px T,o is -44 units, so -0.97 and -1 px, while ",Ѡ is -7 units and drops at zero.
    assert_eq!(kerning(&rendered, 0x54, 0x6F), Some(-1));
    assert_eq!(kerning(&rendered, 0x22, 0x4E0), None);
}

#[test]
fn a_pair_outside_the_gpos_coverage_stays_out() {
    let rendered = render_font(UBUNTU_LIGHT, 100, CharSet::Default, Sizing::Em).unwrap();

    // Space is in no coverage, yet its class pair with " holds -16 units.
    assert_eq!(kerning(&rendered, 0x48, 0x48), None);
    assert_eq!(kerning(&rendered, 0x20, 0x22), None);
}

#[test]
fn a_font_without_a_kerning_table_has_no_pairs() {
    let rendered = render_font(HACK_REGULAR, 64, CharSet::Default, Sizing::Em).unwrap();

    assert!(rendered.kerning.is_empty());
}

#[test]
fn a_gpos_table_without_a_kern_feature_keeps_the_legacy_pairs() {
    let face = ttf_parser::Face::parse(NOTO_EMOJI_REGULAR, 0).unwrap();
    let id = |c: char| face.glyph_index(c).unwrap().0;
    // Noto Emoji has GPOS without a kern feature; -32 units at 2048 per em and 64 px is -1 px.
    let font = with_kern(NOTO_EMOJI_REGULAR, &[(id('#'), id('0'), -32)]);

    let rendered = render_font(&font, 64, CharSet::Default, Sizing::Em).unwrap();

    assert_eq!(kerning(&rendered, 0x23, 0x30), Some(-1));
}

#[test]
fn a_font_with_both_kerning_tables_takes_the_gpos_pairs() {
    let face = ttf_parser::Face::parse(UBUNTU_LIGHT, 0).unwrap();
    let id = |c: char| face.glyph_index(c).unwrap().0;
    // A legacy +1000 units against the GPOS -50: only the GPOS value may count.
    let font = with_kern(UBUNTU_LIGHT, &[(id('A'), id('V'), 1000)]);

    let rendered = render_font(&font, 100, CharSet::Default, Sizing::Em).unwrap();

    assert_eq!(kerning(&rendered, 0x41, 0x56), Some(-5));
}

#[test]
fn the_default_set_holds_latin_and_cyrillic_and_no_control_code_points() {
    let rendered = render_font(HACK_REGULAR, 16, CharSet::Default, Sizing::Em).unwrap();

    let code_points: Vec<u16> = rendered
        .glyphs
        .iter()
        .map(|glyph| glyph.codepoint)
        .collect();
    assert!(code_points.contains(&0x41));
    assert!(code_points.contains(&0x416));
    assert!(code_points.iter().all(|&codepoint| codepoint >= 0x20));
    let space = glyph(&rendered, 0x20);
    assert_eq!((space.width, space.height), (0, 0));
    for glyph in &rendered.glyphs {
        assert_eq!(
            glyph.coverage.len(),
            usize::from(glyph.width) * usize::from(glyph.height),
            "U+{:04X}",
            glyph.codepoint
        );
    }
}

#[test]
fn the_advance_is_the_font_advance_scaled_to_the_size() {
    let rendered = render_font(HACK_REGULAR, 16, CharSet::Default, Sizing::Em).unwrap();

    let face = ttf_parser::Face::parse(HACK_REGULAR, 0).unwrap();
    let advance_units = face
        .glyph_hor_advance(face.glyph_index('A').unwrap())
        .unwrap();
    let expected = (f32::from(advance_units) * 16.0 / f32::from(face.units_per_em())).round();

    assert_eq!(f32::from(glyph(&rendered, 0x41).advance), expected);
}

#[test]
fn ascent_and_descent_come_from_os2_else_hhea() {
    // Hack OS/2 win 1901/483 per 2048; the emoji font has no OS/2, hhea 960/-64 per 1024.
    for (font, size, expected) in [(HACK_REGULAR, 22, (21, 6)), (EMOJI_ICON, 16, (15, 1))] {
        let rendered = render_font(font, size, CharSet::Default, Sizing::Em).unwrap();

        assert_eq!((rendered.ascent, rendered.descent), expected);
    }
}

#[test]
fn the_box_is_drawn_when_missing_and_glyphs_are_trimmed() {
    let hack = render_font(HACK_REGULAR, 16, CharSet::Default, Sizing::Em).unwrap();
    let covered = glyph(&hack, 0x25A1);
    assert_eq!(
        (covered.width, covered.height, covered.left, covered.top),
        (10, 11, 0, 9)
    );

    let rendered = render_font(UBUNTU_LIGHT, 22, CharSet::Default, Sizing::Em).unwrap();

    let boxed = glyph(&rendered, 0x25A1);
    assert_eq!((boxed.left, boxed.top, boxed.advance), (1, 14, 17));
    assert_eq!((boxed.width, boxed.height), (15, 15));
    let width = usize::from(boxed.width);
    for row in 0..usize::from(boxed.height) {
        for column in 0..width {
            let border = row == 0
                || column == 0
                || row == usize::from(boxed.height) - 1
                || column == width - 1;
            let expected = if border { 255 } else { 0 };
            assert_eq!(boxed.coverage[row * width + column], expected);
        }
    }

    for glyph in rendered.glyphs.iter().filter(|glyph| glyph.width > 0) {
        let width = usize::from(glyph.width);
        let height = usize::from(glyph.height);
        let ink = |x: usize, y: usize| glyph.coverage[y * width + x] != 0;
        assert!(
            (0..width).any(|x| ink(x, 0)),
            "U+{:04X} top",
            glyph.codepoint
        );
        assert!(
            (0..width).any(|x| ink(x, height - 1)),
            "U+{:04X} bottom",
            glyph.codepoint
        );
        assert!(
            (0..height).any(|y| ink(0, y)),
            "U+{:04X} left",
            glyph.codepoint
        );
        assert!(
            (0..height).any(|y| ink(width - 1, y)),
            "U+{:04X} right",
            glyph.codepoint
        );
    }
}

#[test]
fn rendered_fonts_build_fnt3_the_same_twice() {
    for font in [HACK_REGULAR, UBUNTU_LIGHT] {
        for size in [12, 28] {
            let rendered = render_font(font, size, CharSet::Default, Sizing::Em).unwrap();

            let first = build_font("test", &rendered).unwrap();
            let second = build_font(
                "test",
                &render_font(font, size, CharSet::Default, Sizing::Em).unwrap(),
            )
            .unwrap();

            assert_eq!(first.fnt, second.fnt, "{size}");
            assert_eq!(first.texture, second.texture, "{size}");
        }
    }
}

#[test]
fn render_font_refuses_what_it_cannot_render() {
    let cases: [(&[u8], u32, &str); 3] = [
        (b"not a font", 16, "the file is not a font"),
        (HACK_REGULAR, 0, "the size 0 is outside 1 to 512 pixels"),
        (HACK_REGULAR, 513, "the size 513 is outside 1 to 512 pixels"),
    ];

    for (font, size, message) in cases {
        let error = render_font(font, size, CharSet::Default, Sizing::Em).unwrap_err();

        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn the_all_set_takes_every_mapped_code_point_in_the_bmp() {
    let all = render_font(HACK_REGULAR, 16, CharSet::All, Sizing::Em).unwrap();
    let default = render_font(HACK_REGULAR, 16, CharSet::Default, Sizing::Em).unwrap();

    assert!(all.glyphs.iter().any(|glyph| glyph.codepoint == 0x192));
    assert!(all.glyphs.len() > default.glyphs.len());
    assert!(default.glyphs.iter().all(|glyph| glyph.codepoint != 0x192));
    assert!(all
        .glyphs
        .iter()
        .all(|glyph| glyph.codepoint >= 0x20 && !(0x7F..=0x9F).contains(&glyph.codepoint)));
}

#[test]
fn a_glyph_below_the_baseline_keeps_its_trimmed_box() {
    let rendered = render_font(HACK_REGULAR, 16, CharSet::Default, Sizing::Em).unwrap();

    let g = glyph(&rendered, 0x67);
    assert_eq!(
        (g.width, g.height, g.left, g.top, g.advance),
        (8, 12, 1, 9, 10)
    );
}

#[test]
fn a_set_too_big_for_one_texture_is_refused() {
    let Err(error) = render_font(HACK_REGULAR, 512, CharSet::All, Sizing::Em) else {
        panic!("a 512 pixel font of every glyph was accepted");
    };

    assert!(
        error.to_string().contains("more than a 4096x4096 texture"),
        "{error}"
    );
}

/// `font` with a format 0 `kern` table spliced in, holding `pairs` of glyph ids and font units.
fn with_kern(font: &[u8], pairs: &[(u16, u16, i16)]) -> Vec<u8> {
    let mut sorted = pairs.to_vec();
    sorted.sort_by_key(|&(left, right, _)| (left, right));
    let mut kern = Vec::new();
    for value in [
        0u16,
        1,
        0,
        (14 + 6 * sorted.len()) as u16,
        0x0001,
        sorted.len() as u16,
        0,
        0,
        0,
    ] {
        kern.extend_from_slice(&value.to_be_bytes());
    }
    for (left, right, value) in sorted {
        kern.extend_from_slice(&left.to_be_bytes());
        kern.extend_from_slice(&right.to_be_bytes());
        kern.extend_from_slice(&value.to_be_bytes());
    }

    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let mut records: Vec<[u8; 16]> = (0..tables)
        .map(|index| font[12 + 16 * index..28 + 16 * index].try_into().unwrap())
        .collect();
    for record in &mut records {
        let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) + 16;
        record[8..12].copy_from_slice(&offset.to_be_bytes());
    }
    let kern_offset = (font.len() + 16).next_multiple_of(4) as u32;
    let mut record = [0u8; 16];
    record[0..4].copy_from_slice(b"kern");
    record[8..12].copy_from_slice(&kern_offset.to_be_bytes());
    record[12..16].copy_from_slice(&(kern.len() as u32).to_be_bytes());
    records.push(record);
    records.sort_by_key(|record| <[u8; 4]>::try_from(&record[0..4]).unwrap());

    let mut out = font[..12].to_vec();
    out[4..6].copy_from_slice(&((tables + 1) as u16).to_be_bytes());
    records
        .iter()
        .for_each(|record| out.extend_from_slice(record));
    out.extend_from_slice(&font[12 + 16 * tables..]);
    out.resize(kern_offset as usize, 0);
    out.extend_from_slice(&kern);
    out
}

#[test]
fn legacy_kern_pairs_scale_to_pixels_and_drop_zero() {
    let face = ttf_parser::Face::parse(HACK_REGULAR, 0).unwrap();
    let id = |c: char| face.glyph_index(c).unwrap().0;
    // At 64 px Hack's 2048 units scale by 1/32.
    let font = with_kern(
        HACK_REGULAR,
        &[
            (id('A'), id('V'), -320),
            (id('V'), id('A'), -48),
            (id('T'), id('o'), 16),
            (id('L'), id('T'), 8),
        ],
    );

    let rendered = render_font(&font, 64, CharSet::Default, Sizing::Em).unwrap();

    let mut kerning = rendered.kerning.clone();
    kerning.sort();
    assert_eq!(
        kerning,
        vec![(0x41, 0x56, -10), (0x54, 0x6F, 1), (0x56, 0x41, -2)]
    );
}

#[test]
fn a_line_sized_bake_has_cells_as_tall_as_its_size() {
    for font in [HACK_REGULAR, UBUNTU_LIGHT] {
        for size in [8, 12, 13, 27] {
            let rendered = render_font(font, size, CharSet::All, Sizing::Line).unwrap();
            let fnt = build_font("test", &rendered).unwrap().fnt;

            let tcrd = fnt.windows(4).position(|tag| tag == b"TCRD").unwrap() + 8;
            let cell_height = u16::from_le_bytes([fnt[tcrd + 6], fnt[tcrd + 7]]);
            assert_eq!(u32::from(cell_height), size);
        }
    }
}

#[test]
fn a_line_sized_bake_shrinks_the_em_until_the_windows_ascent_and_descent_fill_the_size() {
    // Hack: OS/2 win 1901/483 per 2048, so the em at size 22 is 22 * 2048 / 2384 pixels.
    let rendered = render_font(HACK_REGULAR, 22, CharSet::Default, Sizing::Line).unwrap();

    let face = ttf_parser::Face::parse(HACK_REGULAR, 0).unwrap();
    let advance_units = face
        .glyph_hor_advance(face.glyph_index('A').unwrap())
        .unwrap();
    let expected = (f32::from(advance_units) * 22.0 / 2384.0).round();
    assert_eq!(f32::from(glyph(&rendered, 0x41).advance), expected);
    assert_eq!((rendered.ascent, rendered.descent), (18, 4));
}

fn with_windows_metrics(font: &[u8], ascent: u16, descent: u16) -> Vec<u8> {
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let record = (0..tables)
        .map(|index| 12 + 16 * index)
        .find(|&record| &font[record..record + 4] == b"OS/2")
        .unwrap();
    let os2 = u32::from_be_bytes(font[record + 8..record + 12].try_into().unwrap()) as usize;
    let mut patched = font.to_vec();
    patched[os2 + 74..os2 + 76].copy_from_slice(&ascent.to_be_bytes());
    patched[os2 + 76..os2 + 78].copy_from_slice(&descent.to_be_bytes());
    patched
}

#[test]
fn a_line_sized_bake_keeps_the_drawn_box_whole() {
    // Ubuntu Light is 1000 units per em; 400 + 110 leaves the em nearly twice the size.
    let font = with_windows_metrics(UBUNTU_LIGHT, 400, 110);
    let rendered = render_font(&font, 16, CharSet::Default, Sizing::Line).unwrap();

    let boxed = glyph(&rendered, 0x25A1);
    assert_eq!(boxed.width, boxed.height, "the box stays square");
    assert_eq!(
        boxed.width, 14,
        "capped to an ascent of 13 and one row below"
    );
    let width = usize::from(boxed.width);
    for row in 0..usize::from(boxed.height) {
        for column in 0..width {
            let border = row == 0
                || column == 0
                || row == usize::from(boxed.height) - 1
                || column == width - 1;
            let expected = if border { 255 } else { 0 };
            assert_eq!(boxed.coverage[row * width + column], expected, "row {row}");
        }
    }
    assert!(
        boxed.top - boxed.height as i16 >= -(rendered.descent as i16),
        "the box bottom {boxed:?} fits a descent of {}",
        rendered.descent
    );
    assert!(
        boxed.top <= rendered.ascent as i16,
        "the box top {} fits an ascent of {}",
        boxed.top,
        rendered.ascent
    );

    let fnt = build_font("test", &rendered).unwrap().fnt;
    let tcrd = fnt.windows(4).position(|tag| tag == b"TCRD").unwrap() + 8;
    assert_eq!(u16::from_le_bytes([fnt[tcrd + 6], fnt[tcrd + 7]]), 16);
}

#[test]
fn a_line_sized_bake_with_no_descent_sits_the_box_on_the_baseline() {
    let font = with_windows_metrics(UBUNTU_LIGHT, 800, 0);
    let rendered = render_font(&font, 16, CharSet::Default, Sizing::Line).unwrap();

    let boxed = glyph(&rendered, 0x25A1);
    assert_eq!(boxed.top, boxed.height as i16, "{boxed:?}");
    let width = usize::from(boxed.width);
    let bottom = &boxed.coverage[boxed.coverage.len() - width..];
    assert!(
        bottom.iter().all(|&c| c == 255),
        "the bottom border is kept"
    );
}

#[test]
fn line_sizing_refuses_windows_metrics_far_under_the_em_and_em_sizing_ignores_them() {
    let tiny = with_windows_metrics(HACK_REGULAR, 1, 0);
    let error = render_font(&tiny, 40, CharSet::Default, Sizing::Line).unwrap_err();
    assert!(error.to_string().contains("--sizing em"), "{error}");

    let zero = with_windows_metrics(HACK_REGULAR, 0, 0);
    assert!(render_font(&zero, 12, CharSet::Default, Sizing::Em).is_ok());
}

#[test]
fn a_line_sized_bake_cuts_ink_at_the_cell_edges_and_no_further() {
    // Hack's capitals reach above 1400 units and its descenders below 200.
    let font = with_windows_metrics(HACK_REGULAR, 1400, 200);
    let rendered = render_font(&font, 16, CharSet::Default, Sizing::Line).unwrap();

    let highest = rendered.glyphs.iter().map(|glyph| glyph.top).max().unwrap();
    let lowest = rendered
        .glyphs
        .iter()
        .map(|glyph| glyph.height as i16 - glyph.top)
        .max()
        .unwrap();
    assert_eq!(
        (highest, lowest),
        (rendered.ascent as i16, rendered.descent as i16)
    );
}
