fn ref_read(data: &[u8], pos: usize, n: u32) -> u32 {
    let mut r = 0u32;
    for i in 0..n {
        let bi = pos + i as usize;
        let byte = bi / 8;
        let bit = if byte < data.len() {
            (data[byte] >> (bi % 8)) & 1
        } else {
            0
        };
        r |= u32::from(bit) << i;
    }
    r
}

fn main() {
    let data = [
        0x21u8, 0x00, 0x00, 0x04, 0x61, 0x03, 0xA7, 0xF0, 0x5B, 0x8E, 0x11, 0xC3,
    ];
    let mut br = omnizip_brotli::decoder::BitReader::new(&data);
    let mut pos_ref = 0usize;
    for n in [1u32, 3, 1, 4, 16, 1, 1, 7, 24, 5, 13, 32, 2, 11] {
        let v = br.read_bits(n);
        let r = ref_read(&data, pos_ref, n);
        pos_ref += n as usize;
        assert_eq!(v, r, "read({n}) @{pos_ref}");
    }
    // Randomized seek + mixed peek/read/drop sequences.
    let mut seed = 0x12345678u64;
    let mut rng = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let pos = (rng() % 100) as usize;
        br.set_bit_pos(pos);
        let mut p = pos;
        for _ in 0..8 {
            let n = 1 + (rng() % 24) as u32;
            match rng() % 3 {
                0 => {
                    let v = br.peek_bits(n);
                    assert_eq!(v, ref_read(&data, p, n), "peek({n}) @{p}");
                }
                _ => {
                    let v = br.read_bits(n);
                    assert_eq!(v, ref_read(&data, p, n), "read({n}) @{p}");
                    p += n as usize;
                }
            }
        }
    }
    println!("ALL-OK");
}
