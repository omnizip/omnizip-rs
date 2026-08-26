//! RAR3 encryption (unrar crypt3.cpp `SetKey30`), pure Rust.
//!
//! Key derivation: the raw password is UTF-16LE bytes plus the 8-byte
//! salt, hashed through 0x40000 rounds of a modified SHA1 that feeds
//! the 3-byte little-endian round counter and — the famous rar29 quirk
//! — overwrites full 64-byte input blocks with words from the SHA1
//! message schedule (only observable for password buffers ≥ 64 bytes).
//! Every 0x4000th round the running state is finished into one byte of
//! the CBC init vector; the final state's first 16 bytes (as five
//! little-endian u32 words) become the AES-128 key. Data areas are
//! AES-128-CBC with chaining across the whole entry stream; the packed
//! size is padded up to a 16-byte multiple.
#![forbid(unsafe_code)]

const HASH_ROUNDS: u32 = 0x40000;
const HASH_ROUNDS_STEP: u32 = HASH_ROUNDS / 16;

struct Sha1Rar29 {
    state: [u32; 5],
    count: u64,
    buffer: [u8; 64],
}

fn rotl32(v: u32, n: u32) -> u32 {
    v.rotate_left(n)
}

impl Sha1Rar29 {
    fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0],
            count: 0,
            buffer: [0u8; 64],
        }
    }

    /// SHA1 block transform. `w` seeds the 16-word schedule (bytes are
    /// big-endian in the digest input). The schedule is expanded in
    /// place, so on return `w` holds the final 16 expanded words — the
    /// source the rar29 writeback copies back into the input.
    fn transform(&mut self, w: &mut [u32; 16]) {
        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        for i in 0..80 {
            let f = match i / 20 {
                0 => (b & (c ^ d)) ^ d,
                1 | 3 => b ^ c ^ d,
                _ => ((b | c) & d) | (b & c),
            };
            let k = match i / 20 {
                0 => 0x5A82_7999,
                1 => 0x6ED9_EBA1,
                2 => 0x8F1B_BCDC,
                _ => 0xCA62_C1D6,
            };
            if i >= 16 {
                w[i & 15] = rotl32(
                    w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15],
                    1,
                );
            }
            let t = rotl32(a, 5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i & 15]);
            e = d;
            d = c;
            c = rotl32(b, 30);
            b = a;
            a = t;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }

    /// `sha1_process_rar29`: buffer-accumulating update that rewrites
    /// full input blocks (beyond the first of this call) with the final
    /// expanded schedule words, little-endian.
    fn process_rar29(&mut self, data: &mut [u8]) {
        let len = data.len();
        let mut j = (self.count & 63) as usize;
        self.count += len as u64;
        let mut i = 0usize;
        if j + len > 63 {
            self.buffer[j..].copy_from_slice(&data[..64 - j]);
            i = 64 - j;
            let mut w = [0u32; 16];
            for (k, chunk) in self.buffer.chunks_exact(4).enumerate() {
                w[k] = u32::from_be_bytes(chunk.try_into().expect("4"));
            }
            self.transform(&mut w);
            while i + 63 < len {
                let mut w = [0u32; 16];
                let block = &mut data[i..i + 64];
                for (k, chunk) in block.chunks_exact(4).enumerate() {
                    w[k] = u32::from_be_bytes(chunk.try_into().expect("4"));
                }
                self.transform(&mut w);
                for k in 0..16 {
                    block[k * 4..k * 4 + 4].copy_from_slice(&w[k].to_le_bytes());
                }
                i += 64;
            }
            j = 0;
        }
        if len > i {
            self.buffer[j..j + len - i].copy_from_slice(&data[i..]);
        }
    }

    /// `sha1_done`: standard padding, returns the digest words.
    fn finish(mut self) -> [u32; 5] {
        let bit_length = self.count * 8;
        let mut buf_pos = (self.count & 63) as usize;
        self.buffer[buf_pos] = 0x80;
        buf_pos += 1;
        if buf_pos != 56 {
            if buf_pos > 56 {
                for b in &mut self.buffer[buf_pos..] {
                    *b = 0;
                }
                buf_pos = 0;
                let mut w = [0u32; 16];
                for (k, chunk) in self.buffer.chunks_exact(4).enumerate() {
                    w[k] = u32::from_be_bytes(chunk.try_into().expect("4"));
                }
                self.transform(&mut w);
            }
            for b in &mut self.buffer[buf_pos..56] {
                *b = 0;
            }
        }
        self.buffer[56..60].copy_from_slice(&((bit_length >> 32) as u32).to_be_bytes());
        self.buffer[60..64].copy_from_slice(&(bit_length as u32).to_be_bytes());
        let mut w = [0u32; 16];
        for (k, chunk) in self.buffer.chunks_exact(4).enumerate() {
            w[k] = u32::from_be_bytes(chunk.try_into().expect("4"));
        }
        self.transform(&mut w);
        self.state
    }
}

/// Derived RAR3 keys: AES-128 key and CBC init vector.
pub struct Rar3Keys {
    pub aes_key: [u8; 16],
    pub aes_init: [u8; 16],
}

/// Port of unrar's `SetKey30` key schedule.
#[must_use]
pub fn set_key30(password: &[u8], salt: Option<&[u8; 8]>) -> Rar3Keys {
    let mut raw: Vec<u8> = Vec::with_capacity(password.len() * 2 + 8);
    for unit in String::from_utf8_lossy(password).encode_utf16() {
        raw.extend_from_slice(&unit.to_le_bytes());
    }
    if let Some(salt) = salt {
        raw.extend_from_slice(salt);
    }

    let mut ctx = Sha1Rar29::new();
    let mut aes_init = [0u8; 16];
    for round in 0..HASH_ROUNDS {
        ctx.process_rar29(&mut raw);
        let mut counter = [round as u8, (round >> 8) as u8, (round >> 16) as u8];
        ctx.process_rar29(&mut counter);
        if round % HASH_ROUNDS_STEP == 0 {
            let digest = {
                let temp = Sha1Rar29 {
                    state: ctx.state,
                    count: ctx.count,
                    buffer: ctx.buffer,
                };
                temp.finish()
            };
            aes_init[(round / HASH_ROUNDS_STEP) as usize] = digest[4] as u8;
        }
    }
    let digest = ctx.finish();
    let mut aes_key = [0u8; 16];
    for i in 0..4 {
        for j in 0..4 {
            aes_key[i * 4 + j] = (digest[i] >> (j * 8)) as u8;
        }
    }
    Rar3Keys { aes_key, aes_init }
}

/// Decrypt a RAR3-encrypted data area in place: continuous AES-128-CBC
/// over the 16-aligned prefix; any sub-16 tail stays untouched (the
/// format pads the stored packed size, so a tail only appears in
/// malformed archives).
pub fn decrypt_rar30(password: &[u8], salt: Option<&[u8; 8]>, data: &mut [u8]) {
    let keys = set_key30(password, salt);
    let whole = data.len() - data.len() % 16;
    let mut cipher = omnizip_crypto::AesCbc128Decrypt::new(&keys.aes_key, &keys.aes_init);
    cipher.decrypt(&mut data[..whole]);
}

/// Decrypt an encrypted-header stream: returns the plaintext for the
/// whole buffer (block-aligned part).
pub fn decrypt_headers_rar30(password: &[u8], salt: &[u8; 8], data: &[u8]) -> Vec<u8> {
    let mut buf = data.to_vec();
    decrypt_rar30(password, Some(salt), &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_reference() {
        // The normal path must agree with the generic SHA1.
        for msg in [&b""[..], b"abc", b"The quick brown fox jumps over the lazy dog"] {
            let mut ctx = Sha1Rar29::new();
            ctx.process_rar29(&mut msg.to_vec());
            let digest = ctx.finish();
            let bytes: Vec<u8> = digest.iter().flat_map(|w| w.to_be_bytes()).collect();
            assert_eq!(bytes, omnizip_crypto::sha1(msg));
        }
    }

    #[test]
    fn key_schedule_is_deterministic() {
        let a = set_key30(b"password", Some(&[1u8; 8]));
        let b = set_key30(b"password", Some(&[1u8; 8]));
        assert_eq!(a.aes_key, b.aes_key);
        assert_eq!(a.aes_init, b.aes_init);
        let c = set_key30(b"password", None);
        assert_ne!(a.aes_key, c.aes_key);
    }
}
