use modfoundry_font::{build_field_font, FieldFont, FieldGlyph};
use modfoundry_texture::edds::decode_edds;

fn space() -> FieldGlyph {
    FieldGlyph {
        codepoint: 0x20,
        width: 0,
        height: 0,
        left: 0,
        top: 0,
        advance: 4,
        field: vec![[0, 0, 0]; 4],
    }
}

fn missing_glyph() -> FieldGlyph {
    FieldGlyph {
        codepoint: 0x25A1,
        width: 2,
        height: 2,
        left: 0,
        top: 2,
        advance: 3,
        field: vec![[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]],
    }
}

fn font(glyphs: Vec<FieldGlyph>) -> FieldFont {
    FieldFont {
        em: 32,
        range: 8,
        cap_height: 20.0,
        cell: 2,
        glyphs,
        kerning: Vec::new(),
    }
}

fn glyph(codepoint: u16, field: [u8; 4]) -> FieldGlyph {
    FieldGlyph {
        codepoint,
        width: 2,
        height: 2,
        left: 0,
        top: 2,
        advance: 3,
        field: field.map(|value| [value, value + 1, value + 2]).to_vec(),
    }
}

fn chunk<'a>(fnt: &'a [u8], tag: &[u8; 4]) -> &'a [u8] {
    let mut position = 12;
    while position < fnt.len() {
        let chunk_tag: &[u8; 4] = fnt[position..position + 4].try_into().unwrap();
        let length =
            u32::from_be_bytes(fnt[position + 4..position + 8].try_into().unwrap()) as usize;
        if chunk_tag == tag {
            return &fnt[position + 8..position + 8 + length];
        }
        position += 8 + length;
    }
    panic!("no {:?} chunk", String::from_utf8_lossy(tag));
}

#[test]
fn a_minimal_font_writes_the_expected_fnt_bytes() {
    let rendered = FieldFont {
        kerning: vec![(0x20, 0x25A1, -2)],
        ..font(vec![space(), missing_glyph()])
    };

    let built = build_field_font("saira32", &rendered).unwrap();

    let expected: &[u8] = &[
        // FORM, file length - 8, FNT5
        b'F', b'O', b'R', b'M', 0, 0, 0, 145, b'F', b'N', b'T', b'5', //
        // HEAD, 41 bytes: name length with NUL, "saira32", NUL, em, zero, MSDF, cell,
        // cap height, em, range, bold, italic, em
        b'H', b'E', b'A', b'D', 0, 0, 0, 41, //
        8, 0, 0, 0, b's', b'a', b'i', b'r', b'a', b'3', b'2', 0, //
        32, 0, 0, 0, //
        0, 0, 0, 0, //
        2, //
        2, 0, 0, 0, //
        0, 0, 160, 65, //
        0, 0, 0, 66, //
        8, 0, //
        0, //
        0, //
        0, 0, 0, 66, //
        // GLPS, 32 bytes: two unused words, two glyphs in two ranges
        b'G', b'L', b'P', b'S', 0, 0, 0, 32, //
        0, 0, 0, 0, //
        0, 0, 0, 0, //
        2, 0, 0, 0, //
        2, 0, 0, 0, //
        0x20, 0, 0, 0, 1, 0, 0, 0, //
        0xA1, 0x25, 0, 0, 1, 0, 0, 0, //
        // TCRD, 28 bytes: space in the tile at (1,1), the box in the tile at (5,1)
        b'T', b'C', b'R', b'D', 0, 0, 0, 28, //
        1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, //
        5, 0, 1, 0, 2, 0, 2, 0, 0, 0, 2, 0, 3, 0, //
        // KERN, 8 bytes: the pair between the two glyphs
        b'K', b'E', b'R', b'N', 0, 0, 0, 8, //
        0x20, 0, 0xA1, 0x25, 0xFE, 0xFF, 0xFF, 0xFF,
    ];

    assert_eq!(built.fnt.as_slice(), expected);
}

#[test]
fn code_points_out_of_order_with_gaps_get_one_range_per_run_ascending() {
    let glyphs = vec![
        missing_glyph(),
        space(),
        glyph(0x42, [21, 22, 23, 24]),
        glyph(0x41, [11, 12, 13, 14]),
        glyph(0x44, [41, 42, 43, 44]),
    ];

    let built = build_field_font("saira32", &font(glyphs)).unwrap();

    let glps = chunk(&built.fnt, b"GLPS");
    assert_eq!(u32::from_le_bytes(glps[8..12].try_into().unwrap()), 5);
    assert_eq!(u32::from_le_bytes(glps[12..16].try_into().unwrap()), 4);
    assert_eq!(
        glps[16..],
        [
            0x20, 0, 0, 0, 1, 0, 0, 0, //
            0x41, 0, 0, 0, 2, 0, 0, 0, //
            0x44, 0, 0, 0, 1, 0, 0, 0, //
            0xA1, 0x25, 0, 0, 1, 0, 0, 0,
        ]
    );

    let tiles: Vec<(u16, u16)> = chunk(&built.fnt, b"TCRD")
        .as_chunks::<14>()
        .0
        .iter()
        .map(|entry| {
            (
                u16::from_le_bytes(entry[0..2].try_into().unwrap()),
                u16::from_le_bytes(entry[2..4].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(tiles, vec![(1, 1), (5, 1), (9, 1), (13, 1), (1, 5)]);
}

#[test]
fn the_atlas_is_one_power_of_two_level_holding_each_field_at_its_tile() {
    let glyphs = vec![
        glyph(0x20, [0, 0, 0, 0]),
        glyph(0x41, [11, 12, 13, 14]),
        glyph(0x42, [21, 22, 23, 24]),
        glyph(0x43, [31, 32, 33, 34]),
        glyph(0x25A1, [1, 2, 3, 4]),
    ];

    let built = build_field_font("saira32", &font(glyphs.clone())).unwrap();

    let levels = decode_edds(&built.texture).unwrap();
    assert_eq!(levels.len(), 1);
    let base = &levels[0];
    assert_eq!((base.width, base.height), (16, 8));
    assert!(base
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 255));
    assert_eq!(&base.pixels[(16 + 15) * 4..(16 + 16) * 4], [0, 0, 0, 255]);

    for (index, entry) in chunk(&built.fnt, b"TCRD")
        .as_chunks::<14>()
        .0
        .iter()
        .enumerate()
    {
        let (x, y) = (
            u16::from_le_bytes(entry[0..2].try_into().unwrap()),
            u16::from_le_bytes(entry[2..4].try_into().unwrap()),
        );
        let glyph = &glyphs[index];
        for row in 0..2u16 {
            for column in 0..2u16 {
                let texel = glyph.field[usize::from(row * 2 + column)];
                let at = (((y + row) * 16 + x + column) * 4) as usize;
                assert_eq!(&base.pixels[at..at + 3], texel);
            }
        }
    }
}

#[test]
fn an_atlas_grows_to_twice_as_wide_before_it_doubles_the_square() {
    let cell = 62u16;
    let field = vec![[0, 0, 0]; usize::from(cell) * usize::from(cell)];
    let wide = |codepoint: u16| FieldGlyph {
        field: field.clone(),
        ..glyph(codepoint, [0, 0, 0, 0])
    };
    let mut glyphs = vec![wide(0x20), wide(0x25A1)];
    glyphs.extend((0x41..0x47).map(wide));
    let rendered = FieldFont {
        cell,
        ..font(glyphs)
    };

    let built = build_field_font("saira32", &rendered).unwrap();

    let levels = decode_edds(&built.texture).unwrap();
    assert_eq!((levels[0].width, levels[0].height), (256, 128));
    assert!(chunk(&built.fnt, b"KERN").is_empty());
}

#[test]
fn build_field_font_refuses_fonts_the_game_cannot_load() {
    let long_name = "n".repeat(255);
    let short_field = FieldGlyph {
        field: vec![[0, 0, 0]; 3],
        ..glyph(0x41, [1, 2, 3, 4])
    };
    let cases = [
        (
            "saira32",
            font(Vec::new()),
            "a font needs at least one glyph",
        ),
        (
            long_name.as_str(),
            font(vec![space(), missing_glyph()]),
            "the font name is 255 bytes, more than 254",
        ),
        (
            "sai\0ra",
            font(vec![space(), missing_glyph()]),
            "the font name contains a NUL byte",
        ),
        (
            "saira32",
            font(vec![space(), missing_glyph(), short_field]),
            "the U+0041 glyph has 3 field texels for a 2x2 cell",
        ),
        (
            "saira32",
            font(vec![space(), missing_glyph(), glyph(0x1F, [1, 2, 3, 4])]),
            "U+001F is below U+0020, which the game skips",
        ),
        (
            "saira32",
            font(vec![missing_glyph()]),
            "the font has no U+0020 glyph",
        ),
        (
            "saira32",
            font(vec![space(), space(), missing_glyph()]),
            "two glyphs share U+0020",
        ),
        (
            "saira32",
            FieldFont {
                range: 40000,
                ..font(vec![space(), missing_glyph()])
            },
            "a range of 40000 pixels does not fit 16 bits",
        ),
        (
            "saira32",
            font(vec![space()]),
            "the font has no U+25A1 glyph",
        ),
        (
            "saira32",
            font(vec![
                space(),
                missing_glyph(),
                FieldGlyph {
                    width: 3,
                    ..glyph(0x41, [1, 2, 3, 4])
                },
            ]),
            "the U+0041 glyph is 3x2 pixels, larger than its 2 pixel cell",
        ),
        (
            "saira32",
            FieldFont {
                em: 0,
                ..font(vec![space(), missing_glyph()])
            },
            "an em of 0 pixels draws nothing",
        ),
    ];

    for (name, font, message) in cases {
        let error = build_field_font(name, &font).unwrap_err();

        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn an_atlas_that_would_outgrow_the_largest_texture_is_refused() {
    let cell = 1022u16;
    let wide = |codepoint: u16| FieldGlyph {
        field: vec![[0, 0, 0]; usize::from(cell) * usize::from(cell)],
        ..glyph(codepoint, [0, 0, 0, 0])
    };
    let mut glyphs = vec![wide(0x20), wide(0x25A1)];
    glyphs.extend((0x41..0x50).map(wide));
    let crowded = FieldFont {
        cell,
        ..font(glyphs)
    };

    assert_eq!(
        build_field_font("saira32", &crowded)
            .unwrap_err()
            .to_string(),
        "17 glyphs in 1022 pixel cells do not fit a 4096 pixel texture"
    );
}
