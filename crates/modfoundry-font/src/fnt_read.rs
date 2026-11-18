//! Reads a DayZ `.fnt` file (FNT3 bitmap, FNT5 distance field) into glyph rectangles and metrics.

use anyhow::{bail, ensure, Context, Result};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_GLYPHS: u32 = 70_000;
const MAX_CODE_POINT: u32 = 0x10FFFF;
const MAX_NAME_BYTES: usize = 255;
const FORM_HEADER_BYTES: usize = 12;
const CHUNK_HEADER_BYTES: usize = 8;
const BITMAP_RECORD_BYTES: usize = 12;
const SDF_RECORD_BYTES: usize = 14;
const SPACE: u32 = 0x20;
const BOX: u32 = 0x25A1;

/// The font format version from the FORM header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FntVersion {
    Fnt3,
    Fnt5,
}

/// A parsed `.fnt` file.
#[derive(Debug, Clone, PartialEq)]
pub struct FntFont {
    pub version: FntVersion,
    pub name: String,
    pub size: u32,
    pub sdf: Option<SdfInfo>,
    pub glyphs: Vec<FntGlyph>,
    pub kerning: Vec<Kerning>,
}

/// Distance-field metrics from an FNT5 HEAD chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct SdfInfo {
    pub multi_channel: bool,
    pub cell: u32,
    pub cap_height: f32,
    pub em: f32,
    pub range: i16,
    pub bold: bool,
    pub italic: bool,
}

/// One glyph rectangle in the atlas with its bearings and advance, in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct FntGlyph {
    pub codepoint: u32,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub bearing_x: i16,
    /// Baseline to ink top, positive up. Only FNT5 distance-field glyphs have it.
    pub bearing_y: Option<i16>,
    pub advance: i16,
}

/// A kerning pair; `amount` is negative to pull the glyphs together.
#[derive(Debug, Clone, PartialEq)]
pub struct Kerning {
    pub left: u32,
    pub right: u32,
    pub amount: i32,
}

/// Parses the bytes of a `.fnt` file. The texture is not read.
pub fn parse_fnt(bytes: &[u8]) -> Result<FntFont> {
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "the font file is {} bytes, over the 16 MiB limit",
        bytes.len()
    );
    let Some(header) = bytes.get(..FORM_HEADER_BYTES) else {
        bail!(
            "the font file is {} bytes, too short for a FORM header",
            bytes.len()
        );
    };
    ensure!(
        &header[..4] == b"FORM",
        "the font file does not start with FORM"
    );
    let version = read_version(&header[8..12])?;
    let chunks = split_chunks(bytes)?;

    let (name, size, sdf) = read_head(required_chunk(&chunks, b"HEAD")?, version)?;
    let (glyph_count, ranges) = read_glps(required_chunk(&chunks, b"GLPS")?, version)?;

    let tcrd = required_chunk(&chunks, b"TCRD")?;
    let record_bytes = if sdf.is_some() {
        SDF_RECORD_BYTES
    } else {
        BITMAP_RECORD_BYTES
    };
    ensure!(
        tcrd.payload.len() % record_bytes == 0,
        "the TCRD chunk at byte {} is {} bytes, not a multiple of {record_bytes}",
        tcrd.offset,
        tcrd.payload.len()
    );
    let entries = (tcrd.payload.len() / record_bytes) as u64;
    let indices = index_count(&ranges);
    ensure!(
        u64::from(glyph_count) == indices && indices == entries,
        "GLPS declares {glyph_count} glyphs, its ranges give {indices}, TCRD has {entries}"
    );
    let glyphs = read_glyphs(tcrd, sdf.is_some(), &code_points(&ranges))?;

    let kerning = match find_chunk(&chunks, b"KERN") {
        Some(kern) => read_kerning(kern)?,
        None => Vec::new(),
    };
    Ok(FntFont {
        version,
        name,
        size,
        sdf,
        glyphs,
        kerning,
    })
}

impl FntFont {
    /// The glyph for exactly this code point.
    pub fn glyph(&self, codepoint: u32) -> Option<&FntGlyph> {
        self.glyphs
            .iter()
            .find(|glyph| glyph.codepoint == codepoint)
    }

    /// The glyph for this code point, else U+25A1, else U+0020.
    pub fn glyph_or_fallback(&self, codepoint: u32) -> Option<&FntGlyph> {
        self.glyph(codepoint)
            .or_else(|| self.glyph(BOX))
            .or_else(|| self.glyph(SPACE))
    }

    /// The kerning amount for a pair, 0 when the font has none.
    pub fn kern(&self, left: u32, right: u32) -> i32 {
        match self
            .kerning
            .binary_search_by_key(&(left, right), |pair| (pair.left, pair.right))
        {
            Ok(index) => self.kerning[index].amount,
            Err(_) => 0,
        }
    }

    /// Line height in pixels: the distance-field em, else glyph 0's height.
    pub fn line_height(&self) -> f32 {
        match &self.sdf {
            Some(sdf) => sdf.em,
            None => self.glyphs.first().map_or(0.0, |glyph| f32::from(glyph.h)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Chunk<'a> {
    tag: [u8; 4],
    offset: usize,
    payload: &'a [u8],
}

fn read_version(tag: &[u8]) -> Result<FntVersion> {
    match tag {
        b"FNT3" => Ok(FntVersion::Fnt3),
        b"FNT5" => Ok(FntVersion::Fnt5),
        other => bail!(
            "the font version {} is not read; only FNT3 and FNT5 are",
            String::from_utf8_lossy(other)
        ),
    }
}

fn split_chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let mut chunks = Vec::new();
    let mut offset = FORM_HEADER_BYTES;
    while offset < bytes.len() {
        let Some(header) = bytes.get(offset..offset.saturating_add(CHUNK_HEADER_BYTES)) else {
            bail!("a chunk header at byte {offset} is cut short");
        };
        let tag: [u8; 4] = header[..4].try_into()?;
        let length = u32::from_be_bytes(header[4..].try_into()?) as usize;
        let start = offset + CHUNK_HEADER_BYTES;
        let Some(payload) = bytes.get(start..start.saturating_add(length)) else {
            bail!(
                "the {} chunk at byte {offset} claims {length} bytes, but the file ends at byte {}",
                String::from_utf8_lossy(&tag),
                bytes.len()
            );
        };
        chunks.push(Chunk {
            tag,
            offset,
            payload,
        });
        offset = start + length;
    }
    Ok(chunks)
}

fn find_chunk<'a>(chunks: &[Chunk<'a>], tag: &[u8; 4]) -> Option<Chunk<'a>> {
    chunks.iter().find(|chunk| chunk.tag == *tag).copied()
}

fn required_chunk<'a>(chunks: &[Chunk<'a>], tag: &[u8; 4]) -> Result<Chunk<'a>> {
    find_chunk(chunks, tag)
        .with_context(|| format!("the font has no {} chunk", String::from_utf8_lossy(tag)))
}

fn read_head(chunk: Chunk<'_>, version: FntVersion) -> Result<(String, u32, Option<SdfInfo>)> {
    let mut reader = Reader::new(chunk);
    let name_length = reader.u32()? as usize;
    let name = reader.take(name_length)?;
    let name = &name[..name.len().min(MAX_NAME_BYTES)];
    let name = name.split(|&byte| byte == 0).next().unwrap_or_default();
    let name = String::from_utf8_lossy(name).into_owned();
    let size = reader.u32()?;
    reader.u32()?;
    let sdf = match version {
        FntVersion::Fnt3 => None,
        FntVersion::Fnt5 => read_sdf(&mut reader)?,
    };
    Ok((name, size, sdf))
}

fn read_sdf(reader: &mut Reader<'_>) -> Result<Option<SdfInfo>> {
    let sdf_type = reader.u8()?;
    let cell = reader.u32()?;
    let cap_height = reader.f32()?;
    let em = reader.f32()?;
    let range = reader.i16()?;
    let bold = reader.u8()? != 0;
    let italic = reader.u8()? != 0;
    reader.f32()?;
    let multi_channel = match sdf_type {
        0 => return Ok(None),
        1 => false,
        2 => true,
        other => bail!("sdfType {other} is not 0, 1 or 2"),
    };
    Ok(Some(SdfInfo {
        multi_channel,
        cell,
        cap_height,
        em,
        range,
        bold,
        italic,
    }))
}

fn read_glps(chunk: Chunk<'_>, version: FntVersion) -> Result<(u32, Vec<(u32, u16)>)> {
    let mut reader = Reader::new(chunk);
    reader.take(8)?;
    let glyph_count = reader.u32()?;
    let range_count = reader.u32()?;
    ensure!(
        glyph_count <= MAX_GLYPHS,
        "the font declares {glyph_count} glyphs, over the {MAX_GLYPHS} limit"
    );
    let ranges = (0..range_count)
        .map(|_| read_range(&mut reader, version))
        .collect::<Result<Vec<_>>>()?;
    Ok((glyph_count, ranges))
}

fn read_range(reader: &mut Reader<'_>, version: FntVersion) -> Result<(u32, u16)> {
    let (first, count) = match version {
        FntVersion::Fnt3 => (u32::from(reader.u16()?), reader.u16()?),
        FntVersion::Fnt5 => {
            let first = reader.u32()?;
            let count = reader.u16()?;
            reader.u16()?;
            (first, count)
        }
    };
    ensure!(
        first <= MAX_CODE_POINT,
        "a GLPS range starts at U+{first:04X}, past U+10FFFF"
    );
    Ok((first, count))
}

/// Glyph indices the ranges produce: code points below U+0020 take no index.
fn index_count(ranges: &[(u32, u16)]) -> u64 {
    ranges
        .iter()
        .map(|&(first, count)| {
            let below_space = SPACE.saturating_sub(first).min(u32::from(count));
            u64::from(count) - u64::from(below_space)
        })
        .sum()
}

fn code_points(ranges: &[(u32, u16)]) -> Vec<u32> {
    ranges
        .iter()
        .flat_map(|&(first, count)| first..first + u32::from(count))
        .filter(|&codepoint| codepoint >= SPACE)
        .collect()
}

fn read_glyphs(chunk: Chunk<'_>, sdf: bool, codepoints: &[u32]) -> Result<Vec<FntGlyph>> {
    let mut reader = Reader::new(chunk);
    codepoints
        .iter()
        .map(|&codepoint| read_glyph(&mut reader, codepoint, sdf))
        .collect()
}

fn read_glyph(reader: &mut Reader<'_>, codepoint: u32, sdf: bool) -> Result<FntGlyph> {
    let x = reader.u16()?;
    let y = reader.u16()?;
    let w = reader.u16()?;
    let h = reader.u16()?;
    let (bearing_x, bearing_y, advance) = if sdf {
        let bearing_x = reader.i16()?;
        let bearing_y = reader.i16()?;
        let advance = reader.i16()?;
        (bearing_x, Some(bearing_y), advance)
    } else {
        let left = reader.i16()?;
        let right = reader.i16()?;
        let advance = i32::from(left) + i32::from(w) + i32::from(right);
        let advance = i16::try_from(advance)
            .with_context(|| format!("the advance of U+{codepoint:04X} does not fit 16 bits"))?;
        (left, None, advance)
    };
    Ok(FntGlyph {
        codepoint,
        x,
        y,
        w,
        h,
        bearing_x,
        bearing_y,
        advance,
    })
}

fn read_kerning(chunk: Chunk<'_>) -> Result<Vec<Kerning>> {
    let mut reader = Reader::new(chunk);
    let mut pairs = Vec::new();
    while reader.remaining() > 0 {
        let left = u32::from(reader.u16()?);
        let right = u32::from(reader.u16()?);
        let amount = reader.i32()?;
        pairs.push(Kerning {
            left,
            right,
            amount,
        });
    }
    // Sorted so kern can binary search a file whose pairs are out of order.
    pairs.sort_by_key(|pair| (pair.left, pair.right));
    Ok(pairs)
}

struct Reader<'a> {
    chunk: Chunk<'a>,
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(chunk: Chunk<'a>) -> Self {
        Self { chunk, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.chunk.payload.len() - self.pos
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let payload = self.chunk.payload;
        let Some(bytes) = payload[self.pos..].get(..len) else {
            bail!(
                "the {} chunk is cut short at byte {}",
                String::from_utf8_lossy(&self.chunk.tag),
                self.chunk.offset + CHUNK_HEADER_BYTES + self.pos
            );
        };
        self.pos += len;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
}
