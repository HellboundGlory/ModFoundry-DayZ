use crate::RgbaImage;
use anyhow::{anyhow, bail, ensure, Result};
use std::borrow::Cow;
use texpresso::{Algorithm, Format, Params};

// The width field's top bit is the LZO flag; the cap also bounds what a damaged header can make a reader allocate.
const MAX_SIDE: u32 = 8192;

/// The DXT pixel format of a `.paa`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaaFormat {
    Dxt1,
    Dxt5,
}

impl PaaFormat {
    /// The format ImageToPAA picks: DXT5 when any pixel is not fully opaque.
    pub fn for_image(image: &RgbaImage) -> PaaFormat {
        if image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] < 255)
        {
            PaaFormat::Dxt5
        } else {
            PaaFormat::Dxt1
        }
    }

    fn type_code(self) -> u16 {
        match self {
            PaaFormat::Dxt1 => 0xFF01,
            PaaFormat::Dxt5 => 0xFF05,
        }
    }

    fn texpresso(self) -> Format {
        match self {
            PaaFormat::Dxt1 => Format::Bc1,
            PaaFormat::Dxt5 => Format::Bc3,
        }
    }

    fn params(self) -> Params {
        Params {
            algorithm: Algorithm::ClusterFit,
            weigh_colour_by_alpha: self == PaaFormat::Dxt5,
            ..Params::default()
        }
    }
}

/// Encodes mip levels, largest first, as a `.paa` laid out like DayZ Tools' ImageToPAA.
pub fn encode_paa(levels: &[RgbaImage], format: PaaFormat) -> Result<Vec<u8>> {
    let base = levels
        .first()
        .ok_or_else(|| anyhow!("a .paa needs at least one mip level"))?;
    ensure!(
        levels.len() <= 16,
        "a .paa holds at most 16 mip levels, got {}",
        levels.len()
    );
    let records = levels
        .iter()
        .map(|level| encode_record(level, format))
        .collect::<Result<Vec<_>>>()?;

    let mut out = Vec::new();
    out.extend(format.type_code().to_le_bytes());
    push_block(&mut out, b"GGATCGVA", &average_colour(base));
    push_block(&mut out, b"GGATCXAM", &[0xFF; 4]);
    if format == PaaFormat::Dxt5 && PaaFormat::for_image(base) == PaaFormat::Dxt5 {
        push_block(&mut out, b"GGATGALF", &1u32.to_le_bytes());
    }

    // The SFFO block (name, length, 16 offsets) and the palette count come before the records.
    let table_len = out.len() + 8 + 4 + 64 + 2;
    let mut offsets = [0u32; 16];
    let mut at = table_len;
    for (slot, record) in offsets.iter_mut().zip(&records) {
        *slot = u32::try_from(at)?;
        at += record.len();
    }
    let table: Vec<u8> = offsets
        .iter()
        .flat_map(|offset| offset.to_le_bytes())
        .collect();
    push_block(&mut out, b"GGATSFFO", &table);
    out.extend([0, 0]);

    out.extend(records.iter().flatten());
    out.extend([0; 6]);
    Ok(out)
}

fn push_block(out: &mut Vec<u8>, name: &[u8; 8], payload: &[u8]) {
    out.extend(name);
    out.extend((payload.len() as u32).to_le_bytes());
    out.extend(payload);
}

fn encode_record(level: &RgbaImage, format: PaaFormat) -> Result<Vec<u8>> {
    ensure!(
        level.width <= MAX_SIDE && level.height <= MAX_SIDE,
        "a {}x{} mip level is larger than a .paa holds ({MAX_SIDE} per side)",
        level.width,
        level.height
    );
    let (width, height) = (level.width as usize, level.height as usize);
    let mut blocks = vec![0; format.texpresso().compressed_size(width, height)];
    format
        .texpresso()
        .compress(&level.pixels, width, height, format.params(), &mut blocks);

    let (width_field, data) = if level.width >= 256 {
        (
            u16::try_from(level.width)? | 0x8000,
            lzokay::compress::compress(&blocks)?,
        )
    } else {
        (u16::try_from(level.width)?, blocks)
    };
    let stored = u32::try_from(data.len())?;
    ensure!(
        stored <= 0xFF_FFFF,
        "a {}x{} mip level stores {stored} bytes, more than a .paa size field holds",
        level.width,
        level.height
    );
    let mut record = Vec::with_capacity(7 + data.len());
    record.extend(width_field.to_le_bytes());
    record.extend(u16::try_from(level.height)?.to_le_bytes());
    record.extend(&stored.to_le_bytes()[..3]);
    record.extend(data);
    Ok(record)
}

fn average_colour(image: &RgbaImage) -> [u8; 4] {
    let mut linear = [0.0; 3];
    let mut alpha = 0.0;
    for pixel in image.pixels.as_chunks::<4>().0 {
        for (sum, &channel) in linear.iter_mut().zip(pixel) {
            *sum += srgb_to_linear(channel);
        }
        alpha += f64::from(pixel[3]);
    }
    let count = (image.pixels.len() / 4) as f64;
    let [r, g, b] = linear.map(|sum| linear_to_srgb(sum / count));
    [b, g, r, (alpha / count).round() as u8]
}

fn srgb_to_linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(linear: f64) -> u8 {
    let value = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0).round() as u8
}

/// Decodes a `.paa` into its format and mip levels, largest first.
pub fn decode_paa(bytes: &[u8]) -> Result<(PaaFormat, Vec<RgbaImage>)> {
    let mut cursor = Cursor { bytes, at: 0 };
    let format = match u16::from_le_bytes(cursor.array()?) {
        0xFF01 => PaaFormat::Dxt1,
        0xFF05 => PaaFormat::Dxt5,
        other => bail!("unsupported .paa type 0x{other:04X}; only DXT1 and DXT5 are read"),
    };
    while cursor.starts_with(b"GGAT") {
        cursor.take(8)?;
        let length = u32::from_le_bytes(cursor.array()?);
        cursor.take(length as usize)?;
    }
    let palette = u16::from_le_bytes(cursor.array()?);
    cursor.take(usize::from(palette) * 3)?;

    let mut levels = Vec::new();
    loop {
        let width_field = u16::from_le_bytes(cursor.array()?);
        let height = u16::from_le_bytes(cursor.array()?);
        if width_field == 0 && height == 0 {
            return Ok((format, levels));
        }
        levels.push(decode_record(&mut cursor, format, width_field, height)?);
    }
}

fn decode_record(
    cursor: &mut Cursor,
    format: PaaFormat,
    width_field: u16,
    height: u16,
) -> Result<RgbaImage> {
    let [low, middle, high] = cursor.array::<3>()?;
    let stored = usize::from(low) | usize::from(middle) << 8 | usize::from(high) << 16;
    let data = cursor.take(stored)?;

    let (width, height) = (usize::from(width_field & 0x7FFF), usize::from(height));
    ensure!(width > 0 && height > 0, "a .paa mip level has no pixels");
    ensure!(
        width <= MAX_SIDE as usize && height <= MAX_SIDE as usize,
        "a {width}x{height} mip level is larger than {MAX_SIDE} per side"
    );
    let needed = format.texpresso().compressed_size(width, height);
    let blocks = if width_field & 0x8000 != 0 {
        let mut blocks = vec![0; needed];
        let written = lzokay::decompress::decompress(data, &mut blocks)
            .map_err(|err| anyhow!("a {width}x{height} mip level is damaged LZO: {err}"))?;
        ensure!(
            written == needed,
            "a {width}x{height} mip level decompresses to {written} bytes, expected {needed}"
        );
        Cow::Owned(blocks)
    } else {
        ensure!(
            data.len() == needed,
            "a {width}x{height} mip level stores {} bytes, expected {needed}",
            data.len()
        );
        Cow::Borrowed(data)
    };

    let mut pixels = vec![0; width * height * 4];
    format
        .texpresso()
        .decompress(&blocks, width, height, &mut pixels);
    Ok(RgbaImage {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| anyhow!("unexpected end of .paa data at byte {}", self.at))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.at..].starts_with(prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mips::{mip_chain, paa_level_count};
    use crate::png::decode_png;

    const GRAD_CO_PNG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/grad_co.png"
    ));
    const WHITE_CA_PNG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/white_ca.png"
    ));
    const WIDE_CA_PNG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wide_ca.png"
    ));
    const GRAD_CO_PAA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/grad_co.paa"
    ));
    const WHITE_CA_PAA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/white_ca.paa"
    ));
    const WIDE_CA_PAA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wide_ca.paa"
    ));

    fn assert_close(decoded: &RgbaImage, expected: &RgbaImage) {
        assert_eq!(
            (decoded.width, decoded.height),
            (expected.width, expected.height)
        );
        let total: f64 = decoded
            .pixels
            .iter()
            .zip(&expected.pixels)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum();
        let mean = total / decoded.pixels.len() as f64;
        assert!(mean < 8.0, "mean difference {mean}");
    }

    fn record_header(paa: &[u8], offset: usize) -> (u16, u16, usize) {
        let size = &paa[offset + 4..offset + 7];
        (
            u16::from_le_bytes([paa[offset], paa[offset + 1]]),
            u16::from_le_bytes([paa[offset + 2], paa[offset + 3]]),
            usize::from(size[0]) | usize::from(size[1]) << 8 | usize::from(size[2]) << 16,
        )
    }

    fn assert_record_headers_match(ours: &[u8], reference: &[u8]) {
        for offset in mip_offsets(reference) {
            assert_eq!(&ours[offset..offset + 7], &reference[offset..offset + 7]);
        }
    }

    fn levels_of(png: &[u8]) -> Vec<RgbaImage> {
        let base = decode_png(png).unwrap();
        let count = paa_level_count(base.width, base.height);
        mip_chain(base, count)
    }

    fn mip_offsets(paa: &[u8]) -> Vec<usize> {
        let table = paa.windows(8).position(|name| name == b"GGATSFFO").unwrap() + 12;
        paa[table..table + 64]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|entry| u32::from_le_bytes(*entry) as usize)
            .take_while(|&offset| offset != 0)
            .collect()
    }

    #[test]
    fn alpha_below_full_opacity_selects_dxt5() {
        let opaque = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 40, 50, 60, 255],
        };
        let translucent = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 40, 50, 60, 254],
        };

        assert_eq!(PaaFormat::for_image(&opaque), PaaFormat::Dxt1);
        assert_eq!(PaaFormat::for_image(&translucent), PaaFormat::Dxt5);
    }

    #[test]
    fn only_dxt5_of_an_image_with_alpha_gets_the_alpha_flag() {
        let levels = levels_of(WHITE_CA_PNG);
        let has_flag = |format| {
            let paa = encode_paa(&levels, format).unwrap();
            paa.windows(8).any(|name| name == b"GGATGALF")
        };

        assert!(has_flag(PaaFormat::Dxt5));
        assert!(!has_flag(PaaFormat::Dxt1));
    }

    #[test]
    fn opaque_dxt1_header_and_record_headers_match_image_to_paa() {
        let levels = levels_of(GRAD_CO_PNG);

        let ours = encode_paa(&levels, PaaFormat::Dxt1).unwrap();

        let first_record = mip_offsets(GRAD_CO_PAA)[0];
        assert_eq!(&ours[..first_record], &GRAD_CO_PAA[..first_record]);
        assert_record_headers_match(&ours, GRAD_CO_PAA);
    }

    #[test]
    fn alpha_dxt5_header_matches_image_to_paa_apart_from_average_colour() {
        let levels = levels_of(WHITE_CA_PNG);

        let ours = encode_paa(&levels, PaaFormat::Dxt5).unwrap();

        let first_record = mip_offsets(WHITE_CA_PAA)[0];
        assert_eq!(&ours[..14], &WHITE_CA_PAA[..14]);
        assert_eq!(ours[17], WHITE_CA_PAA[17]);
        assert_eq!(&ours[18..first_record], &WHITE_CA_PAA[18..first_record]);
        assert_record_headers_match(&ours, WHITE_CA_PAA);
    }

    #[test]
    fn wide_dxt5_packs_the_top_level_with_lzo_and_points_the_table_at_each_record() {
        let levels = levels_of(WIDE_CA_PNG);

        let ours = encode_paa(&levels, PaaFormat::Dxt5).unwrap();

        let offsets = mip_offsets(&ours);
        let headers: Vec<_> = offsets
            .iter()
            .map(|&offset| record_header(&ours, offset))
            .collect();
        let reference: Vec<_> = mip_offsets(WIDE_CA_PAA)
            .iter()
            .map(|&offset| record_header(WIDE_CA_PAA, offset))
            .collect();
        assert_eq!(
            (headers[0].0, headers[0].1),
            (reference[0].0, reference[0].1)
        );
        assert_eq!(headers[1..], reference[1..]);
        for (index, &(_, _, size)) in headers.iter().enumerate() {
            let next = offsets.get(index + 1).copied().unwrap_or(ours.len() - 6);
            assert_eq!(offsets[index] + 7 + size, next);
        }
    }

    #[test]
    fn image_to_paa_fixtures_decode_to_their_format_sizes_and_level_zero() {
        type Case = (
            &'static [u8],
            &'static [u8],
            PaaFormat,
            &'static [(u32, u32)],
        );
        let cases: [Case; 3] = [
            (
                GRAD_CO_PAA,
                GRAD_CO_PNG,
                PaaFormat::Dxt1,
                &[(64, 32), (32, 16), (16, 8), (8, 4)],
            ),
            (
                WHITE_CA_PAA,
                WHITE_CA_PNG,
                PaaFormat::Dxt5,
                &[(64, 64), (32, 32), (16, 16), (8, 8), (4, 4)],
            ),
            (
                WIDE_CA_PAA,
                WIDE_CA_PNG,
                PaaFormat::Dxt5,
                &[(256, 32), (128, 16), (64, 8), (32, 4)],
            ),
        ];

        for (paa, png, format, sizes) in cases {
            let (decoded_format, levels) = decode_paa(paa).unwrap();

            assert_eq!(decoded_format, format);
            let decoded_sizes: Vec<_> = levels
                .iter()
                .map(|level| (level.width, level.height))
                .collect();
            assert_eq!(decoded_sizes, sizes);
            assert_close(&levels[0], &decode_png(png).unwrap());
        }
    }

    #[test]
    fn encoded_dxt5_decodes_to_levels_close_to_their_input() {
        let levels = levels_of(WIDE_CA_PNG);

        let (format, decoded) = decode_paa(&encode_paa(&levels, PaaFormat::Dxt5).unwrap()).unwrap();

        assert_eq!(format, PaaFormat::Dxt5);
        assert_eq!(decoded.len(), levels.len());
        for (decoded, level) in decoded.iter().zip(&levels) {
            assert_close(decoded, level);
        }
    }

    #[test]
    fn unknown_types_and_files_cut_inside_a_mip_record_are_refused() {
        let mut unknown_type = GRAD_CO_PAA.to_vec();
        unknown_type[..2].copy_from_slice(&[0x88, 0x88]);
        let mut huge = GRAD_CO_PAA.to_vec();
        let first = mip_offsets(GRAD_CO_PAA)[0];
        huge[first..first + 2].copy_from_slice(&(0x8000u16 | 9000).to_le_bytes());
        let cases: [(&[u8], &str); 3] = [
            (&unknown_type, "unsupported .paa type 0x8888"),
            (&huge, "larger than 8192 per side"),
            (&GRAD_CO_PAA[..120], "unexpected end of .paa data"),
        ];

        for (bytes, message) in cases {
            let error = decode_paa(bytes).unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }
}
