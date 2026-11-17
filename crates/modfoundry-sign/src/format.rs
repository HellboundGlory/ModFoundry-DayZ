//! Binary layout of BI key files and signatures (`.bikey`, `.biprivatekey`,
//! `.bisign`), reimplemented natively from byte-level RE.

use anyhow::{bail, Context, Result};

use crate::tools::SignVersion;

pub const BLOB_PUBLIC: u8 = 0x06;
pub const BLOB_PRIVATE: u8 = 0x07;
pub const BLOB_VERSION: u8 = 0x02;
pub const CALG_RSA_SIGN: u32 = 0x2400;
pub const MAGIC_PUBLIC: &[u8; 4] = b"RSA1";
pub const MAGIC_PRIVATE: &[u8; 4] = b"RSA2";

/// One BI key (public or private) as a CryptoAPI blob plus its authority name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBlob {
    pub authority: String,
    pub private: bool,
    pub bits: u32,
    pub exponent: u32,
    /// CryptoAPI integers, little-endian fixed width, in blob order:
    /// `[modulus]` for public; `[modulus, p, q, dmp1, dmq1, iqmp, d]` for private.
    pub components: Vec<Vec<u8>>,
}

impl KeyBlob {
    /// Parse a complete `.bikey`/`.biprivatekey` file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (key, used) = Self::read_at(bytes, 0)?;
        if used != bytes.len() {
            bail!("trailing {} bytes after key blob", bytes.len() - used);
        }
        Ok(key)
    }

    /// Parse a key starting at `pos`; returns the key and the byte offset just
    /// past it (a `.bisign` embeds one at its start).
    pub fn read_at(bytes: &[u8], pos: usize) -> Result<(Self, usize)> {
        let (authority, mut off) = read_cstring(bytes, pos)?;
        let blob_len = read_u32(bytes, off)? as usize;
        off += 4;
        let end = off
            .checked_add(blob_len)
            .filter(|e| *e <= bytes.len())
            .with_context(|| format!("key blob length {blob_len} runs past end of file"))?;
        let blob = &bytes[off..end];
        off = end;

        if blob.len() < 20 {
            bail!("key blob too short ({} bytes)", blob.len());
        }
        let magic = &blob[8..12];
        let private = match blob[0] {
            BLOB_PUBLIC if magic == MAGIC_PUBLIC => false,
            BLOB_PRIVATE if magic == MAGIC_PRIVATE => true,
            other => bail!("unrecognized key blob: type {other:#x}, magic {magic:?}"),
        };
        let bits = read_u32(blob, 12)?;
        let exponent = read_u32(blob, 16)?;
        if bits == 0 || bits % 16 != 0 {
            bail!("unsupported key size {bits}");
        }

        let full = (bits / 8) as usize;
        let half = (bits / 16) as usize;
        let widths: &[usize] = if private {
            &[full, half, half, half, half, half, full]
        } else {
            &[full]
        };
        let mut components = Vec::with_capacity(widths.len());
        let mut p = 20;
        for w in widths {
            let end = p + w;
            if end > blob.len() {
                bail!("key blob truncated at component {}", components.len());
            }
            components.push(blob[p..end].to_vec());
            p = end;
        }
        if p != blob.len() {
            bail!("key blob has {} trailing bytes", blob.len() - p);
        }

        Ok((
            KeyBlob {
                authority,
                private,
                bits,
                exponent,
                components,
            },
            off,
        ))
    }

    /// Serialize to the on-disk file framing `<authority>\0 + u32 len + blob`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let blob = self.blob_bytes();
        let mut out = Vec::with_capacity(self.authority.len() + 5 + blob.len());
        out.extend_from_slice(self.authority.as_bytes());
        out.push(0);
        out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&blob);
        out
    }

    fn blob_bytes(&self) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.push(if self.private {
            BLOB_PRIVATE
        } else {
            BLOB_PUBLIC
        });
        blob.push(BLOB_VERSION);
        blob.extend_from_slice(&0u16.to_le_bytes());
        blob.extend_from_slice(&CALG_RSA_SIGN.to_le_bytes());
        blob.extend_from_slice(if self.private {
            MAGIC_PRIVATE
        } else {
            MAGIC_PUBLIC
        });
        blob.extend_from_slice(&self.bits.to_le_bytes());
        blob.extend_from_slice(&self.exponent.to_le_bytes());
        for c in &self.components {
            blob.extend_from_slice(c);
        }
        blob
    }
}

/// A parsed `.bisign`: the embedded public key, the version, and the three raw
/// little-endian RSA signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bisign {
    pub key: KeyBlob,
    pub version: SignVersion,
    pub signatures: [Vec<u8>; 3],
}

impl Bisign {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (key, mut off) = KeyBlob::read_at(bytes, 0)?;
        if key.private {
            bail!("bisign embeds a private key blob");
        }
        let mut sigs: Vec<Vec<u8>> = Vec::with_capacity(3);
        let mut version = None;
        for i in 0..3 {
            if i == 1 {
                let v = read_u32(bytes, off)?;
                off += 4;
                version = Some(
                    SignVersion::from_number(v)
                        .with_context(|| format!("unknown bisign version {v}"))?,
                );
            }
            let len = read_u32(bytes, off)? as usize;
            off += 4;
            let end = off
                .checked_add(len)
                .filter(|e| *e <= bytes.len())
                .with_context(|| format!("signature {i} runs past end of file"))?;
            sigs.push(bytes[off..end].to_vec());
            off = end;
        }
        if off != bytes.len() {
            bail!("trailing {} bytes after bisign", bytes.len() - off);
        }
        let [s1, s2, s3] = <[Vec<u8>; 3]>::try_from(sigs).expect("three signatures");
        Ok(Bisign {
            key,
            version: version.expect("version read"),
            signatures: [s1, s2, s3],
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.key.to_bytes();
        for (i, sig) in self.signatures.iter().enumerate() {
            if i == 1 {
                out.extend_from_slice(&self.version.number().to_le_bytes());
            }
            out.extend_from_slice(&(sig.len() as u32).to_le_bytes());
            out.extend_from_slice(sig);
        }
        out
    }
}

fn read_u32(bytes: &[u8], pos: usize) -> Result<u32> {
    let end = pos + 4;
    if end > bytes.len() {
        bail!("unexpected end of data reading u32 at {pos}");
    }
    Ok(u32::from_le_bytes(bytes[pos..end].try_into()?))
}

fn read_cstring(bytes: &[u8], pos: usize) -> Result<(String, usize)> {
    let rel = bytes[pos..]
        .iter()
        .position(|b| *b == 0)
        .with_context(|| format!("unterminated string at {pos}"))?;
    let s = String::from_utf8(bytes[pos..pos + rel].to_vec()).context("non-UTF-8 string")?;
    Ok((s, pos + rel + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_blob(authority: &str, bits: u32, fill: u8) -> KeyBlob {
        KeyBlob {
            authority: authority.to_owned(),
            private: false,
            bits,
            exponent: 65537,
            components: vec![vec![fill; (bits / 8) as usize]],
        }
    }

    fn private_blob(authority: &str, bits: u32) -> KeyBlob {
        let full = (bits / 8) as usize;
        let half = (bits / 16) as usize;
        KeyBlob {
            authority: authority.to_owned(),
            private: true,
            bits,
            exponent: 65537,
            components: vec![
                vec![1; full],
                vec![2; half],
                vec![3; half],
                vec![4; half],
                vec![5; half],
                vec![6; half],
                vec![7; full],
            ],
        }
    }

    #[test]
    fn public_key_round_trips_through_bytes() {
        let key = public_blob("dayz", 1024, 0xAB);
        let bytes = key.to_bytes();
        assert_eq!(bytes.len(), 5 + 4 + 148);
        assert_eq!(&bytes[..5], b"dayz\0");
        assert_eq!(KeyBlob::parse(&bytes).unwrap(), key);
    }

    #[test]
    fn private_key_components_have_cryptoapi_widths() {
        let key = private_blob("MFRE", 1024);
        let parsed = KeyBlob::parse(&key.to_bytes()).unwrap();
        assert!(parsed.private);
        assert_eq!(
            parsed.components.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![128, 64, 64, 64, 64, 64, 128]
        );
        assert_eq!(parsed, key);
    }

    #[test]
    fn rejects_blob_of_wrong_type() {
        let mut bytes = public_blob("x", 1024, 0).to_bytes();
        bytes[6] = BLOB_PRIVATE;
        assert!(KeyBlob::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = public_blob("x", 1024, 0).to_bytes();
        bytes.push(0);
        assert!(KeyBlob::parse(&bytes).is_err());
    }

    #[test]
    fn bisign_round_trips_with_embedded_public_key() {
        let bisign = Bisign {
            key: public_blob("MFRE", 1024, 0x5A),
            version: SignVersion::V3,
            signatures: [vec![0x11; 128], vec![0x22; 128], vec![0x33; 128]],
        };
        let bytes = bisign.to_bytes();
        assert_eq!(bytes.len(), 557);
        assert_eq!(Bisign::parse(&bytes).unwrap(), bisign);
    }

    #[test]
    fn bisign_parses_v2_version_field() {
        let bisign = Bisign {
            key: public_blob("MFRE", 1024, 0),
            version: SignVersion::V2,
            signatures: [vec![0; 128], vec![0; 128], vec![0; 128]],
        };
        let parsed = Bisign::parse(&bisign.to_bytes()).unwrap();
        assert_eq!(parsed.version, SignVersion::V2);
    }

    #[test]
    fn bisign_rejects_unknown_version() {
        let mut bytes = Bisign {
            key: public_blob("MFRE", 1024, 0),
            version: SignVersion::V3,
            signatures: [vec![0; 128], vec![0; 128], vec![0; 128]],
        }
        .to_bytes();
        let vpos = bytes.len() - 4 - 128 - 4 - 128 - 4;
        bytes[vpos..vpos + 4].copy_from_slice(&9u32.to_le_bytes());
        assert!(Bisign::parse(&bytes).is_err());
    }
}
