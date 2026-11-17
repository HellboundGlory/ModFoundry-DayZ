use modfoundry_font::{build_font, Glyph, RenderedFont};
use modfoundry_texture::edds::decode_edds;

fn space() -> Glyph {
    Glyph {
        codepoint: 0x20,
        width: 0,
        height: 0,
        coverage: Vec::new(),
        left: 0,
        top: 0,
        advance: 4,
    }
}

fn font(glyphs: Vec<Glyph>) -> RenderedFont {
    RenderedFont {
        size: 22,
        ascent: 3,
        descent: 2,
        glyphs,
        kerning: Vec::new(),
    }
}

fn glyph(codepoint: u16, width: u16) -> Glyph {
    Glyph {
        codepoint,
        width,
        height: 1,
        coverage: vec![9; usize::from(width)],
        left: 0,
        top: 1,
        advance: width as i16 + 1,
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

fn rects(fnt: &[u8]) -> Vec<[u32; 4]> {
    chunk(fnt, b"TCRD")
        .as_chunks::<12>()
        .0
        .iter()
        .map(|entry| {
            let field = |at: usize| u32::from(u16::from_le_bytes([entry[at], entry[at + 1]]));
            [field(0), field(2), field(4), field(6)]
        })
        .collect()
}

#[test]
fn textures_are_square_or_twice_as_wide_with_two_free_pixels_at_the_far_edges() {
    for count in [1u16, 9, 40, 150, 600] {
        let mut glyphs = vec![space()];
        glyphs.extend((0..count).map(|index| glyph(0x21 + index, 11)));

        let built = build_font("saira22", &font(glyphs)).unwrap();

        let base = &decode_edds(&built.texture).unwrap()[0];
        let (width, height) = (base.width, base.height);
        assert!(
            width == height || width == 2 * height,
            "{count} glyphs: {width}x{height}"
        );
        for [x, y, w, h] in rects(&built.fnt) {
            assert!(
                x + w + 2 <= width && y + h + 2 <= height,
                "{count} glyphs: {x},{y} {w}x{h} in {width}x{height}"
            );
        }
    }
}

#[test]
fn a_two_glyph_font_writes_the_expected_fnt_bytes() {
    let glyphs = vec![
        space(),
        Glyph {
            codepoint: 0x21,
            width: 2,
            height: 3,
            coverage: vec![1, 2, 3, 4, 5, 6],
            left: 1,
            top: 3,
            advance: 4,
        },
    ];

    let built = build_font("saira22", &font(glyphs)).unwrap();

    let expected: &[u8] = &[
        // FORM, file length - 8, FNT3
        b'F', b'O', b'R', b'M', 0, 0, 0, 100, b'F', b'N', b'T', b'3', //
        // HEAD, 20 bytes: name length with NUL, "saira22", NUL, size, zero
        b'H', b'E', b'A', b'D', 0, 0, 0, 20, //
        8, 0, 0, 0, b's', b'a', b'i', b'r', b'a', b'2', b'2', 0, //
        22, 0, 0, 0, //
        0, 0, 0, 0, //
        // GLPS, 20 bytes: two unused words, two glyphs in one range from U+0020
        b'G', b'L', b'P', b'S', 0, 0, 0, 20, //
        0, 0, 0, 0, //
        0, 0, 0, 0, //
        2, 0, 0, 0, //
        1, 0, 0, 0, //
        0x20, 0, 2, 0, //
        // TCRD, 24 bytes: space at (1,1) then the 2x3 glyph at (2,1), cell height 5
        b'T', b'C', b'R', b'D', 0, 0, 0, 24, //
        1, 0, 1, 0, 0, 0, 5, 0, 0, 0, 4, 0, //
        2, 0, 1, 0, 2, 0, 5, 0, 1, 0, 1, 0, //
        // KERN, empty
        b'K', b'E', b'R', b'N', 0, 0, 0, 0,
    ];

    assert_eq!(built.fnt.as_slice(), expected);
}

#[test]
fn glyphs_given_out_of_order_get_one_range_per_run_in_code_point_order() {
    let glyphs = vec![glyph(0x42, 4), space(), glyph(0x41, 3)];

    let built = build_font("saira22", &font(glyphs)).unwrap();

    let glps = chunk(&built.fnt, b"GLPS");
    assert_eq!(u32::from_le_bytes(glps[8..12].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(glps[12..16].try_into().unwrap()), 2);
    assert_eq!(glps[16..], [0x20, 0, 1, 0, 0x41, 0, 2, 0]);

    let tcrd = chunk(&built.fnt, b"TCRD");
    let entries: Vec<(u16, u16)> = tcrd
        .as_chunks::<12>()
        .0
        .iter()
        .map(|entry| {
            (
                u16::from_le_bytes(entry[0..2].try_into().unwrap()),
                u16::from_le_bytes(entry[4..6].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(entries, vec![(1, 0), (2, 3), (6, 4)]);
}

#[test]
fn the_texture_holds_each_glyph_coverage_in_its_tcrd_rect() {
    let ink = Glyph {
        codepoint: 0x21,
        width: 2,
        height: 3,
        coverage: vec![1, 2, 3, 4, 5, 6],
        left: 1,
        top: 1,
        advance: 4,
    };
    let glyphs = vec![space(), ink.clone()];

    let rendered = font(glyphs.clone());
    let ascent = 3;
    let built = build_font("saira22", &rendered).unwrap();

    let levels = decode_edds(&built.texture).unwrap();
    let base = &levels[0];
    assert_eq!((base.width, base.height), (8, 8));
    assert!(base
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[..3] == [255, 255, 255]));

    let rects = rects(&built.fnt);
    assert_eq!(rects, vec![[1, 1, 0, 5], [2, 1, 2, 5]]);

    let mut expected = vec![0u8; (base.width * base.height) as usize];
    for (glyph, [x, y, width, height]) in glyphs.iter().zip(&rects) {
        let offset = ascent - i32::from(glyph.top);
        for row in 0..*height {
            for column in 0..*width {
                let ink_row = row as i32 - offset;
                let value = if ink_row < 0 {
                    0
                } else {
                    let index = ink_row as usize * usize::from(glyph.width) + column as usize;
                    glyph.coverage.get(index).copied().unwrap_or(0)
                };
                expected[((y + row) * base.width + x + column) as usize] = value;
            }
        }
    }

    let alpha: Vec<u8> = base
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[3])
        .collect();
    assert_eq!(alpha, expected);
}

#[test]
fn a_glyph_reaching_above_the_ascent_raises_the_cell_height() {
    let rendered = RenderedFont {
        ascent: 1,
        descent: 0,
        ..font(vec![
            space(),
            Glyph {
                codepoint: 0x41,
                width: 1,
                height: 3,
                coverage: vec![7, 8, 9],
                left: 0,
                top: 2,
                advance: 2,
            },
        ])
    };

    let built = build_font("saira22", &rendered).unwrap();

    let tcrd = chunk(&built.fnt, b"TCRD");
    assert_eq!(u16::from_le_bytes(tcrd[6..8].try_into().unwrap()), 3);
}

#[test]
fn a_cell_that_does_not_fit_starts_a_row_below_the_cell_height() {
    let rendered = font(vec![
        space(),
        glyph(0x41, 2000),
        glyph(0x42, 2000),
        glyph(0x43, 2000),
    ]);

    let built = build_font("saira22", &rendered).unwrap();

    let positions: Vec<[u32; 2]> = rects(&built.fnt)
        .iter()
        .map(|[x, y, ..]| [*x, *y])
        .collect();
    assert_eq!(positions, vec![[1, 1], [2, 1], [1, 7], [1, 13]]);
}

#[test]
fn kern_keeps_the_nonzero_pairs_between_glyphs_sorted() {
    let rendered = RenderedFont {
        kerning: vec![
            (0x41, 0x42, -1),
            (0x20, 0x41, 2),
            (0x41, 0x41, 0),
            (0x41, 0x99, 3),
        ],
        ..font(vec![space(), glyph(0x41, 2), glyph(0x42, 2)])
    };

    let built = build_font("saira22", &rendered).unwrap();

    let kern = chunk(&built.fnt, b"KERN");
    let pairs: Vec<(u16, u16, i32)> = kern
        .as_chunks::<8>()
        .0
        .iter()
        .map(|pair| {
            (
                u16::from_le_bytes(pair[0..2].try_into().unwrap()),
                u16::from_le_bytes(pair[2..4].try_into().unwrap()),
                i32::from_le_bytes(pair[4..8].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(pairs, vec![(0x20, 0x41, 2), (0x41, 0x42, -1)]);
}

#[test]
fn build_font_refuses_fonts_the_game_cannot_load() {
    let long_name = "n".repeat(255);
    let short_coverage = Glyph {
        width: 2,
        height: 2,
        coverage: vec![0, 0, 0],
        ..glyph(0x41, 2)
    };
    let cases = [
        (
            "saira22",
            font(Vec::new()),
            "a font needs at least one glyph",
        ),
        (
            "saira22",
            font(vec![space(), glyph(0x1F, 2)]),
            "U+001F is below U+0020, which the game skips",
        ),
        (
            "saira22",
            font(vec![space(), space()]),
            "two glyphs share U+0020",
        ),
        (
            "saira22",
            font(vec![space(), short_coverage]),
            "the U+0041 glyph has 3 coverage bytes for a 2x2 cell",
        ),
        (
            long_name.as_str(),
            font(vec![space()]),
            "the font name is 255 bytes, more than 254",
        ),
        (
            "sai\0ra",
            font(vec![space()]),
            "the font name contains a NUL byte",
        ),
        (
            "saira22",
            font(vec![glyph(0x41, 2)]),
            "the font has no U+0020 glyph",
        ),
        (
            "saira22",
            font(vec![space(), glyph(0x41, 4095)]),
            "a glyph 4095 pixels wide needs a 8192 pixel texture, past the 4096 pixel limit",
        ),
        (
            "saira22",
            RenderedFont {
                ascent: 3000,
                descent: 0,
                ..font(vec![space(), glyph(0x41, 3000), glyph(0x42, 3000)])
            },
            "the glyphs need a 4096x8192 texture, past the 4096 pixel limit",
        ),
        (
            "saira22",
            RenderedFont {
                ascent: 4094,
                descent: 0,
                ..font(vec![space(), glyph(0x41, 2)])
            },
            "the cells are 4094 pixels tall, too tall for a 4096 pixel texture",
        ),
        (
            "saira22",
            font(vec![
                space(),
                Glyph {
                    left: -30000,
                    advance: 30000,
                    ..glyph(0x41, 10)
                },
            ]),
            "the U+0041 glyph's right bearing 59990 does not fit 16 bits",
        ),
        (
            "saira22",
            RenderedFont {
                kerning: vec![(0x41, 0x42, -1), (0x41, 0x42, -2)],
                ..font(vec![space(), glyph(0x41, 2), glyph(0x42, 2)])
            },
            "two kerning pairs for U+0041 U+0042",
        ),
    ];

    for (name, font, message) in cases {
        let error = build_font(name, &font).unwrap_err();

        assert_eq!(error.to_string(), message);
    }
}
