//! Crypto primitives for the omnizip container formats — TODO.containers
//! task 05's decision crate. The codecs' "no dependencies" rule exists
//! to keep the wire-format crates minimal; crypto is a different domain
//! where hand-rolling is exactly the wrong move, so the vetted
//! RustCrypto implementations (all `#![forbid(unsafe_code)]` cores)
//! are wrapped here behind narrow, format-facing functions:
//!
//! - WinZip AES (AE-1/AE-2): AES-CTR + HMAC-SHA1/SHA2 + PBKDF2
//! - RPM file digests (MD5 hex, per the rpm header format)
//! - PAR2 slice hashing (MD5) — see task 13
//!
//! Codec crates never depend on this; only container crates do.
#![forbid(unsafe_code)]

use digest::Digest;

/// MD5 digest, raw 16 bytes (PAR2 ids, packet checksums).
#[must_use]
pub fn md5(data: &[u8]) -> [u8; 16] {
    md5::Md5::digest(data).into()
}

/// MD5 digest, hex-encoded lowercase (RPM `filedigests`, PAR2 packet ids).
#[must_use]
pub fn md5_hex(data: &[u8]) -> String {
    hex(&md5(data))
}

/// SHA-1 digest, raw bytes (WinZip AE-1 HMAC, PAR2 file ids).
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1::Sha1::digest(data).into()
}

/// SHA-256 digest, raw bytes (7z AES key derivation, RAR5 KDF).
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(data).into()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// PBKDF2-HMAC-SHA1 (WinZip AES, 1000 iterations per the spec).
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, iterations, out);
}

/// PBKDF2-HMAC-SHA256 (RAR5 KDF, 2^16+ iterations per spec).
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, salt, iterations, out);
}

/// HMAC-SHA1 over `data` with `key` (WinZip AE-1 authentication).
#[must_use]
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    use hmac::{KeyInit, Mac};
    let mut mac = <hmac::Hmac<sha1::Sha1>>::new_from_slice(key).expect("any key length");
    mac.update(data);
    let out: [u8; 20] = mac.finalize().into_bytes().into();
    out
}

/// HMAC-SHA256 over `data` with `key` (WinZip AE-2 authentication).
#[must_use]
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use hmac::{KeyInit, Mac};
    let mut mac = <hmac::Hmac<sha2::Sha256>>::new_from_slice(key).expect("any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// AES-128/192/256 in CTR mode (no padding, stream-shaped) — the
/// WinZip AES cipher. Keystream starts at counter 0/1 per spec usage.
pub struct AesCtr {
    inner: AesCtrInner,
}

enum AesCtrInner {
    Aes128(ctr::Ctr128BE<aes::Aes128>),
    Aes192(ctr::Ctr128BE<aes::Aes192>),
    Aes256(ctr::Ctr128BE<aes::Aes256>),
    WinZip(AesCtrWinZip),
}

/// WinZip AES CTR: the 16-byte counter is a 128-bit *little-endian*
/// integer starting at 1 (a spec quirk; verified against 7-Zip's
/// WzAes). Implemented with the ctr crate's LE flavor from that
/// initial nonce.
pub struct AesCtrWinZip {
    inner: WinZipInner,
}

enum WinZipInner {
    Aes128(ctr::Ctr128LE<aes::Aes128>),
    Aes192(ctr::Ctr128LE<aes::Aes192>),
    Aes256(ctr::Ctr128LE<aes::Aes256>),
}

impl AesCtr {
    /// Build a 128-bit-key CTR stream.
    #[must_use]
    pub fn new_aes128(key: &[u8; 16], nonce: &[u8; 16]) -> Self {
        use aes::cipher::{InnerIvInit, KeyInit};
        let enc = aes::Aes128::new(key.into());
        let core = ctr::CtrCore::<_, ctr::flavors::Ctr128BE>::inner_iv_init(enc, nonce.into());
        Self {
            inner: AesCtrInner::Aes128(ctr::Ctr128BE::from_core(core)),
        }
    }

    /// Build a 192-bit-key CTR stream.
    #[must_use]
    pub fn new_aes192(key: &[u8; 24], nonce: &[u8; 16]) -> Self {
        use aes::cipher::{InnerIvInit, KeyInit};
        let enc = aes::Aes192::new(key.into());
        let core = ctr::CtrCore::<_, ctr::flavors::Ctr128BE>::inner_iv_init(enc, nonce.into());
        Self {
            inner: AesCtrInner::Aes192(ctr::Ctr128BE::from_core(core)),
        }
    }

    /// Build a 256-bit-key CTR stream.
    #[must_use]
    pub fn new_aes256(key: &[u8; 32], nonce: &[u8; 16]) -> Self {
        use aes::cipher::{InnerIvInit, KeyInit};
        let enc = aes::Aes256::new(key.into());
        let core = ctr::CtrCore::<_, ctr::flavors::Ctr128BE>::inner_iv_init(enc, nonce.into());
        Self {
            inner: AesCtrInner::Aes256(ctr::Ctr128BE::from_core(core)),
        }
    }

    /// WinZip AES CTR stream (LE counter from 1) at 128/192/256 bits.
    #[must_use]
    pub fn new_winzip(key: &[u8], nonce: &[u8; 16]) -> Self {
        use aes::cipher::{InnerIvInit, KeyInit};
        let mut n = [0u8; 16];
        n[0] = 1; // little-endian counter starting at 1
        let _ = nonce;
        let inner = match key.len() {
            16 => {
                let enc = aes::Aes128::new(key[..16].try_into().expect("16"));
                let core =
                    ctr::CtrCore::<_, ctr::flavors::Ctr128LE>::inner_iv_init(enc, (&n).into());
                WinZipInner::Aes128(ctr::Ctr128LE::from_core(core))
            }
            24 => {
                let enc = aes::Aes192::new(key[..24].try_into().expect("24"));
                let core =
                    ctr::CtrCore::<_, ctr::flavors::Ctr128LE>::inner_iv_init(enc, (&n).into());
                WinZipInner::Aes192(ctr::Ctr128LE::from_core(core))
            }
            _ => {
                let enc = aes::Aes256::new(key[..32].try_into().expect("32"));
                let core =
                    ctr::CtrCore::<_, ctr::flavors::Ctr128LE>::inner_iv_init(enc, (&n).into());
                WinZipInner::Aes256(ctr::Ctr128LE::from_core(core))
            }
        };
        Self {
            inner: AesCtrInner::WinZip(AesCtrWinZip { inner }),
        }
    }

    /// XOR `data` with the keystream in place.
    pub fn apply(&mut self, data: &mut [u8]) {
        use aes::cipher::StreamCipher;
        match &mut self.inner {
            AesCtrInner::Aes128(c) => c.apply_keystream(data),
            AesCtrInner::Aes192(c) => c.apply_keystream(data),
            AesCtrInner::Aes256(c) => c.apply_keystream(data),
            AesCtrInner::WinZip(w) => match &mut w.inner {
                WinZipInner::Aes128(c) => c.apply_keystream(data),
                WinZipInner::Aes192(c) => c.apply_keystream(data),
                WinZipInner::Aes256(c) => c.apply_keystream(data),
            },
        }
    }
}

/// AES-256-CBC with zero IV — the 7z encrypted-stream shape (task 06
/// Phase C). Returns ciphertext with PKCS-style in-stream padding left
/// to the caller (7z defines its own tail).
pub struct AesCbc256 {
    encryptor: ecb_mode_encrypt_adapter::Aes256CbcEncrypt,
}

// RustCrypto cbc crate split; provide a minimal CBC wrapper over aes's
// block cipher to avoid pulling the `cbc` crate's encryptor traits.
mod ecb_mode_encrypt_adapter {
    use aes::cipher::BlockCipherEncrypt;

    /// CBC encryption over 16-byte blocks, IV prepended by the caller.
    pub struct Aes256CbcEncrypt {
        cipher: aes::Aes256,
        prev: [u8; 16],
    }

    impl Aes256CbcEncrypt {
        pub fn new(key: &[u8; 32], iv: &[u8; 16]) -> Self {
            use aes::cipher::KeyInit;
            Self {
                cipher: aes::Aes256::new(key.into()),
                prev: *iv,
            }
        }

        /// Encrypt whole blocks in place (len must be a multiple of 16).
        pub fn encrypt_blocks(&mut self, data: &mut [u8]) {
            assert_eq!(data.len() % 16, 0, "CBC input must be block-aligned");
            for chunk in data.chunks_exact_mut(16) {
                for (b, p) in chunk.iter_mut().zip(self.prev) {
                    *b ^= p;
                }
                let block: &mut [u8; 16] = chunk.try_into().expect("16 bytes");
                self.cipher.encrypt_block(block.into());
                self.prev.copy_from_slice(chunk);
            }
        }
    }
}

impl AesCbc256 {
    #[must_use]
    pub fn new(key: &[u8; 32], iv: &[u8; 16]) -> Self {
        Self {
            encryptor: ecb_mode_encrypt_adapter::Aes256CbcEncrypt::new(key, iv),
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.encryptor.encrypt_blocks(data);
    }
}

/// AES-256-CBC decryption (RAR5, 7z).
pub struct AesCbc256Decrypt {
    cipher: aes::Aes256,
    prev: [u8; 16],
}

impl AesCbc256Decrypt {
    #[must_use]
    pub fn new(key: &[u8; 32], iv: &[u8; 16]) -> Self {
        use aes::cipher::KeyInit;
        Self {
            cipher: aes::Aes256::new(key.into()),
            prev: *iv,
        }
    }

    /// Decrypt whole blocks in place (len must be a multiple of 16).
    pub fn decrypt(&mut self, data: &mut [u8]) {
        use aes::cipher::BlockCipherDecrypt;
        assert_eq!(data.len() % 16, 0, "CBC input must be block-aligned");
        for chunk in data.chunks_exact_mut(16) {
            let cipher_block: [u8; 16] = chunk.try_into().expect("16 bytes");
            let block: &mut [u8; 16] = chunk.try_into().expect("16 bytes");
            self.cipher.decrypt_block(block.into());
            for (b, p) in chunk.iter_mut().zip(self.prev) {
                *b ^= p;
            }
            self.prev = cipher_block;
        }
    }
}

/// WinZip AES key schedule: PBKDF2-HMAC-SHA1, 1000 iterations, over
/// `password` + `salt`, producing `2*key_len + 2` bytes laid out per
/// the WinZip AES spec (byte-verified against 7-Zip's WzAes):
///
/// ```text
/// [CTR enc key (key_len)][HMAC-SHA1 key (key_len)][verifier 2B]
/// ```
#[must_use]
pub fn winzip_aes_keys(password: &[u8], salt: &[u8], key_len: usize) -> WinZipAesKeys {
    let mut derived = vec![0u8; 2 * key_len + 2];
    pbkdf2_hmac_sha1(password, salt, 1000, &mut derived);
    let enc = derived[0..key_len].to_vec();
    let auth = derived[key_len..2 * key_len].to_vec();
    let verification = [derived[2 * key_len], derived[2 * key_len + 1]];
    WinZipAesKeys {
        enc,
        auth,
        verification,
    }
}

/// The WinZip AES derived key material.
pub struct WinZipAesKeys {
    pub enc: Vec<u8>,
    pub auth: Vec<u8>,
    pub verification: [u8; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vector() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn sha_known_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn pbkdf2_sha1_rfc6070_vector() {
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 1, &mut out);
        // First 20 bytes of the RFC 6070 c=1 vector (25 bytes long).
        let expected_full = [
            0x0c, 0x60, 0xc8, 0x0f, 0x96, 0x1f, 0x0e, 0x71, 0xf3, 0xa9, 0xb5, 0x24, 0xaf, 0x60,
            0x12, 0x06, 0x2f, 0xe0, 0x37, 0xa6,
        ];
        assert_eq!(out, expected_full);
    }

    #[test]
    fn aes_ctr_round_trip() {
        let key = [7u8; 32];
        let nonce = [9u8; 16];
        let mut a = AesCtr::new_aes256(&key, &nonce);
        let mut data = b"hello winzip aes".to_vec();
        a.apply(&mut data);
        let mut b = AesCtr::new_aes256(&key, &nonce);
        b.apply(&mut data);
        assert_eq!(data, b"hello winzip aes");
    }

    #[test]
    fn aes_cbc_round_trip() {
        let key = [3u8; 32];
        let iv = [5u8; 16];
        let plaintext = vec![0x42u8; 32];
        let mut ct = plaintext.clone();
        AesCbc256::new(&key, &iv).encrypt(&mut ct);
        assert_ne!(ct, plaintext);
        let mut back = ct;
        AesCbc256Decrypt::new(&key, &iv).decrypt(&mut back);
        assert_eq!(back, plaintext);
    }

    #[test]
    fn winzip_keys_shape() {
        let keys = winzip_aes_keys(b"secret", &[0xAA; 16], 32);
        assert_eq!(keys.enc.len(), 32);
        assert_eq!(keys.auth.len(), 32);
    }
}
