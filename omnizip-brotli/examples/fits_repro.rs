fn main() {
    let total: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000_000);
    let q: i32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let mut d = Vec::with_capacity(total);
    let mut header = vec![b' '; 2880];
    for (off, rec) in [
        "SIMPLE  = T",
        "BITPIX  = 16",
        "NAXIS   = 2",
        "NAXIS1  = 5000",
        "NAXIS2  = 5000",
        "END",
    ]
    .iter()
    .enumerate()
    {
        let b = rec.as_bytes();
        header[off * 80..off * 80 + b.len()].copy_from_slice(b);
    }
    d.extend_from_slice(&header);
    let mut state: u64 = 0x1234_5678_9ABC_DEF0;
    let total_pixels = (total - 2880) / 2;
    for idx in 0..total_pixels as u64 {
        let base = ((idx) / 8) & 0xFFFF;
        let noise = (state >> 56) & 0x0F;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let pixel = (base ^ noise as u64) & 0xFFFF;
        d.push((pixel >> 8) as u8);
        d.push(pixel as u8);
    }
    let out = omnizip_brotli::from_spec_encoder::compress_with_quality(&d, q);
    std::fs::write("/tmp/fits_out.br", &out).unwrap();
    std::fs::write("/tmp/fits_in.bin", &d).unwrap(); // save input for decode validation
    println!("OK {} -> {}", d.len(), out.len());
}
