//! Parsing the classic DayZ PBO header (property entry + property block + file
//! entries + terminator) into a typed [`PboHeader`].

/// Record `packing` value of the leading property entry: the constant `"Vers"`
/// (little-endian on-disk bytes `sreV`).
pub const PACKING_VERS: u32 = 0x5665_7273;
/// Record `packing` value meaning the entry payload is Cprs/LZSS-compressed.
pub const PACKING_CPRS: u32 = 0x4370_7273;
/// Record `packing` value meaning the entry payload is raw (uncompressed).
pub const PACKING_RAW: u32 = 0;

/// One 20-byte file-entry record plus its asciiz name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path of the file inside the PBO, using `\` separators (engine form),
    /// e.g. `config.bin` or `1_Core\constants.c`.
    ///
    /// These are the raw on-disk bytes. Name-obfuscating packers write names
    /// that are not valid UTF-8 (6224 of them in the local corpus), and signing
    /// hashes them verbatim, so they must not be round-tripped through text.
    /// Use [`Entry::name_str`] to display one.
    pub name: Vec<u8>,
    pub packing: u32,
    /// Decompressed size (0 when `packing == PACKING_RAW`; authoritative when
    /// Cprs-compressed).
    pub original_size: u32,
    pub reserved: u32,
    pub timestamp: u32,
    pub data_size: u32,
}

impl Entry {
    /// The entry name as text, replacing bytes that are not valid UTF-8 with
    /// U+FFFD. For display and error messages only — anything that must
    /// reproduce the on-disk bytes (hashing, repacking) uses [`Entry::name`].
    pub fn name_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.name)
    }
}

/// A parsed DayZ PBO header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PboHeader {
    /// Property pairs in file order (`product`, `prefix`, `version`, ...).
    /// Ordered so a reader can round-trip them; duplicate keys are preserved as
    /// separate pairs.
    pub properties: Vec<(String, String)>,
    /// File entries in file order.
    pub entries: Vec<Entry>,
}

impl PboHeader {
    /// Value of the first property whose key matches `key` ignoring ASCII case,
    /// in file order.
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// The header runs past the bytes read so far; read more of the file and parse again.
#[derive(Debug)]
pub(crate) struct NeedsMoreBytes;

impl std::fmt::Display for NeedsMoreBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("header runs past the bytes read")
    }
}

impl std::error::Error for NeedsMoreBytes {}

/// Parse a PBO header from `bytes` (whole file). Returns the header and the
/// byte offset where the first entry's payload begins (i.e. the end of the
/// terminator entry).
pub fn parse_header(bytes: &[u8]) -> anyhow::Result<(PboHeader, usize)> {
    parse_header_bounded(bytes, bytes.len())
}

/// [`parse_header`] over a *prefix* of the file: `bytes` may stop before the
/// end of the header (the caller re-reads with a larger buffer), while
/// `file_len` stays the real on-disk length so the payload-budget guard still
/// measures against the whole file.
pub(crate) fn parse_header_bounded(
    bytes: &[u8],
    file_len: usize,
) -> anyhow::Result<(PboHeader, usize)> {
    let mut properties = Vec::new();
    let mut entries = Vec::new();
    let mut pos = 0usize;
    let mut declared_payload = 0u64;

    loop {
        let nul = bytes[pos..].iter().position(|&b| b == 0).ok_or_else(|| {
            cut_short(bytes, file_len, format!("unterminated entry name at {pos}"))
        })?;
        let name = bytes[pos..pos + nul].to_vec();
        pos += nul + 1;

        if bytes.len() < pos + 20 {
            return Err(cut_short(
                bytes,
                file_len,
                format!("truncated record at {pos}"),
            ));
        }
        let packing = u32::from_le_bytes(bytes[pos..pos + 4].try_into()?);
        let original_size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?);
        let reserved = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into()?);
        let timestamp = u32::from_le_bytes(bytes[pos + 12..pos + 16].try_into()?);
        let data_size = u32::from_le_bytes(bytes[pos + 16..pos + 20].try_into()?);
        pos += 20;

        let entry = Entry {
            name,
            packing,
            original_size,
            reserved,
            timestamp,
            data_size,
        };

        if packing == PACKING_VERS {
            // Property block follows this entry's record: asciiz key/value
            // pairs until a zero-length key.
            loop {
                let (key, after) = read_asciiz(bytes, pos, file_len)?;
                pos = after;
                if key.is_empty() {
                    break;
                }
                let (value, after) = read_asciiz(bytes, pos, file_len)?;
                pos = after;
                properties.push((key, value));
            }
        } else if entry.name.is_empty() {
            // Empty-name terminator entry (non-Vers): header ends here.
            return Ok((
                PboHeader {
                    properties,
                    entries,
                },
                pos,
            ));
        } else {
            // The header sits in front of the payload, so the entry table can
            // never claim more bytes than follow it. Checked per entry so a
            // desynced table (or one padded far past the payload) fails here
            // instead of yielding a fabricated entry list.
            declared_payload += data_size as u64;
            if declared_payload > (file_len - pos) as u64 {
                anyhow::bail!(
                    "entry `{}` at {pos} pushes the declared payload to \
                     {declared_payload} bytes with only {} left in the file",
                    String::from_utf8_lossy(&entry.name),
                    file_len - pos
                );
            }
            entries.push(entry);
        }
    }
}

/// The error for a read that ran off the end of `bytes`: a cut-short buffer
/// asks for more bytes, while a whole file that ends there is a broken header.
fn cut_short(bytes: &[u8], file_len: usize, message: String) -> anyhow::Error {
    if bytes.len() < file_len {
        NeedsMoreBytes.into()
    } else {
        anyhow::anyhow!(message)
    }
}

/// Read a NUL-terminated ASCII string starting at `pos`.
fn read_asciiz(bytes: &[u8], pos: usize, file_len: usize) -> anyhow::Result<(String, usize)> {
    let nul = bytes[pos..]
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| cut_short(bytes, file_len, format!("unterminated asciiz at {pos}")))?;
    Ok((decode_text(&bytes[pos..pos + nul]), pos + nul + 1))
}

/// Decode NUL-terminated property *text* (key or value).
///
/// Property strings are valid UTF-8 in every one of the 2581 PBOs in the local
/// corpus (only entry *names* carry raw bytes — those are kept as [`Vec<u8>`]).
/// Should a value ever be invalid UTF-8 it is decoded byte-per-char rather than
/// rejected, so a file with an exotic property still parses.
fn decode_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => raw.iter().map(|&b| b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built minimal header: property entry + property block + one file
    /// entry + terminator, then a payload region.
    fn build(bytes: &mut Vec<u8>) {
        bytes.extend(build_with(
            &[
                ("product", "dayz"),
                ("prefix", "DZ\\X"),
                ("version", "12345"),
            ],
            &[(b"config.bin", PACKING_RAW, 7, 5)],
            0,
        ));
        bytes.extend_from_slice(b"hello");
    }

    #[test]
    fn parses_properties_and_single_entry() {
        let mut bytes = Vec::new();
        build(&mut bytes);
        let (hdr, data_offset) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("product"), Some("dayz"));
        assert_eq!(hdr.property("prefix"), Some("DZ\\X"));
        assert_eq!(hdr.property("version"), Some("12345"));
        assert_eq!(hdr.entries.len(), 1);
        let e = &hdr.entries[0];
        assert_eq!(e.name_str(), "config.bin");
        assert_eq!(e.packing, PACKING_RAW);
        assert_eq!(e.timestamp, 7);
        assert_eq!(e.data_size, 5);
        // payload "hello" begins at data_offset
        assert_eq!(&bytes[data_offset..data_offset + 5], b"hello");
    }

    /// Property entry + property block from `props` + entries + terminator,
    /// then `payload_len` bytes of filler.
    fn build_with(
        props: &[(&str, &str)],
        entries: &[(&[u8], u32, u32, u32)],
        payload_len: usize,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(0);
        bytes.extend_from_slice(&PACKING_VERS.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        for (k, v) in props {
            bytes.extend_from_slice(k.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(v.as_bytes());
            bytes.push(0);
        }
        bytes.push(0);
        for (name, packing, timestamp, data_size) in entries {
            bytes.extend_from_slice(name);
            bytes.push(0);
            bytes.extend_from_slice(&packing.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 8]); // original_size, reserved
            bytes.extend_from_slice(&timestamp.to_le_bytes());
            bytes.extend_from_slice(&data_size.to_le_bytes());
        }
        bytes.push(0);
        bytes.extend_from_slice(&[0u8; 20]);
        bytes.extend(std::iter::repeat_n(0u8, payload_len));
        bytes
    }

    #[test]
    fn keeps_entry_names_that_are_not_utf8_as_raw_bytes() {
        // Obfuscating packers write raw byte names (6224 of them corpus-wide).
        // A container parser must neither reject them nor round-trip them
        // through text, which signing would silently mis-hash.
        let bytes = build_with(&[], &[(&[0xC4, 0x54, b'x'], PACKING_RAW, 0, 1)], 1);
        let (hdr, _) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.entries.len(), 1);
        assert_eq!(hdr.entries[0].name, [0xC4, 0x54, b'x']);
        assert_eq!(hdr.entries[0].name_str(), "\u{fffd}Tx");
    }

    #[test]
    fn rejects_entry_payload_larger_than_the_file() {
        // A desynced or hostile entry table declares far more payload than the
        // file can hold. Reading it must fail in the header pass, not after
        // handing back a fabricated entry list.
        let bytes = build_with(&[], &[(b"config.bin", PACKING_RAW, 0, u32::MAX)], 8);
        let err = parse_header(&bytes).unwrap_err().to_string();
        assert!(err.contains("payload"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_cumulative_payload_beyond_the_file() {
        // Each size is plausible on its own; only the running total proves the
        // header has run past the payload region into unrelated bytes.
        let entries = [(b"a".as_slice(), PACKING_RAW, 0, 40u32); 6];
        let bytes = build_with(&[], &entries, 40);
        let err = parse_header(&bytes).unwrap_err().to_string();
        assert!(err.contains("payload"), "unexpected error: {err}");
    }

    #[test]
    fn accepts_payload_that_exactly_fills_the_file() {
        // Loose bound on purpose: some writers omit the trailing SHA-1.
        let bytes = build_with(&[], &[(b"a", PACKING_RAW, 0, 40)], 40);
        let (hdr, data_offset) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.entries.len(), 1);
        assert_eq!(data_offset + 40, bytes.len());
    }

    #[test]
    fn parses_long_junk_entry_tables() {
        // Obfuscating packers pad the entry table with decoy entries (up to
        // ~10^6 in the workshop corpus); a consistent table must still parse.
        let entries: Vec<(&[u8], u32, u32, u32)> = (0..5_000)
            .map(|_| (b"junk".as_slice(), PACKING_RAW, 0, 1u32))
            .collect();
        let bytes = build_with(&[], &entries, 5_000);
        let (hdr, data_offset) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.entries.len(), 5_000);
        assert_eq!(data_offset + 5_000, bytes.len());
    }

    #[test]
    fn property_keys_match_without_regard_to_case() {
        let bytes = build_with(
            &[("Product", "dayz"), ("Prefix", "DZ\\X"), ("VERSION", "1")],
            &[],
            0,
        );
        let (hdr, _) = parse_header(&bytes).unwrap();
        let rows = [
            ("prefix", Some("DZ\\X")),
            ("PREFIX", Some("DZ\\X")),
            ("Prefix", Some("DZ\\X")),
            ("product", Some("dayz")),
            ("version", Some("1")),
            ("Version", Some("1")),
        ];
        for (asked, want) in rows {
            assert_eq!(hdr.property(asked), want, "{asked}");
        }
    }

    #[test]
    fn reads_the_first_of_a_repeated_property() {
        let bytes = build_with(
            &[
                ("prefix", "DZ\\X"),
                ("obfuscated", "a.ogg"),
                ("obfuscated", "b.rtm"),
            ],
            &[],
            0,
        );
        let (hdr, _) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("obfuscated"), Some("a.ogg"));
    }

    #[test]
    fn the_first_property_in_file_order_wins_across_a_keys_case() {
        let bytes = build_with(&[("Prefix", "a"), ("prefix", "b")], &[], 0);
        let (hdr, _) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("prefix"), Some("a"));

        let bytes = build_with(&[("prefix", "b"), ("Prefix", "a")], &[], 0);
        let (hdr, _) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("prefix"), Some("b"));
    }

    #[test]
    fn a_key_that_only_shares_a_start_is_not_matched() {
        let bytes = build_with(&[("prefixes", "a")], &[], 0);
        let (hdr, _) = parse_header(&bytes).unwrap();
        assert_eq!(hdr.property("prefix"), None);
        assert_eq!(hdr.property("prefixes"), Some("a"));
    }

    #[test]
    fn parses_multiple_entries_in_order() {
        let bytes = build_with(
            &[],
            &[
                (b"a", PACKING_RAW, 1, 1),
                (b"b\\c", PACKING_RAW, 2, 2),
                (b"d", PACKING_RAW, 3, 3),
            ],
            6,
        );
        let (hdr, data_offset) = parse_header(&bytes).unwrap();
        let names: Vec<std::borrow::Cow<str>> = hdr.entries.iter().map(|e| e.name_str()).collect();
        assert_eq!(names, ["a", "b\\c", "d"]);
        // total payload = 1+2+3 = 6 bytes, so data starts at end of header
        assert_eq!(data_offset + 6, bytes.len());
    }

    #[test]
    fn a_cut_short_buffer_asks_for_more_and_a_broken_header_does_not() {
        let full = build_with(&[("prefix", "x")], &[(b"config.bin", PACKING_RAW, 0, 4)], 4);
        let oversized = build_with(&[], &[(b"config.bin", PACKING_RAW, 0, u32::MAX)], 8);
        let rows: [(&str, &[u8], usize, bool); 3] = [
            ("cut short", &full[..10], full.len(), true),
            ("cut at file end", &full[..10], 10, false),
            ("payload", &oversized, oversized.len() * 100, false),
        ];
        for (label, bytes, file_len, needs_more) in rows {
            let err = parse_header_bounded(bytes, file_len).unwrap_err();
            assert_eq!(err.is::<NeedsMoreBytes>(), needs_more, "{label}: {err}");
        }
        let (_, header_len) = parse_header(&full).unwrap();
        for cut in 1..header_len {
            let err = parse_header_bounded(&full[..cut], full.len()).unwrap_err();
            assert!(err.is::<NeedsMoreBytes>(), "cut at {cut}: {err}");
        }
    }
}
