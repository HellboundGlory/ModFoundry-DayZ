//! Pure-Rust RSA keygen/sign/verify for BI keys, replacing the Wine adapter as
//! the default path (the [`crate::tools::DsUtils`] adapter stays as an oracle
//! and Windows-native escape hatch).

use anyhow::{bail, Context, Result};
use rand::rngs::OsRng;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};

use crate::format::{Bisign, KeyBlob};
use crate::hash::PboDigest;
use crate::tools::SignVersion;

/// SHA-1 `DigestInfo` prefix used by PKCS#1 v1.5 signatures (`30 21 30 09 06 05
/// 2b 0e 03 02 1a 05 00 04 14`).
const SHA1_DIGEST_INFO: [u8; 15] = [
    0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14,
];

/// BI keys are RSA-1024.
pub const KEY_BITS: usize = 1024;

fn sha1_padding() -> Pkcs1v15Sign {
    Pkcs1v15Sign {
        hash_len: Some(20),
        prefix: Box::from(&SHA1_DIGEST_INFO[..]),
    }
}

fn le(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_le(bytes)
}

fn le_pad(value: &BigUint, width: usize) -> Vec<u8> {
    let mut v = value.to_bytes_le();
    v.resize(width, 0);
    v
}

/// Generate a fresh keypair: the private key file and the matching public key.
pub fn generate_keypair(authority: &str, bits: usize) -> Result<(KeyBlob, KeyBlob)> {
    let key = RsaPrivateKey::new(&mut OsRng, bits).context("RSA key generation failed")?;
    let full = bits / 8;
    let half = bits / 16;
    let primes = key.primes();
    if primes.len() < 2 {
        bail!("generated key has fewer than two primes");
    }
    let (p, q) = (&primes[0], &primes[1]);
    let d = key.d();
    let pm1 = p - BigUint::from(1u32);
    let qm1 = q - BigUint::from(1u32);
    let coefficient = key
        .crt_coefficient()
        .context("failed to compute the CRT coefficient")?;

    let private = KeyBlob {
        authority: authority.to_owned(),
        private: true,
        bits: bits as u32,
        exponent: 65537,
        components: vec![
            le_pad(key.n(), full),
            le_pad(p, half),
            le_pad(q, half),
            le_pad(&(d % &pm1), half),
            le_pad(&(d % &qm1), half),
            le_pad(&coefficient, half),
            le_pad(d, full),
        ],
    };
    let public = public_of(&private);
    Ok((private, public))
}

fn private_key(blob: &KeyBlob) -> Result<RsaPrivateKey> {
    if !blob.private {
        bail!("expected a private key");
    }
    if blob.components.len() != 7 {
        bail!("private key blob has {} components", blob.components.len());
    }
    let c = &blob.components;
    RsaPrivateKey::from_components(
        le(&c[0]),
        BigUint::from(blob.exponent),
        le(&c[6]),
        vec![le(&c[1]), le(&c[2])],
    )
    .context("invalid RSA private key components")
}

fn public_key(blob: &KeyBlob) -> Result<RsaPublicKey> {
    if blob.private {
        bail!("expected a public key");
    }
    if blob.components.len() != 1 {
        bail!("public key blob has {} components", blob.components.len());
    }
    RsaPublicKey::new(le(&blob.components[0]), BigUint::from(blob.exponent))
        .context("invalid RSA public key components")
}

/// RSA-sign a PBO's content with a private key, producing a `.bisign`.
pub fn sign(private: &KeyBlob, pbo: &PboDigest, version: SignVersion) -> Result<Bisign> {
    let key = private_key(private)?;
    let public = public_of(private);
    let hashes = pbo.hashes(version);
    let mut signatures: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (slot, hash) in signatures.iter_mut().zip(hashes) {
        let be = key
            .sign(sha1_padding(), &hash)
            .context("RSA signing failed")?;
        *slot = be.iter().rev().copied().collect();
    }
    Ok(Bisign {
        key: public,
        version,
        signatures,
    })
}

/// Verify a parsed `.bisign` against a public key and the PBO content.
pub fn verify(public: &KeyBlob, pbo: &PboDigest, bisign: &Bisign) -> Result<()> {
    if bisign.key.components != public.components || bisign.key.exponent != public.exponent {
        bail!("signature was not made by this key");
    }
    let key = public_key(public)?;
    let hashes = pbo.hashes(bisign.version);
    for (i, (sig, hash)) in bisign.signatures.iter().zip(hashes).enumerate() {
        let be: Vec<u8> = sig.iter().rev().copied().collect();
        key.verify(sha1_padding(), &hash, &be)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("signature {} does not match", i + 1))?;
    }
    Ok(())
}

/// Convenience: sign packed PBO bytes with a private key file's bytes.
pub fn sign_pbo_bytes(
    private_key_file: &[u8],
    pbo: &[u8],
    version: SignVersion,
) -> Result<Vec<u8>> {
    let private = KeyBlob::parse(private_key_file)?;
    let digest = PboDigest::from_pbo_bytes(pbo)?;
    Ok(sign(&private, &digest, version)?.to_bytes())
}

fn public_of(private: &KeyBlob) -> KeyBlob {
    KeyBlob {
        authority: private.authority.clone(),
        private: false,
        bits: private.bits,
        exponent: private.exponent,
        components: vec![private.components[0].clone()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_have_cryptoapi_widths_and_round_trip_through_the_file_framing() {
        let (private, public) = generate_keypair("MFRE", KEY_BITS).unwrap();
        assert!(private.private && !public.private);
        assert_eq!(private.components[0], public.components[0]);
        assert_eq!(
            private.components.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![128, 64, 64, 64, 64, 64, 128]
        );
        assert_eq!(public.components[0].len(), 128);
        private_key(&private).unwrap();
        public_key(&public).unwrap();
        assert_eq!(KeyBlob::parse(&private.to_bytes()).unwrap(), private);
        assert_eq!(KeyBlob::parse(&public.to_bytes()).unwrap(), public);
    }

    #[test]
    fn sign_then_verify_accepts_the_signature_and_rejects_tampering() {
        let (private, public) = generate_keypair("MFRE", KEY_BITS).unwrap();
        let digest = super::super::hash::tests_support::digest(
            &[("Scripts\\a.c", b"AAAA"), ("config.cpp", b"BBBB")],
            None,
        );
        let bisign = sign(&private, &digest, SignVersion::V3).unwrap();
        verify(&public, &digest, &bisign).unwrap();

        let mut tampered = digest.clone();
        tampered.entries[0].1 = b"XXXX".to_vec();
        assert!(verify(&public, &tampered, &bisign).is_err());
    }

    #[test]
    fn v2_and_v3_signatures_differ() {
        let (private, public) = generate_keypair("MFRE", KEY_BITS).unwrap();
        let digest = super::super::hash::tests_support::digest(&[("config.cpp", b"BBBB")], None);
        let v2 = sign(&private, &digest, SignVersion::V2).unwrap();
        let v3 = sign(&private, &digest, SignVersion::V3).unwrap();
        assert_ne!(v2.signatures[2], v3.signatures[2]);
        verify(&public, &digest, &v2).unwrap();
        verify(&public, &digest, &v3).unwrap();
    }
}
