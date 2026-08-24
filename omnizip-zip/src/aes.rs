//! WinZip AES encryption (TODO.containers task 05) — AE-1/AE-2 per
//! the WinZip AES APPNOTE: PBKDF2-HMAC-SHA1 (1000 iterations) key
//! schedule, AES-CTR with a 16-byte big-endian counter starting at 1,
//! and a 10-byte HMAC-SHA1 authenticator. Entries carry compression
//! method 99 with the real method + strength in extra field 0x9901.
//!
//! Determinism: the salt is *derived* (SHA-256 of password ‖ name ‖
//! content CRC) rather than random, so the same input + options still
//! produce a byte-identical archive. Readers do not care where a salt
//! came from; reproducible archives do.
#![forbid(unsafe_code)]

use omnizip_archive_core::ArchiveError;

/// Extra-field id for the WinZip AES extension.
pub const AES_EXTRA_TAG: u16 = 0x9901;
/// Pseudo compression method marking WinZip-AES entries.
pub const METHOD_AES: u16 = 99;
/// Version needed to extract for AES entries (5.1).
pub const VERSION_AES: u16 = 51;

/// AES key strength (also selects the salt length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AesStrength {
    Aes128,
    Aes192,
    Aes256,
}

impl AesStrength {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Aes128 => 1,
            Self::Aes192 => 2,
            Self::Aes256 => 3,
        }
    }

    #[must_use]
    pub const fn key_len(self) -> usize {
        match self {
            Self::Aes128 => 16,
            Self::Aes192 => 24,
            Self::Aes256 => 32,
        }
    }

    #[must_use]
    pub const fn salt_len(self) -> usize {
        match self {
            Self::Aes128 => 8,
            Self::Aes192 => 12,
            Self::Aes256 => 16,
        }
    }

    fn from_code(code: u8) -> Result<Self, ArchiveError> {
        match code {
            1 => Ok(Self::Aes128),
            2 => Ok(Self::Aes192),
            3 => Ok(Self::Aes256),
            other => Err(ArchiveError::InvalidArchive(format!(
                "WinZip AES strength {other} out of range"
            ))),
        }
    }
}

/// The parsed 0x9901 extra field.
#[derive(Clone, Copy, Debug)]
pub struct AesInfo {
    /// 1 = AE-1 (keeps CRC32), 2 = AE-2 (CRC forced to 0).
    pub version: u16,
    pub strength: AesStrength,
    /// The real compression method (8 deflate, 0 store, …).
    pub real_method: u16,
}

/// Parse the 7-byte 0x9901 body.
pub fn parse_extra(body: &[u8]) -> Result<AesInfo, ArchiveError> {
    if body.len() < 7 {
        return Err(ArchiveError::InvalidArchive(
            "WinZip AES extra field truncated".into(),
        ));
    }
    let version = u16::from_le_bytes([body[0], body[1]]);
    if version != 1 && version != 2 {
        return Err(ArchiveError::InvalidArchive(format!(
            "WinZip AES header version {version} unsupported"
        )));
    }
    if &body[2..4] != b"AE" {
        return Err(ArchiveError::InvalidArchive(
            "WinZip AES extra field has a non-AE vendor id".into(),
        ));
    }
    Ok(AesInfo {
        version,
        strength: AesStrength::from_code(body[4])?,
        real_method: u16::from_le_bytes([body[5], body[6]]),
    })
}

/// Serialize the 0x9901 extra field (header + body).
#[must_use]
pub fn extra_bytes(version: u16, strength: AesStrength, real_method: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(11);
    v.extend_from_slice(&AES_EXTRA_TAG.to_le_bytes());
    v.extend_from_slice(&7u16.to_le_bytes());
    v.extend_from_slice(&version.to_le_bytes());
    v.extend_from_slice(b"AE");
    v.push(strength.code());
    v.extend_from_slice(&real_method.to_le_bytes());
    v
}

/// Deterministic per-entry salt: SHA-256(password ‖ name ‖ crc32) cut
/// to the strength's salt length. Keeps encrypted archives
/// byte-reproducible (task 17).
#[must_use]
pub fn derived_salt(
    password: &[u8],
    name: &[u8],
    content_crc: u32,
    strength: AesStrength,
) -> Vec<u8> {
    let mut material = Vec::with_capacity(password.len() + name.len() + 4);
    material.extend_from_slice(password);
    material.extend_from_slice(name);
    material.extend_from_slice(&content_crc.to_le_bytes());
    let digest = omnizip_crypto::sha256(&material);
    let n = strength.salt_len();
    digest[..n].to_vec()
}

/// Encrypt one entry's compressed payload into the AE wire layout:
/// salt ‖ verifier ‖ AES-CTR(payload) ‖ HMAC-SHA1[..10].
///
/// # Errors
///
/// Crypto layer failures (cannot happen with valid key material).
pub fn encrypt(
    password: &[u8],
    salt: &[u8],
    strength: AesStrength,
    compressed: &[u8],
) -> Result<Vec<u8>, ArchiveError> {
    let keys = omnizip_crypto::winzip_aes_keys(password, salt, strength.key_len());
    let mut out = Vec::with_capacity(salt.len() + 2 + compressed.len() + 10);
    out.extend_from_slice(salt);
    out.extend_from_slice(&keys.verification);

    // WinZip CTR: 128-bit little-endian counter starting at 1.
    let mut body = compressed.to_vec();
    omnizip_crypto::AesCtr::new_winzip(&keys.enc, &[0u8; 16]).apply(&mut body);
    let mac = omnizip_crypto::hmac_sha1(&keys.auth, &body);
    out.extend_from_slice(&body);
    out.extend_from_slice(&mac[..10]);
    Ok(out)
}

/// Decrypt an AE entry blob back to the compressed payload, verifying
/// the password bytes and the HMAC (in that order — a wrong password
/// fails on the verification bytes, never on padding).
///
/// # Errors
///
/// [`ArchiveError::Security`] on password/HMAC mismatch.
pub fn decrypt(
    password: &[u8],
    raw: &[u8],
    strength: AesStrength,
    name: &str,
) -> Result<Vec<u8>, ArchiveError> {
    let salt_len = strength.salt_len();
    let blob_len = salt_len + 2 + 10;
    if raw.len() < blob_len {
        return Err(ArchiveError::InvalidArchive(format!(
            "entry '{name}': WinZip AES blob too short"
        )));
    }
    let salt = &raw[..salt_len];
    let stored_verifier = &raw[salt_len..salt_len + 2];
    let body = &raw[salt_len + 2..raw.len() - 10];
    let stored_mac = &raw[raw.len() - 10..];

    let keys = omnizip_crypto::winzip_aes_keys(password, salt, strength.key_len());
    if keys.verification != *stored_verifier {
        return Err(ArchiveError::Security(format!(
            "entry '{name}': wrong password (verification bytes mismatch)"
        )));
    }
    let computed_mac = omnizip_crypto::hmac_sha1(&keys.auth, body);
    if computed_mac[..10] != *stored_mac {
        return Err(ArchiveError::Security(format!(
            "entry '{name}': HMAC-SHA1 authentication failed"
        )));
    }

    let mut plain = body.to_vec();
    omnizip_crypto::AesCtr::new_winzip(&keys.enc, &[0u8; 16]).apply(&mut plain);
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_round_trip() {
        let bytes = extra_bytes(2, AesStrength::Aes256, 8);
        let info = parse_extra(&bytes[4..]).unwrap();
        assert_eq!(info.version, 2);
        assert_eq!(info.strength, AesStrength::Aes256);
        assert_eq!(info.real_method, 8);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let salt = derived_salt(b"pw", b"name.txt", 0xDEADBEEF, AesStrength::Aes256);
        let blob = encrypt(b"pw", &salt, AesStrength::Aes256, b"compressed bytes").unwrap();
        let back = decrypt(b"pw", &blob, AesStrength::Aes256, "name.txt").unwrap();
        assert_eq!(back, b"compressed bytes");
    }

    #[test]
    fn wrong_password_fails_on_verification() {
        let salt = derived_salt(b"pw", b"name.txt", 1, AesStrength::Aes256);
        let blob = encrypt(b"pw", &salt, AesStrength::Aes256, b"data").unwrap();
        let err = decrypt(b"nope", &blob, AesStrength::Aes256, "name.txt").unwrap_err();
        assert!(err.to_string().contains("verification"), "{err}");
    }

    #[test]
    fn deterministic_salt_is_stable() {
        let a = derived_salt(b"pw", b"a", 7, AesStrength::Aes256);
        let b = derived_salt(b"pw", b"a", 7, AesStrength::Aes256);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }
}
