use crate::RgbaImage;
use anyhow::{anyhow, bail, ensure, Result};
use std::borrow::Cow;

const HEADER_LEN: usize = 0x80;
const RECORD_LEN: usize = 8;
const CHUNK: usize = 65_536;
const LZ4_TAG: [u8; 4] = *b"LZ4 ";
const COPY_TAG: [u8; 4] = *b"COPY";

/// Encodes mip levels, largest first, as a `.edds` file.
pub fn encode_edds(levels: &[RgbaImage]) -> Result<Vec<u8>> {
    check_chain(levels)?;
    let base = &levels[0];
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(b"DDS ");
    push_u32(&mut out, 124);
    push_u32(&mut out, 0x0002_100F);
    push_u32(&mut out, base.height);
    push_u32(&mut out, base.width);
    push_u32(&mut out, base.width * 4);
    push_u32(&mut out, 0);
    push_u32(&mut out, levels.len() as u32);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"ENF1");
    out.extend_from_slice(&[0; 36]);
    push_u32(&mut out, 32);
    push_u32(&mut out, 0x41);
    push_u32(&mut out, 0);
    push_u32(&mut out, 32);
    for mask in [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        push_u32(&mut out, mask);
    }
    let caps = if levels.len() > 1 {
        0x0040_1008
    } else {
        0x0000_1000
    };
    push_u32(&mut out, caps);
    out.extend_from_slice(&[0; 16]);
    let bodies: Vec<([u8; 4], Vec<u8>)> = levels.iter().rev().map(encode_level).collect();
    for (tag, body) in &bodies {
        out.extend_from_slice(tag);
        push_u32(&mut out, body.len() as u32);
    }
    for (_, body) in &bodies {
        out.extend_from_slice(body);
    }
    Ok(out)
}

/// Refuses a chain the decoder would read back at other sizes than it was given.
fn check_chain(levels: &[RgbaImage]) -> Result<()> {
    ensure!(
        !levels.is_empty(),
        "an .edds file needs at least one mip level"
    );
    ensure!(
        levels.len() <= 32,
        "an .edds file cannot hold {} mip levels",
        levels.len()
    );
    let base = &levels[0];
    ensure!(
        base.width >= 1 && base.height >= 1,
        "level 0 is {}x{}, a level needs at least 1x1",
        base.width,
        base.height
    );
    for (level, image) in levels.iter().enumerate() {
        let (width, height) = level_size(base.width, base.height, level);
        ensure!(
            (image.width, image.height) == (width, height),
            "level {level} is {}x{}, a chain from {}x{} needs {width}x{height}",
            image.width,
            image.height,
            base.width,
            base.height
        );
        let expected = level_bytes(width, height)?;
        ensure!(
            image.pixels.len() == expected,
            "level {level} holds {} bytes, {width}x{height} needs {expected}",
            image.pixels.len()
        );
    }
    Ok(())
}

/// Decodes an `.edds` file into its mip levels, largest first.
pub fn decode_edds(bytes: &[u8]) -> Result<Vec<RgbaImage>> {
    ensure!(
        bytes.get(..4) == Some(b"DDS ".as_slice()),
        "not an .edds file"
    );
    ensure!(bytes.len() >= HEADER_LEN, "short .edds file");
    let height = u32_at(bytes, 0x0C);
    let width = u32_at(bytes, 0x10);
    let mip_count = u32_at(bytes, 0x1C).max(1) as usize;
    ensure!(
        mip_count <= 32,
        "an .edds file cannot hold {mip_count} mip levels"
    );
    let alpha_unused = pixel_format_alpha_unused(bytes)?;
    let table_end = HEADER_LEN + mip_count * RECORD_LEN;
    ensure!(bytes.len() >= table_end, "short .edds file");

    let mut body_start = table_end;
    let mut levels = Vec::with_capacity(mip_count);
    for (index, record) in bytes[HEADER_LEN..table_end]
        .as_chunks::<RECORD_LEN>()
        .0
        .iter()
        .enumerate()
    {
        let (level_width, level_height) = level_size(width, height, mip_count - 1 - index);
        let size = u32_at(record, 4) as usize;
        let body = bytes
            .get(body_start..body_start.saturating_add(size))
            .ok_or_else(|| anyhow!("short .edds file: a level body runs past the end"))?;
        body_start += size;
        levels.push(decode_level(
            &record[..4],
            body,
            level_width,
            level_height,
            alpha_unused,
        )?);
    }
    levels.reverse();
    Ok(levels)
}

fn encode_level(level: &RgbaImage) -> ([u8; 4], Vec<u8>) {
    let raw = bgra(&level.pixels);
    let lz4 = lz4_body(&raw);
    if lz4.len() < raw.len() {
        (LZ4_TAG, lz4)
    } else {
        (COPY_TAG, raw)
    }
}

fn lz4_body(raw: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    push_u32(&mut body, raw.len() as u32);
    let mut chunks = raw.chunks(CHUNK).peekable();
    while let Some(chunk) = chunks.next() {
        let packed = lz4_flex::block::compress(chunk);
        let flag = if chunks.peek().is_none() { 0x80 } else { 0 };
        body.extend_from_slice(&(packed.len() as u32).to_le_bytes()[..3]);
        body.push(flag);
        body.extend_from_slice(&packed);
    }
    body
}

fn decode_level(
    tag: &[u8],
    body: &[u8],
    width: u32,
    height: u32,
    alpha_unused: bool,
) -> Result<RgbaImage> {
    let raw_len = level_bytes(width, height)?;
    let raw = match tag {
        b"COPY" => {
            ensure!(
                body.len() == raw_len,
                "COPY body is {} bytes, a {width}x{height} level needs {raw_len}",
                body.len()
            );
            Cow::Borrowed(body)
        }
        b"LZ4 " => Cow::Owned(lz4_decode(body, raw_len)?),
        _ => bail!("unknown .edds table tag {:?}", String::from_utf8_lossy(tag)),
    };
    Ok(RgbaImage {
        width,
        height,
        pixels: rgba_from_bgra(&raw, alpha_unused),
    })
}

fn lz4_decode(body: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    ensure!(body.len() >= 4, "short .edds LZ4 body");
    let stored = u32_at(body, 0) as usize;
    // LZ4 expands at most 255 times, so a larger claim is damage and must not size an allocation.
    ensure!(
        stored <= body.len().saturating_mul(255),
        "LZ4 body of {} bytes cannot hold {stored} bytes",
        body.len()
    );
    ensure!(
        stored == raw_len,
        "LZ4 body holds {stored} bytes, a level needs {raw_len}"
    );
    let mut out = vec![0; raw_len];
    let mut written = 0;
    let mut position = 4;
    loop {
        ensure!(position + 4 <= body.len(), "LZ4 body has no final chunk");
        let packed_len =
            u32::from_le_bytes([body[position], body[position + 1], body[position + 2], 0])
                as usize;
        let flag = body[position + 3];
        ensure!(
            flag & !0x80 == 0,
            "LZ4 chunk flag {flag:#04x} is not 0x00 or 0x80"
        );
        let packed = body
            .get(position + 4..position + 4 + packed_len)
            .ok_or_else(|| anyhow!("short .edds file: an LZ4 chunk runs past the end"))?;
        let (done, rest) = out.split_at_mut(written);
        let size = rest.len().min(CHUNK);
        let dictionary = &done[done.len().saturating_sub(CHUNK)..];
        let decoded =
            lz4_flex::block::decompress_into_with_dict(packed, &mut rest[..size], dictionary)?;
        ensure!(
            decoded == size,
            "LZ4 chunk decodes to {decoded} bytes, not {size}"
        );
        written += size;
        position += 4 + packed_len;
        if flag == 0x80 {
            ensure!(position == body.len(), "data after the final LZ4 chunk");
            ensure!(
                written == raw_len,
                "LZ4 chunks end before the level is full"
            );
            return Ok(out);
        }
    }
}

fn pixel_format_alpha_unused(bytes: &[u8]) -> Result<bool> {
    let fourcc = &bytes[0x54..0x58];
    if fourcc != [0; 4] {
        bail!(
            "unsupported .edds pixel format: {}; only uncompressed RGBA is read",
            String::from_utf8_lossy(fourcc)
        );
    }
    let bits = u32_at(bytes, 0x58);
    let masks = [0x5C, 0x60, 0x64, 0x68].map(|offset| u32_at(bytes, offset));
    let alpha = masks[3];
    ensure!(
        bits == 32
            && masks[..3] == [0x00FF_0000, 0x0000_FF00, 0x0000_00FF]
            && (alpha == 0 || alpha == 0xFF00_0000),
        "unsupported .edds pixel format: {bits}; only uncompressed RGBA is read"
    );
    Ok(alpha == 0)
}

fn level_size(width: u32, height: u32, level: usize) -> (u32, u32) {
    let level = level as u32;
    (
        width.checked_shr(level).unwrap_or(0).max(1),
        height.checked_shr(level).unwrap_or(0).max(1),
    )
}

fn level_bytes(width: u32, height: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("{width}x{height} .edds level is too large"))
}

fn bgra(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect()
}

fn rgba_from_bgra(bgra: &[u8], alpha_unused: bool) -> Vec<u8> {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let alpha = if alpha_unused { 255 } else { pixel[3] };
            [pixel[2], pixel[1], pixel[0], alpha]
        })
        .collect()
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mips::{edds_level_count, mip_chain};

    const HEADER_64X64_7_LEVELS: [u8; 128] = [
        0x44, 0x44, 0x53, 0x20, 0x7c, 0x00, 0x00, 0x00, 0x0f, 0x10, 0x02, 0x00, 0x40, 0x00, 0x00,
        0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x45, 0x4e, 0x46, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x20, 0x00, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xff, 0x08, 0x10, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    fn stripes(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let pixel = if (x / 8 + y / 8) % 2 == 0 {
                    [200, 100, 50, 255]
                } else {
                    [10, 20, 30, 128]
                };
                pixels.extend_from_slice(&pixel);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn an_empty_chain_is_an_error_not_a_panic() {
        let error = encode_edds(&[]).unwrap_err().to_string();

        assert_eq!(error, "an .edds file needs at least one mip level");
    }

    #[test]
    fn a_chain_that_is_not_halved_is_refused_with_the_level_named() {
        let mut short = stripes(8, 8);
        short.pixels.truncate(short.pixels.len() - 4);
        let mut too_many = mip_chain(stripes(2, 2), 2);
        let last = too_many[1].clone();
        too_many.extend(std::iter::repeat_n(last, 31));
        let cases: [(&str, Vec<RgbaImage>, &str); 5] = [
            (
                "wrong size",
                vec![stripes(64, 64), stripes(16, 16)],
                "level 1 is 16x16, a chain from 64x64 needs 32x32",
            ),
            (
                "not shrinking",
                vec![stripes(16, 8), stripes(16, 8)],
                "level 1 is 16x8, a chain from 16x8 needs 8x4",
            ),
            (
                "short pixels",
                vec![short],
                "level 0 holds 252 bytes, 8x8 needs 256",
            ),
            (
                "empty base",
                vec![RgbaImage {
                    width: 0,
                    height: 4,
                    pixels: vec![],
                }],
                "level 0 is 0x4, a level needs at least 1x1",
            ),
            (
                "too many",
                too_many,
                "an .edds file cannot hold 33 mip levels",
            ),
        ];
        for (label, levels, message) in cases {
            let error = encode_edds(&levels).unwrap_err().to_string();

            assert_eq!(error, message, "{label}");
        }
    }

    #[test]
    fn a_full_chain_and_a_chain_cut_short_both_encode_and_round_trip() {
        for levels in [
            mip_chain(stripes(32, 16), 6),
            mip_chain(stripes(32, 16), 3),
            mip_chain(stripes(2, 2), 32),
        ] {
            let bytes = encode_edds(&levels).unwrap();

            assert_eq!(decode_edds(&bytes).unwrap(), levels);
        }
    }

    #[test]
    fn sixty_four_square_header_matches_vanilla_font_atlas() {
        let levels = mip_chain(stripes(64, 64), 7);

        let bytes = encode_edds(&levels).unwrap();

        assert_eq!(bytes.get(..128), Some(HEADER_64X64_7_LEVELS.as_slice()));
    }

    #[test]
    fn npot_round_trip_stores_large_levels_lz4_and_tiny_levels_raw() {
        let levels = mip_chain(stripes(160, 120), edds_level_count(160, 120));

        let bytes = encode_edds(&levels).unwrap();

        assert_eq!(decode_edds(&bytes).unwrap(), levels);
        let tags: Vec<&[u8]> = (0..levels.len())
            .map(|index| &bytes[0x80 + 8 * index..][..4])
            .collect();
        assert_eq!(tags.first(), Some(&b"COPY".as_slice()));
        assert_eq!(tags.last(), Some(&b"LZ4 ".as_slice()));
    }

    #[test]
    fn single_level_file_has_one_level_caps_and_round_trips() {
        let levels = vec![stripes(16, 8)];

        let bytes = encode_edds(&levels).unwrap();

        assert_eq!(u32_at(&bytes, 0x6C), 0x0000_1000);
        assert_eq!(decode_edds(&bytes).unwrap(), levels);
    }

    #[test]
    fn lz4_chunk_decodes_against_the_previous_chunk_as_dictionary() {
        let first: Vec<u8> = (0..CHUNK).map(|index| (index % 251) as u8).collect();
        let second = first.clone();
        let mut packed_second = vec![0; lz4_flex::block::get_maximum_output_size(CHUNK)];
        let packed_len =
            lz4_flex::block::compress_into_with_dict(&second, &mut packed_second, &first).unwrap();
        packed_second.truncate(packed_len);
        let mut body = Vec::new();
        push_u32(&mut body, (2 * CHUNK) as u32);
        push_chunk(&mut body, &lz4_flex::block::compress(&first), false);
        push_chunk(&mut body, &packed_second, true);

        let decoded = lz4_decode(&body, 2 * CHUNK).unwrap();

        assert!(decoded == [first, second].concat());
    }

    #[test]
    fn malformed_files_are_refused_with_a_reason() {
        let valid = encode_edds(&[stripes(16, 8)]).unwrap();
        assert_eq!(&valid[0x80..0x84], b"LZ4 ");
        type Damage = fn(&mut Vec<u8>);
        let cases: [(&str, Damage, &str); 5] = [
            (
                "a huge size claimed by a tiny body",
                |bytes| {
                    bytes[0x0C..0x10].copy_from_slice(&30_000u32.to_le_bytes());
                    bytes[0x10..0x14].copy_from_slice(&30_000u32.to_le_bytes());
                    bytes[0x88..0x8C].copy_from_slice(&3_600_000_000u32.to_le_bytes());
                },
                "LZ4 body of",
            ),
            ("wrong magic", |bytes| bytes[0] = b'X', "not an .edds file"),
            (
                "DXT5 fourcc",
                |bytes| bytes[0x54..0x58].copy_from_slice(b"DXT5"),
                "unsupported .edds pixel format",
            ),
            (
                "truncated body",
                |bytes| bytes.truncate(bytes.len() - 1),
                "short .edds file",
            ),
            (
                "chunk flag 0x81",
                |bytes| bytes[0x8F] = 0x81,
                "LZ4 chunk flag 0x81",
            ),
        ];
        for (name, damage, message) in cases {
            let mut bytes = valid.clone();
            damage(&mut bytes);
            let error = decode_edds(&bytes).unwrap_err().to_string();
            assert!(error.contains(message), "{name}: {error}");
        }
    }

    fn push_chunk(body: &mut Vec<u8>, packed: &[u8], last: bool) {
        body.extend_from_slice(&(packed.len() as u32).to_le_bytes()[..3]);
        body.push(if last { 0x80 } else { 0 });
        body.extend_from_slice(packed);
    }
}
