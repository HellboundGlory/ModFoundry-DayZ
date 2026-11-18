use modfoundry_font::{
    build_field_font, build_font, parse_fnt, FieldFont, FieldGlyph, FntFont, FntVersion, Glyph,
    Kerning, RenderedFont, SdfInfo,
};

fn glyph(codepoint: u16, width: u16, height: u16, left: i16, top: i16, advance: i16) -> Glyph {
    Glyph {
        codepoint,
        width,
        height,
        coverage: vec![255; usize::from(width) * usize::from(height)],
        left,
        top,
        advance,
    }
}

fn rendered_font() -> RenderedFont {
    RenderedFont {
        size: 22,
        ascent: 4,
        descent: 1,
        glyphs: vec![
            glyph(0x41, 2, 3, 1, 3, 4),
            glyph(0x20, 0, 0, 0, 0, 4),
            glyph(0x25A1, 2, 2, 0, 2, 3),
        ],
        kerning: vec![(0x41, 0x25A1, -2)],
    }
}

#[test]
fn reads_back_a_bitmap_font_from_the_writer() {
    let built = build_font("saira22", &rendered_font()).unwrap();
    let font = parse_fnt(&built.fnt).unwrap();

    assert_eq!(font.version, FntVersion::Fnt3);
    assert_eq!(font.name, "saira22");
    assert_eq!(font.size, 22);
    assert_eq!(font.sdf, None);
    let codepoints: Vec<u32> = font.glyphs.iter().map(|glyph| glyph.codepoint).collect();
    assert_eq!(codepoints, [0x20, 0x41, 0x25A1]);
    let widths: Vec<u16> = font.glyphs.iter().map(|glyph| glyph.w).collect();
    assert_eq!(widths, [0, 2, 2]);
    let advances: Vec<i16> = font.glyphs.iter().map(|glyph| glyph.advance).collect();
    assert_eq!(advances, [4, 4, 3]);
    assert_eq!(font.glyph(0x41).unwrap().bearing_x, 1);
    assert_eq!(
        (font.glyph(0x41).unwrap().x, font.glyph(0x41).unwrap().y),
        (2, 1)
    );
    assert_eq!(
        (font.glyph(0x25A1).unwrap().x, font.glyph(0x25A1).unwrap().y),
        (5, 1)
    );
    assert_eq!(font.glyph(0x41).unwrap().bearing_y, None);
    assert_eq!(font.line_height(), f32::from(font.glyphs[0].h));
    assert_eq!(font.line_height(), 5.0);
    assert_eq!(
        font.kerning,
        [Kerning {
            left: 0x41,
            right: 0x25A1,
            amount: -2
        }]
    );
}

fn field_glyph(codepoint: u16, width: u16, height: u16, top: i16, advance: i16) -> FieldGlyph {
    FieldGlyph {
        codepoint,
        width,
        height,
        left: 0,
        top,
        advance,
        field: vec![[1, 2, 3]; 4],
    }
}

#[test]
fn reads_back_a_distance_field_font_from_the_writer() {
    let mut boxed_glyph = field_glyph(0x25A1, 2, 2, 2, 3);
    boxed_glyph.left = -1;
    let font = FieldFont {
        em: 32,
        range: 8,
        cap_height: 20.0,
        cell: 2,
        glyphs: vec![boxed_glyph, field_glyph(0x20, 0, 0, 0, 4)],
        kerning: Vec::new(),
    };
    let built = build_field_font("saira32", &font).unwrap();
    let parsed = parse_fnt(&built.fnt).unwrap();

    assert_eq!(parsed.version, FntVersion::Fnt5);
    assert_eq!(parsed.name, "saira32");
    assert_eq!(parsed.size, 32);
    assert_eq!(
        parsed.sdf,
        Some(SdfInfo {
            multi_channel: true,
            cell: 2,
            cap_height: 20.0,
            em: 32.0,
            range: 8,
            bold: false,
            italic: false,
        })
    );
    assert_eq!(parsed.line_height(), 32.0);
    let boxed = parsed.glyph(0x25A1).unwrap();
    assert_eq!((boxed.x, boxed.y), (5, 1));
    assert_eq!((boxed.w, boxed.h), (2, 2));
    assert_eq!(boxed.bearing_x, -1);
    assert_eq!(boxed.bearing_y, Some(2));
    assert_eq!(boxed.advance, 3);
}

fn container(version: &[u8; 4], chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut bytes = b"FORM".to_vec();
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(version);
    for (tag, payload) in chunks {
        bytes.extend_from_slice(*tag);
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
    }
    let length = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&length.to_be_bytes());
    bytes
}

fn head3(name: &str, size: u32) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(&((name.len() + 1) as u32).to_le_bytes());
    head.extend_from_slice(name.as_bytes());
    head.push(0);
    head.extend_from_slice(&size.to_le_bytes());
    head.extend_from_slice(&0u32.to_le_bytes());
    head
}

fn glps3(glyph_count: u32, ranges: &[(u16, u16)]) -> Vec<u8> {
    let mut glps = vec![0; 8];
    glps.extend_from_slice(&glyph_count.to_le_bytes());
    glps.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
    for (first, count) in ranges {
        glps.extend_from_slice(&first.to_le_bytes());
        glps.extend_from_slice(&count.to_le_bytes());
    }
    glps
}

/// Each entry is (x, w, left, right); y is 0 and h is 5.
fn tcrd3(entries: &[(u16, u16, i16, i16)]) -> Vec<u8> {
    let mut tcrd = Vec::new();
    for &(x, w, left, right) in entries {
        for value in [x, 0, w, 5] {
            tcrd.extend_from_slice(&value.to_le_bytes());
        }
        tcrd.extend_from_slice(&left.to_le_bytes());
        tcrd.extend_from_slice(&right.to_le_bytes());
    }
    tcrd
}

fn kern_pairs(pairs: &[(u16, u16, i32)]) -> Vec<u8> {
    let mut kern = Vec::new();
    for &(left, right, amount) in pairs {
        kern.extend_from_slice(&left.to_le_bytes());
        kern.extend_from_slice(&right.to_le_bytes());
        kern.extend_from_slice(&amount.to_le_bytes());
    }
    kern
}

#[test]
fn chunk_order_does_not_change_the_font() {
    let head = head3("ddin", 18);
    let glps = glps3(2, &[(0x41, 2)]);
    let tcrd = tcrd3(&[(1, 2, 0, 1), (4, 3, 1, 1)]);
    let kern = kern_pairs(&[(0x41, 0x42, -1)]);
    let head_first = container(
        b"FNT3",
        &[
            (b"HEAD", head.clone()),
            (b"GLPS", glps.clone()),
            (b"TCRD", tcrd.clone()),
            (b"KERN", kern.clone()),
        ],
    );
    let legacy_order = container(
        b"FNT3",
        &[
            (b"GLPS", glps),
            (b"TCRD", tcrd),
            (b"KERN", kern),
            (b"HEAD", head),
        ],
    );

    assert_eq!(
        parse_fnt(&legacy_order).unwrap(),
        parse_fnt(&head_first).unwrap()
    );
}

#[test]
fn reads_a_head_chunk_of_odd_length_with_no_padding() {
    let bytes = container(
        b"FNT3",
        &[
            (b"HEAD", head3("ab", 7)),
            (b"GLPS", glps3(1, &[(0x41, 1)])),
            (b"TCRD", tcrd3(&[(0, 2, 0, 1)])),
        ],
    );
    assert_eq!(head3("ab", 7).len() % 2, 1);

    let font = parse_fnt(&bytes).unwrap();
    assert_eq!(font.name, "ab");
    assert_eq!(font.size, 7);
    assert_eq!(font.glyphs.len(), 1);
}

#[test]
fn skips_code_points_below_space_without_using_an_index() {
    let bytes = container(
        b"FNT3",
        &[
            (b"HEAD", head3("ddin", 18)),
            (b"GLPS", glps3(3, &[(0x1E, 5)])),
            (
                b"TCRD",
                tcrd3(&[(10, 1, 0, 0), (20, 2, 0, 0), (30, 3, 0, 0)]),
            ),
        ],
    );

    let font = parse_fnt(&bytes).unwrap();
    let codepoints: Vec<u32> = font.glyphs.iter().map(|glyph| glyph.codepoint).collect();
    assert_eq!(codepoints, [0x20, 0x21, 0x22]);
    assert_eq!(font.glyphs[0].x, 10);
    assert_eq!(font.glyphs[2].x, 30);
}

/// A FNT3 font with one glyph per code point, each 1 pixel wide.
fn font_of(codepoints: &[u16], kern: &[(u16, u16, i32)]) -> FntFont {
    let ranges: Vec<(u16, u16)> = codepoints.iter().map(|&codepoint| (codepoint, 1)).collect();
    let entries: Vec<(u16, u16, i16, i16)> = (0..codepoints.len())
        .map(|index| (index as u16, 1, 0, 0))
        .collect();
    let bytes = container(
        b"FNT3",
        &[
            (b"HEAD", head3("test", 8)),
            (b"GLPS", glps3(codepoints.len() as u32, &ranges)),
            (b"TCRD", tcrd3(&entries)),
            (b"KERN", kern_pairs(kern)),
        ],
    );
    parse_fnt(&bytes).unwrap()
}

#[test]
fn falls_back_to_the_box_then_to_space() {
    let with_box = font_of(&[0x20, 0x41, 0x25A1], &[]);
    let without_box = font_of(&[0x20, 0x41], &[]);

    assert_eq!(with_box.glyph(0x42), None);
    assert_eq!(with_box.glyph_or_fallback(0x42).unwrap().codepoint, 0x25A1);
    assert_eq!(without_box.glyph_or_fallback(0x42).unwrap().codepoint, 0x20);
    assert_eq!(without_box.glyph_or_fallback(0x41).unwrap().codepoint, 0x41);
}

#[test]
fn kern_finds_stored_pairs_and_returns_zero_for_others() {
    let font = font_of(&[0x41, 0x42, 0x43], &[(0x43, 0x41, 2), (0x41, 0x42, -3)]);

    assert_eq!(font.kern(0x41, 0x42), -3);
    assert_eq!(font.kern(0x43, 0x41), 2);
    assert_eq!(font.kern(0x42, 0x41), 0);
}

#[test]
fn rejects_broken_files_with_a_named_cause() {
    let valid_chunks = [
        (b"HEAD", head3("ddin", 18)),
        (b"GLPS", glps3(1, &[(0x41, 1)])),
        (b"TCRD", tcrd3(&[(0, 1, 0, 0)])),
    ];
    let mut not_form = container(b"FNT3", &valid_chunks);
    not_form[..4].copy_from_slice(b"RIFF");
    let mut truncated = container(b"FNT3", &valid_chunks);
    truncated.truncate(truncated.len() - 3);
    let three_glyphs = [
        (b"HEAD", head3("ddin", 18)),
        (b"GLPS", glps3(2, &[(0x41, 3)])),
        (b"TCRD", tcrd3(&[(0, 1, 0, 0), (1, 1, 0, 0), (2, 1, 0, 0)])),
    ];
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("not FORM", not_form, "FORM"),
        ("FNT4", container(b"FNT4", &valid_chunks), "FNT4"),
        ("unknown version", container(b"FNT9", &valid_chunks), "FNT9"),
        ("truncated chunk", truncated, "TCRD"),
        (
            "glyph count mismatch",
            container(b"FNT3", &three_glyphs),
            "GLPS declares 2 glyphs, its ranges give 3, TCRD has 3",
        ),
        (
            "no HEAD",
            container(b"FNT3", &[(b"GLPS", glps3(0, &[])), (b"TCRD", Vec::new())]),
            "HEAD",
        ),
        (
            "no TCRD",
            container(
                b"FNT3",
                &[(b"HEAD", head3("ddin", 18)), (b"GLPS", glps3(0, &[]))],
            ),
            "TCRD",
        ),
        (
            "glyph count over the limit",
            container(
                b"FNT3",
                &[
                    (b"HEAD", head3("ddin", 18)),
                    (b"GLPS", glps3(70_001, &[])),
                    (b"TCRD", Vec::new()),
                ],
            ),
            "70000",
        ),
        ("file over 16 MiB", vec![0; 16 * 1024 * 1024 + 1], "16 MiB"),
        (
            "HEAD cut short",
            container(
                b"FNT3",
                &[
                    (b"HEAD", vec![100, 0, 0, 0, b'a']),
                    (b"GLPS", glps3(0, &[])),
                    (b"TCRD", Vec::new()),
                ],
            ),
            "HEAD",
        ),
    ];
    for (case, bytes, expected) in cases {
        let error = parse_fnt(&bytes).expect_err(case);
        assert!(format!("{error:#}").contains(expected), "{case}: {error:#}");
    }
}

#[test]
fn every_prefix_of_a_valid_file_parses_or_errors_without_panicking() {
    let field = FieldFont {
        em: 32,
        range: 8,
        cap_height: 20.0,
        cell: 2,
        glyphs: vec![
            field_glyph(0x25A1, 2, 2, 2, 3),
            field_glyph(0x20, 0, 0, 0, 4),
        ],
        kerning: vec![(0x25A1, 0x20, -1)],
    };
    let files = [
        build_font("saira22", &rendered_font()).unwrap().fnt,
        build_field_font("saira32", &field).unwrap().fnt,
    ];
    for file in files {
        for length in 0..file.len() {
            let _ = parse_fnt(&file[..length]);
        }
    }
}

fn head5(sdf_type: u8, em: f32) -> Vec<u8> {
    let mut head = head3("ddin", 16);
    head.push(sdf_type);
    head.extend_from_slice(&32u32.to_le_bytes());
    head.extend_from_slice(&12f32.to_le_bytes());
    head.extend_from_slice(&em.to_le_bytes());
    head.extend_from_slice(&4i16.to_le_bytes());
    head.extend_from_slice(&[0, 0]);
    head.extend_from_slice(&em.to_le_bytes());
    head
}

fn glps5(glyph_count: u32, ranges: &[(u32, u16)]) -> Vec<u8> {
    let mut glps = vec![0; 8];
    glps.extend_from_slice(&glyph_count.to_le_bytes());
    glps.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
    for (first, count) in ranges {
        glps.extend_from_slice(&first.to_le_bytes());
        glps.extend_from_slice(&count.to_le_bytes());
        glps.extend_from_slice(&[0, 0]);
    }
    glps
}

#[test]
fn fnt5_head_reads_bold_and_italic_separately() {
    let mut head = head5(1, 16.0);
    let flags = head.len() - 6;
    head[flags] = 1;
    let mut tcrd = Vec::new();
    for value in [0u16, 0, 2, 2] {
        tcrd.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0i16, 1, 3] {
        tcrd.extend_from_slice(&value.to_le_bytes());
    }
    let bytes = container(
        b"FNT5",
        &[
            (b"HEAD", head),
            (b"GLPS", glps5(1, &[(0x41, 1)])),
            (b"TCRD", tcrd),
        ],
    );

    let sdf = parse_fnt(&bytes).unwrap().sdf.unwrap();
    assert!(sdf.bold);
    assert!(!sdf.italic);
}

#[test]
fn fnt5_with_sdf_type_zero_reads_as_a_bitmap_font() {
    let bytes = container(
        b"FNT5",
        &[
            (b"HEAD", head5(0, 16.0)),
            (b"GLPS", glps5(1, &[(0x41, 1)])),
            (b"TCRD", tcrd3(&[(3, 2, 1, 1)])),
        ],
    );

    let font = parse_fnt(&bytes).unwrap();
    assert_eq!(font.version, FntVersion::Fnt5);
    assert_eq!(font.sdf, None);
    let glyph = font.glyph(0x41).unwrap();
    assert_eq!(
        (glyph.x, glyph.w, glyph.bearing_x, glyph.advance),
        (3, 2, 1, 4)
    );
    assert_eq!(glyph.bearing_y, None);
    assert_eq!(font.line_height(), 5.0);
}

#[test]
#[ignore]
fn fnt_gui_fonts_parse() {
    let Some(folder) = std::env::var_os("MF_GUI_FONTS") else {
        return;
    };
    let mut count = 0;
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("fnt") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        if matches!(bytes.get(8..12), Some(b"FNT2" | b"FNT4")) {
            assert!(
                parse_fnt(&bytes).is_err(),
                "{} should be refused",
                path.display()
            );
            continue;
        }
        let font =
            parse_fnt(&bytes).unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        assert!(!font.glyphs.is_empty(), "{} has no glyphs", path.display());
        assert!(
            font.version == FntVersion::Fnt5 || font.line_height() > 0.0,
            "{} has no line height",
            path.display()
        );
        count += 1;
    }
    println!("parsed {count} fonts");
}

#[test]
fn ignores_a_wrong_form_length() {
    let mut bytes = container(
        b"FNT3",
        &[
            (b"HEAD", head3("ddin", 18)),
            (b"GLPS", glps3(1, &[(0x41, 1)])),
            (b"TCRD", tcrd3(&[(0, 1, 0, 0)])),
        ],
    );
    bytes[4..8].copy_from_slice(&1u32.to_be_bytes());

    assert_eq!(parse_fnt(&bytes).unwrap().glyphs.len(), 1);
}

#[test]
fn fnt5_sdf_type_one_is_single_channel() {
    let mut tcrd = Vec::new();
    for value in [0u16, 0, 2, 2] {
        tcrd.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0i16, 1, 3] {
        tcrd.extend_from_slice(&value.to_le_bytes());
    }
    let bytes = container(
        b"FNT5",
        &[
            (b"HEAD", head5(1, 16.0)),
            (b"GLPS", glps5(1, &[(0x41, 1)])),
            (b"TCRD", tcrd),
        ],
    );

    let font = parse_fnt(&bytes).unwrap();
    assert_eq!(font.sdf.as_ref().map(|sdf| sdf.multi_channel), Some(false));
    assert_eq!(font.glyph(0x41).unwrap().bearing_y, Some(1));
}
