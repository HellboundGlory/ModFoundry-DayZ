//! ModFoundry BI key/signature handling: native crypto plus the official-tool
//! adapter.
//!
//! [`native`] generates keys, signs PBOs and verifies `.bisign`s in pure Rust —
//! byte-identical to `DSCreateKey`/`DSSignFile` and accepted by
//! `DSCheckSignatures`. [`tools`] drives those official Windows binaries through
//! the `modfoundry-platform` process seam (native on Windows, Wine on Linux) and is kept as an opt-in differential oracle (`mf build
//! --tools`). [`format`] owns the on-disk containers, [`hash`] the three signed
//! SHA-1 values.

pub mod format;
pub mod hash;
pub mod native;
pub mod tools;

pub use format::{Bisign, KeyBlob};
pub use hash::{PboDigest, V2_EXCLUDED_EXTENSIONS, V3_EXTENSIONS};
pub use native::{generate_keypair, sign, sign_pbo_bytes, verify, KEY_BITS};
pub use tools::{
    authority_of_private_key, bisign_name, private_key_name, public_key_name, DsUtils, SignVersion,
};
