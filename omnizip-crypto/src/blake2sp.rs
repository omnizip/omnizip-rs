//! BLAKE2sp (RFC 7693): 8 parallel BLAKE2s lanes over interleaved
//! 64-byte blocks, folded by a final BLAKE2s-256 node. Pure Rust,
//! constant-time arithmetic, no unsafe.
//!
//! RAR5 carries a BLAKE2sp-256 hash of each entry (the `-htb`
//! switch) in its EX_HASH extra record; this implementation is sized
//! for that verification path.
#![forbid(unsafe_code)]

const BLAKE2S_BLOCK: usize = 64;
const PARALLELISM: usize = 8;
const CHUNK: usize = PARALLELISM * BLAKE2S_BLOCK; // 512

const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

#[derive(Clone)]
pub struct Blake2s {
    h: [u32; 8],
    t: u64,
    buf: [u8; BLAKE2S_BLOCK],
    buf_len: usize,
    last_node: bool,
    last_block_compressed: bool,
}

impl Blake2s {
    pub fn new(
        digest_len: u8,
        fanout: u8,
        depth: u8,
        node_offset: u32,
        node_depth: u8,
        inner_len: u8,
    ) -> Self {
        // Parameter block: digest_length, key_length, fanout, depth,
        // leaf_length, node_offset, node_depth, xof_length, inner_length,
        // reserved(14), salt(16), personal(16).
        let mut p = [0u8; BLAKE2S_BLOCK];
        p[0] = digest_len;
        p[1] = 0; // key_length
        p[2] = fanout;
        p[3] = depth;
        p[4..8].copy_from_slice(&0u32.to_le_bytes()); // leaf_length
        p[8..12].copy_from_slice(&node_offset.to_le_bytes());
        p[14] = node_depth;
        p[15] = inner_len;
        let mut h = IV;
        for (i, word) in h.iter_mut().enumerate() {
            *word ^= u32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().expect("4"));
        }
        if std::env::var_os("B2DBG").is_some() {
            eprintln!("h init: {:08x?}", h);
        }
        Self {
            h,
            t: 0,
            buf: [0; BLAKE2S_BLOCK],
            buf_len: 0,
            last_node: false,
            last_block_compressed: false,
        }
    }

    #[cfg(test)]
    fn from_raw_params(p: &[u8; 32]) -> Self {
        let mut h = IV;
        for (i, word) in h.iter_mut().enumerate() {
            *word ^= u32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().expect("4"));
        }
        Self {
            h,
            t: 0,
            buf: [0; 64],
            buf_len: 0,
            last_node: false,
            last_block_compressed: false,
        }
    }

    pub fn with_last_node(mut self) -> Self {
        self.last_node = true;
        self
    }

    fn compress(&mut self, block: &[u8], last: bool) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().expect("4"));
        }
        let mut v = [0u32; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= self.t as u32;
        v[13] ^= (self.t >> 32) as u32;
        if last {
            v[14] = !v[14];
            if self.last_node {
                v[15] = !v[15];
            }
        }
        for round in 0..10 {
            let s = &SIGMA[round];
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if self.buf_len == BLAKE2S_BLOCK {
                self.t += BLAKE2S_BLOCK as u64;
                let block = self.buf;
                self.compress(&block, false);
                self.buf_len = 0;
            }
            let take = (BLAKE2S_BLOCK - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
        }
    }

    pub fn finalize(mut self, out: &mut [u8]) {
        let remaining = self.buf_len;
        self.t += remaining as u64;
        for b in self.buf[remaining..].iter_mut() {
            *b = 0;
        }
        let block = self.buf;
        self.compress(&block, true);
        self.last_block_compressed = true;
        for (i, byte) in out.iter_mut().enumerate() {
            let word = self.h[i / 4];
            *byte = (word >> (8 * (i % 4))) as u8;
        }
    }
}

fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// BLAKE2sp-256 state: 8 leaf lanes plus the buffered 512-byte chunk.
pub struct Blake2sp {
    leaves: Vec<Blake2s>,
    buffer: [u8; CHUNK],
    buffered: usize,
}

impl Default for Blake2sp {
    fn default() -> Self {
        Self::new()
    }
}

impl Blake2sp {
    #[must_use]
    pub fn new() -> Self {
        let mut leaves = Vec::with_capacity(PARALLELISM);
        for i in 0..PARALLELISM {
            let mut leaf = Blake2s::new(32, PARALLELISM as u8, 2, i as u32, 0, 32);
            if i == PARALLELISM - 1 {
                leaf = leaf.with_last_node();
            }
            leaves.push(leaf);
        }
        Self {
            leaves,
            buffer: [0; CHUNK],
            buffered: 0,
        }
    }

    fn flush_chunk(&mut self) {
        for (i, leaf) in self.leaves.iter_mut().enumerate() {
            leaf.update(&self.buffer[i * BLAKE2S_BLOCK..(i + 1) * BLAKE2S_BLOCK]);
        }
        self.buffered = 0;
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if self.buffered == CHUNK {
                self.flush_chunk();
            }
            let take = (CHUNK - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
        }
    }

    /// Finish and write the 32-byte digest.
    pub fn finalize(mut self) -> [u8; 32] {
        if self.buffered > 0 {
            for b in self.buffer[self.buffered..].iter_mut() {
                *b = 0;
            }
            for (i, leaf) in self.leaves.iter_mut().enumerate() {
                let start = i * BLAKE2S_BLOCK;
                let end = (start + BLAKE2S_BLOCK).min(self.buffered);
                if start < self.buffered {
                    leaf.update(&self.buffer[start..end]);
                }
            }
            self.buffered = 0;
        }
        let mut leaf_out = [0u8; PARALLELISM * 32];
        for (i, leaf) in self.leaves.drain(..).enumerate() {
            let mut buf = [0u8; 32];
            leaf.finalize(&mut buf);
            if std::env::var_os("B2DBG").is_some() {
                eprintln!(
                    "leaf {i}: {}",
                    buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
                );
            }
            leaf_out[i * 32..(i + 1) * 32].copy_from_slice(&buf);
        }
        let mut root = Blake2s::new(32, PARALLELISM as u8, 2, 0, 1, 32).with_last_node();
        root.update(&leaf_out);
        let mut out = [0u8; 32];
        root.finalize(&mut out);
        out
    }
}

/// One-shot BLAKE2sp-256.
#[must_use]
pub fn blake2sp_256(data: &[u8]) -> [u8; 32] {
    let mut h = Blake2sp::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake2s_param_bisect() {
        for (fanout, depth, offset, node_depth, inner, want) in [
            (
                8u8,
                1u8,
                0u32,
                0u8,
                0u8,
                "11cf1a69c94f3ab04678c152db5d7474a5b250dc95c08c25e4e72855a2df7ea7",
            ),
            (
                8,
                2,
                0,
                0,
                0,
                "3fa4579b33df3f4188f74f59a947bf539738d6fd8b21ed1373ad3fa1f360bbb1",
            ),
            (
                8,
                2,
                0,
                1,
                0,
                "001b906a5c71f83cc90ec557c0f34b84f587da9eec76db800deab62dd206bcf1",
            ),
            (
                1,
                1,
                0,
                0,
                32,
                "56be410b189c62c8bd8a134f714129780e194cb88a70ccc8ad93ca8690af395c",
            ),
            (
                1,
                1,
                3,
                0,
                0,
                "45ff4e10485338457b08539633250063b6dbf60b39dfdf99cccf89c3683bcfe3",
            ),
        ] {
            let mut h = Blake2s::new(32, fanout, depth, offset, node_depth, inner);
            h.update(&[0u8; 64]);
            let mut out = [0u8; 32];
            h.finalize(&mut out);
            let got: String = out.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(
                got, want,
                "f={fanout} d={depth} o={offset} nd={node_depth} i={inner}"
            );
        }
    }

    #[test]
    fn blake2s_param_matrix() {
        // hashlib.blake2s reference matrix (fanout=8 depth=2
        // node_offset=0 node_depth=1 inner=32).
        for (n, last_node, want) in [
            (
                64usize,
                false,
                "5eb933c7deef656346cb0cad0f0cb13989f30f0b2cc640a5889b9d3ae0dd2bc4",
            ),
            (
                128,
                false,
                "6a6ae24013242f62756c4ea35f82f82c3ae664cff594c48cbfcddda585ad4a49",
            ),
            (
                256,
                false,
                "7f6a1500e8ee331e3162e5af339e217853940db3070ba3672e332f9e19b755b8",
            ),
            (
                64,
                true,
                "cecbddadee5c1075d632413d2ad36f70c3f896a4ed26e48c7fecf11b92744213",
            ),
            (
                128,
                true,
                "ef6a59cc4b3b4e6c98ea2183a27aa3dde667b308aa58fe9f799cd194364e4045",
            ),
            (
                256,
                true,
                "b6724c0b3e063ce3b70116d47608b5a8abbc60b29b5fc3c3d885cb9ff6cc3311",
            ),
        ] {
            let mut h = Blake2s::new(32, 8, 2, 0, 1, 32);
            if last_node {
                h = h.with_last_node();
            }
            h.update(&vec![0u8; n]);
            let mut out = [0u8; 32];
            h.finalize(&mut out);
            let got: String = out.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, want, "n={n} last_node={last_node}");
        }
    }

    #[test]
    fn blake2s_core_vectors() {
        // hashlib.blake2s references, default parameters.
        for (data, want) in [
            (
                &b""[..],
                "69217a3079908094e11121d042354a7c1f55b6482ca1a51e1b250dfd1ed0eef9",
            ),
            (
                b"a",
                "4a0d129873403037c2cd9b9048203687f6233fb6738956e0349bd4320fec3e90",
            ),
            (
                b"ab",
                "19c3ebeed2ee90063cb5a8a4dd700ed7e5852dfc6108c84fac85888682a18f0e",
            ),
            (
                b"abc",
                "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982",
            ),
            (
                &[0u8; 64][..],
                "ae09db7cd54f42b490ef09b6bc541af688e4959bb8c53f359a6f56e38ab454a3",
            ),
            (
                &[0u8; 128][..],
                "4e420520b981ce7bdbf4ce2c4dbadb9450079b7deb9737b5232957d323f801cb",
            ),
        ] {
            let mut h = Blake2s::new(32, 1, 1, 0, 0, 0);
            h.update(data);
            let mut out = [0u8; 32];
            h.finalize(&mut out);
            let got: String = out.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, want, "data={data:?}");
        }
    }

    #[test]
    fn empty_input() {
        // BLAKE2sp-256(""), cross-checked against a reference
        // blake2s core with the RFC tree wiring.
        assert_eq!(
            blake2sp_256(b""),
            hex("dd0e891776933f43c7d032b08a917e25741f8aa9a12c12e1cac8801500f2ca4f")
        );
    }

    #[test]
    fn abc_input() {
        assert_eq!(
            blake2sp_256(b"abc"),
            hex("70f75b58f1fecab821db43c88ad84edde5a52600616cd22517b7bb14d440a7d5")
        );
    }

    #[test]
    fn rar5_cebula_vector() {
        // WinRAR stored this BLAKE2sp-256 for cebula.txt (814 bytes)
        // in test_read_format_rar5_blake2.rar; the content decodes
        // CRC-clean through our own rar5 reader.
        let d = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../omnizip/spec/fixtures/rar/libarchive_reference/extracted-cebula.txt"
        ));
        let Ok(d) = d else { return };
        assert_eq!(
            blake2sp_256(&d),
            hex("e67b86259a1cd0d51b6d6776ce10b5a5cf619559903c009ca8c346d6453853a5")
        );
    }

    fn hex(s: &str) -> [u8; 32] {
        let b = s.as_bytes();
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] =
                u8::from_str_radix(std::str::from_utf8(&b[i * 2..i * 2 + 2]).unwrap(), 16).unwrap();
        }
        out
    }
}
