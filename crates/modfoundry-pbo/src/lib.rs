//! ModFoundry DayZ/Enfusion PBO container reader and deterministic writer.
//!
//! The DayZ PBO is the classic BI layout (verified): an
//! empty-name property entry whose record's `packing` field is the constant
//! `0x56657273` (`"Vers"`), followed by an asciiz `key\0value\0` property block
//! terminated by a zero-length key, then one `asciiz name + 20-byte record` file
//! entry per packed file, then an empty-name terminator. File bytes are
//! contiguous after the header in entry order.
//!
//! Parsers are pure (no IO) so they run in tests/CI/WASM. `read_pbo_file` reads the
//! whole file; `PboArchive` seeks to one entry at a time.

pub mod archive;
pub mod cprs;
pub mod header;
pub mod pack;
pub mod read;

pub use archive::PboArchive;
pub use header::{parse_header, Entry, PboHeader, PACKING_CPRS, PACKING_RAW, PACKING_VERS};
pub use pack::{pack_pbo, PackOptions, PboFileToWrite};
pub use read::{read_header_file, read_pbo, read_pbo_file, unpack_entry};
