//! Legacy PKWARE "traditional" ZipCrypto (APPNOTE 6.1.x) — the read
//! path only. Symmetric xor stream keyed by three CRC-rolled 32-bit
//! keys; a 12-byte encryption header carries the password check byte.

#![forbid(unsafe_code)]

use omnizip_archive_core::ArchiveError;

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = build_crc_table();

fn crc_update(crc: u32, byte: u8) -> u32 {
    (crc >> 8) ^ CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize]
}

struct Keys([u32; 3]);

impl Keys {
    fn new(password: &[u8]) -> Self {
        let mut k = Self([0x1234_5678, 0x2345_6789, 0x3456_7890]);
        for &b in password {
            k.update(b);
        }
        k
    }

    fn update(&mut self, byte: u8) {
        self.0[0] = crc_update(self.0[0], byte);
        self.0[1] = self.0[1].wrapping_add(self.0[0] & 0xff);
        self.0[1] = self.0[1].wrapping_mul(134_775_813).wrapping_add(1);
        self.0[2] = crc_update(self.0[2], (self.0[1] >> 24) as u8);
    }

    fn stream_byte(&self) -> u8 {
        let temp = self.0[2] | 2;
        (((temp.wrapping_mul(temp ^ 1)) >> 8) & 0xff) as u8
    }

    /// Keys always roll with the PLAIN byte (APPNOTE 6.1.4) — that is
    /// what makes encrypt and decrypt distinct operations sharing one
    /// keystream.
    fn decrypt_in_place(&mut self, data: &mut [u8]) {
        for b in data.iter_mut() {
            let plain = *b ^ self.stream_byte();
            self.update(plain);
            *b = plain;
        }
    }

    /// Test/fixture seam: encrypt with the same keystream semantics.
    #[cfg(test)]
    fn encrypt_in_place(&mut self, data: &mut [u8]) {
        for b in data.iter_mut() {
            let cipher = *b ^ self.stream_byte();
            self.update(*b);
            *b = cipher;
        }
    }
}

const HEADER_LEN: usize = 12;

/// Decrypt a legacy-encrypted entry blob (12-byte header + compressed
/// payload) back to the compressed payload.
///
/// `check_crc` is the central-directory CRC32 (its top byte verifies
/// the password); `check_time` the DOS mtime, whose high byte is the
/// alternative check when the entry uses a data descriptor (flag bit
/// 3); `flags` carries that bit.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on a blob shorter than the header;
/// [`ArchiveError::Security`] on password-verification failure.
pub fn decrypt(
    password: &[u8],
    raw: &[u8],
    check_crc: u32,
    check_time: u32,
    flags: u16,
    name: &str,
) -> Result<Vec<u8>, ArchiveError> {
    if raw.len() < HEADER_LEN {
        return Err(ArchiveError::InvalidArchive(format!(
            "entry '{name}': ZipCrypto blob shorter than the 12-byte header"
        )));
    }
    let mut keys = Keys::new(password);
    let mut header = [0u8; HEADER_LEN];
    header.copy_from_slice(&raw[..HEADER_LEN]);
    keys.decrypt_in_place(&mut header);

    let crc_byte = ((check_crc >> 24) & 0xff) as u8;
    let time_byte = ((check_time >> 8) & 0xff) as u8;
    let ok = if flags & 0x0008 != 0 {
        header[11] == crc_byte || header[11] == time_byte
    } else {
        header[11] == crc_byte
    };
    if !ok {
        return Err(ArchiveError::Security(format!(
            "entry '{name}': wrong password (ZipCrypto check byte mismatch)"
        )));
    }

    let mut body = raw[HEADER_LEN..].to_vec();
    keys.decrypt_in_place(&mut body);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_is_symmetric() {
        let plain = b"deflate-me-please payload bytes".to_vec();
        let mut enc = plain.clone();
        Keys::new(b"secret").encrypt_in_place(&mut enc);
        assert_ne!(enc, plain);
        let mut dec = enc;
        Keys::new(b"secret").decrypt_in_place(&mut dec);
        assert_eq!(dec, plain);
    }

    #[test]
    fn wrong_password_fails_check_byte() {
        let crc = 0xAB00_0000;
        let mut blob = vec![0u8; HEADER_LEN + 8];
        blob[11] = ((crc >> 24) & 0xff) as u8;
        Keys::new(b"right").encrypt_in_place(&mut blob);
        let r = decrypt(b"wrong", &blob, crc, 0, 0, "e");
        assert!(matches!(r, Err(ArchiveError::Security { .. })));
    }

    #[test]
    fn round_trip_via_public_api() {
        let payload = b"STORE method payload".to_vec();
        let crc = 0x12_AB_CD_EF;
        // Build a ZipCrypto blob: run the cipher over header+payload
        // in one stream (the writer's shape).
        let mut blob = vec![0u8; HEADER_LEN];
        blob.extend_from_slice(&payload);
        blob[11] = ((crc >> 24) & 0xff) as u8;
        Keys::new(b"pw").encrypt_in_place(&mut blob);
        let out = decrypt(b"pw", &blob, crc, 0, 0, "e").unwrap();
        assert_eq!(out, payload);
    }
}
