//! Byte-parity against the official `DSCreateKey`/`DSSignFile` output.
//!
//! Fixtures under `tests/data/re/` are throwaway RE artifacts generated with the
//! DayZ Tools on 2026-09-10 (a synthetic PBO, its v2/v3 `.bisign`, and the
//! 1024-bit key pair that made them). They match our native implementation
//! byte-for-byte.

use modfoundry_sign::{format::Bisign, hash::PboDigest, native, KeyBlob, SignVersion};
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "data", "re", name]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn key_file_framing_round_trips_dsutils_output() {
    let priv_bytes = fixture("MFRE.biprivatekey");
    let public_bytes = fixture("MFRE.bikey");
    let private = KeyBlob::parse(&priv_bytes).unwrap();
    let public = KeyBlob::parse(&public_bytes).unwrap();
    assert_eq!(private.to_bytes(), priv_bytes);
    assert_eq!(public.to_bytes(), public_bytes);
    assert!(private.private && !public.private);
    assert_eq!(private.bits, 1024);
    assert_eq!(private.components[0], public.components[0]);
}

#[test]
fn native_v3_signature_is_byte_identical_to_dsutils() {
    let pbo = fixture("tiny.pbo");
    let expected = fixture("tiny.pbo.MFRE.bisign");
    let private = KeyBlob::parse(&fixture("MFRE.biprivatekey")).unwrap();
    let digest = PboDigest::from_pbo_bytes(&pbo).unwrap();
    let actual = native::sign(&private, &digest, SignVersion::V3).unwrap();
    assert_eq!(actual.to_bytes(), expected);
}

#[test]
fn native_v2_signature_is_byte_identical_to_dsutils() {
    let pbo = fixture("tinyv2.pbo");
    let expected = fixture("tinyv2.pbo.MFRE.bisign");
    let private = KeyBlob::parse(&fixture("MFRE.biprivatekey")).unwrap();
    let digest = PboDigest::from_pbo_bytes(&pbo).unwrap();
    let actual = native::sign(&private, &digest, SignVersion::V2).unwrap();
    assert_eq!(actual.to_bytes(), expected);
}

#[test]
fn dsutils_signature_verifies_against_the_public_key() {
    let digest = PboDigest::from_pbo_bytes(&fixture("tiny.pbo")).unwrap();
    let public = KeyBlob::parse(&fixture("MFRE.bikey")).unwrap();
    let bisign = Bisign::parse(&fixture("tiny.pbo.MFRE.bisign")).unwrap();
    assert_eq!(bisign.version, SignVersion::V3);
    native::verify(&public, &digest, &bisign).unwrap();
}

#[test]
fn tampered_pbo_fails_verification() {
    let mut pbo = fixture("tiny.pbo");
    let at = pbo
        .windows(8)
        .position(|w| w == b"void Foo")
        .expect("script payload present");
    pbo[at] ^= 0xFF;
    let digest = PboDigest::from_pbo_bytes(&pbo).unwrap();
    let public = KeyBlob::parse(&fixture("MFRE.bikey")).unwrap();
    let bisign = Bisign::parse(&fixture("tiny.pbo.MFRE.bisign")).unwrap();
    assert!(native::verify(&public, &digest, &bisign).is_err());
}
