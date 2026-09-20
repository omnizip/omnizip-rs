//! OMNIZIP_ZSTD_SEQ_SPLIT=1 (task 21's opt-in gate extending the
//! sequence splitter to Greedy/Lazy/Lazy2): the split path must stay
//! deterministic and decode-valid on the regime where it engages
//! (≥1 MiB heterogeneous fits-class content; smaller inputs find no
//! winning split). Single test in its own binary — env mutation
//! cannot race sibling tests here.

#[test]
fn env_split_path_is_deterministic_and_valid() {
    // fits-class synthetic: drifting base + residual noise + 8-pixel
    // detector runs — the sweep's fits cells in miniature.
    let mut data = Vec::new();
    for _ in 0..36 {
        let mut card = b"SIMPLE  =                    T / synthetic FITS".to_vec();
        card.resize(80, b' ');
        data.extend_from_slice(&card);
    }
    let mut x: u64 = 0x243F_6A88_85A3_08D3;
    let mut base: i32 = 3000;
    while data.len() < 1_200_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        base += ((x >> 33) as i32 % 17) - 8;
        let run = (x >> 60) % 10 == 0;
        let n = if run { 8 } else { 1 };
        let v = (base + ((x >> 40) as i32 % 64) - 32).clamp(0, 32_000) as i16;
        for _ in 0..n {
            data.extend_from_slice(&v.to_be_bytes());
        }
    }
    std::env::set_var("OMNIZIP_ZSTD_SEQ_SPLIT", "1");
    let a = omnizip_zstd::compress(&data, omnizip_zstd::ZstdLevel::Default).unwrap();
    let b = omnizip_zstd::compress(&data, omnizip_zstd::ZstdLevel::Default).unwrap();
    assert_eq!(a, b, "env-split encode is not deterministic");
    let out = omnizip_zstd::decompress(&a, u32::MAX).unwrap();
    assert_eq!(out, data, "env-split frame failed to round-trip");
    std::env::remove_var("OMNIZIP_ZSTD_SEQ_SPLIT");
    let plain = omnizip_zstd::compress(&data, omnizip_zstd::ZstdLevel::Default).unwrap();
    eprintln!(
        "split={}B unsplit={}B (engaged: {})",
        a.len(),
        plain.len(),
        a.len() != plain.len()
    );
}
