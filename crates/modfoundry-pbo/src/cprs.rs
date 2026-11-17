//! Cprs/LZSS decompression for PBO entry payloads.
//!
//! Algorithm is BI's `unpack_data` (BIKI "Compressed LZSS File Format",
//! `ExpandPbo` variant, transcribed and reimplemented here): each block is
//! preceded by a flag byte; for each of its 8 bits a set bit means one literal
//! byte follows, a clear bit means a 2-byte back-reference pointer. The pointer
//! is `AAAA LLLL AAAAAAAA`: low 8 bits of the 12-bit window offset in the first
//! byte, offset high nibble in the top nibble of the second byte, and
//! `length = (second & 0x0F) + 3`. The last 4 bytes of the packed payload are
//! an unsigned additive checksum over the decompressed output.

use anyhow::Result;

/// Space filler used when a back-reference points before the start of output.
const SPACE_FILLER: u8 = 0x20;

/// Decompress a Cprs-packed PBO entry payload.
///
/// `packed` is the entry's `data_size` bytes (including the trailing 4-byte
/// checksum); `expected` is the decompressed size from the record's
/// `original_size`. Returns the decompressed bytes, validating that the output
/// length equals `expected` and that the trailing checksum matches.
pub fn decompress(packed: &[u8], expected: usize) -> Result<Vec<u8>> {
    if packed.len() < 5 {
        anyhow::bail!("compressed payload too short ({} bytes)", packed.len());
    }
    let read_checksum = u32::from_le_bytes(packed[packed.len() - 4..].try_into().unwrap());
    let data = &packed[..packed.len() - 4];

    // A flag group is 17 bytes and expands to at most 144, so the packed length bounds the output.
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(data.len().saturating_mul(9)));
    let mut i = 0usize;
    while i < data.len() {
        let flag = data[i];
        i += 1;
        let mut bit = 1u8;
        // Up to 8 tokens per flag byte; stop early if the stream ends.
        while bit != 0 && i < data.len() {
            if out.len() >= expected {
                anyhow::bail!("decompressed past {expected} bytes");
            }
            if flag & bit != 0 {
                // Literal byte.
                out.push(data[i]);
                i += 1;
            } else {
                // Back-reference pointer: 2 bytes.
                if i + 2 > data.len() {
                    anyhow::bail!("truncated back-reference pointer at {i}");
                }
                let b1 = data[i] as usize;
                let b2 = data[i + 1] as usize;
                i += 2;
                let length = (b2 & 0x0F) + 3;
                let offset = b1 | ((b2 & 0xF0) << 4);
                if offset == 0 {
                    anyhow::bail!("zero-offset back-reference at {}", i - 2);
                }
                let start = out.len() as isize - offset as isize;
                for k in 0..length {
                    let src = start + k as isize;
                    if src < 0 {
                        out.push(SPACE_FILLER);
                    } else {
                        let byte = out[src as usize];
                        out.push(byte);
                    }
                }
            }
            bit <<= 1;
        }
    }

    if out.len() != expected {
        anyhow::bail!("decompressed to {} bytes, expected {expected}", out.len());
    }
    let calc: u32 = out.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
    if calc != read_checksum {
        anyhow::bail!("Cprs checksum mismatch: got {calc:#x}, file {read_checksum:#x}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All-literal stream: flag 0xFF (8 literals) then bytes, checksum appended.
    fn literal_stream(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut chunk = bytes;
        while !chunk.is_empty() {
            let n = chunk.len().min(8);
            // flag with the low `n` bits set
            let flag: u8 = if n == 8 { 0xFF } else { (1 << n) - 1 };
            out.push(flag);
            out.extend_from_slice(&chunk[..n]);
            chunk = &chunk[n..];
        }
        let sum: u32 = bytes.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32));
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    #[test]
    fn decompresses_all_literal_stream() {
        let src: Vec<u8> = (0u8..=255).cycle().take(512).collect();
        let packed = literal_stream(&src);
        assert_eq!(decompress(&packed, src.len()).unwrap(), src);
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let src = b"hello world, this is a longer string!".to_vec();
        let mut packed = literal_stream(&src);
        let n = packed.len();
        packed[n - 1] ^= 0xFF; // corrupt checksum
        let err = decompress(&packed, src.len()).unwrap_err();
        assert!(err.to_string().contains("checksum"), "unexpected: {err}");
    }

    #[test]
    fn rejects_length_mismatch() {
        let src = b"definitely more than eight bytes here".to_vec();
        let packed = literal_stream(&src);
        let err = decompress(&packed, src.len() + 5).unwrap_err();
        assert!(err.to_string().contains("expected"), "unexpected: {err}");
    }

    #[test]
    fn back_reference_repeats_and_overlaps() {
        // One flag byte (0x07 = 3 literals) then a back-reference pointer that
        // overlaps its own source: length 5 > offset 3, copying while the output
        // grows. "abc" then the ref appends "abcab" => "abcabcab".
        let mut packed: Vec<u8> = Vec::new();
        packed.push(0x07);
        packed.extend_from_slice(b"abc");
        packed.push(3);
        packed.push(2); // (offset high nibble << 4) | (length-3); offset 3, length 5
        let out = b"abcabcab";
        let sum: u32 = out.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32));
        packed.extend_from_slice(&sum.to_le_bytes());
        assert_eq!(decompress(&packed, 8).unwrap(), b"abcabcab");
    }

    #[test]
    fn a_zero_offset_back_reference_is_an_error() {
        let mut packed: Vec<u8> = vec![0x00, 0x00, 0x00];
        packed.extend_from_slice(&0u32.to_le_bytes());
        let err = decompress(&packed, 3).unwrap_err();
        assert!(
            err.to_string().contains("zero-offset back-reference at 1"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn space_filler_pads_early_back_reference() {
        // First token is a pointer to offset far beyond produced output.
        // offset high enough that start < 0 entirely => pad spaces.
        let mut packed: Vec<u8> = Vec::new();
        packed.push(0x00); // flag: all pointers
                           // offset 0x0F (15) with length 4 => start = 0-15 <0 -> 4 spaces
        let offset = 15usize; // 0x0F
        let length_minus3 = 1u8; // length 4
        packed.push(offset as u8); // low 8 bits
        packed.push(((offset >> 8) as u8) << 4 | length_minus3); // high nibble offset + len-3
        let sum = 4u32 * (SPACE_FILLER as u32);
        packed.extend_from_slice(&sum.to_le_bytes());
        assert_eq!(decompress(&packed, 4).unwrap(), vec![SPACE_FILLER; 4]);
    }
}
