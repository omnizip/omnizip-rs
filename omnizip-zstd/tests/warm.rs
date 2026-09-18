//! Standalone-decodability and determinism of `compress_warm`.

use omnizip_zstd::{compress, compress_warm, decompress, ZstdLevel};

fn corpus(n: usize, seed: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    let mut s = seed;
    while v.len() < n {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.extend_from_slice(&s.to_le_bytes());
    }
    v.truncate(n);
    v
}

#[test]
fn warm_frames_decode_standalone_at_every_wired_level() {
    for level in [
        ZstdLevel::Fastest,
        ZstdLevel::Fast,
        ZstdLevel::Default,
        ZstdLevel::Better,
    ] {
        let window = corpus(40 * 1024, 0xA11CE);
        let input = corpus(120 * 1024, 0xB0B);
        let frame = compress_warm(&window, &input, level).expect("warm encode");
        let decoded =
            decompress(&frame, (window.len() + input.len()) as u32).expect("STANDALONE decode");
        let mut want = window.clone();
        want.extend_from_slice(&input);
        assert_eq!(decoded, want, "level {level:?}");
    }
}

#[test]
fn warm_crushes_input_that_repeats_the_window() {
    let window = corpus(64 * 1024, 0x5EED);
    // Input repeats the window verbatim: a warm encoder must turn
    // it into back-references. The honest comparison is per-chunk:
    // today LimniFS pays compress(input) alone (input looks random
    // to a cold encoder); warm pays the raw window + a crushed
    // input region.
    let input = window.clone();
    let warm = compress_warm(&window, &input, ZstdLevel::Fastest).expect("warm");
    let cold_per_chunk = compress(&input, ZstdLevel::Fastest).expect("cold per chunk");
    let input_region = warm.len() - window.len();
    assert!(
        input_region < cold_per_chunk.len() / 4,
        "warm input region {input_region} vs cold per-chunk {} — warming must crush repeats",
        cold_per_chunk.len()
    );
}

#[test]
fn warm_encoding_is_deterministic() {
    let window = corpus(32 * 1024, 1);
    let input = corpus(200 * 1024, 2);
    let a = compress_warm(&window, &input, ZstdLevel::Fastest).expect("a");
    let b = compress_warm(&window, &input, ZstdLevel::Fastest).expect("b");
    assert_eq!(a, b);
}

#[test]
fn empty_window_and_empty_input_edges() {
    let data = corpus(16 * 1024, 3);
    let no_window = compress_warm(&[], &data, ZstdLevel::Fastest).expect("empty window");
    assert_eq!(decompress(&no_window, data.len() as u32).expect("d"), data);
    let window_only = compress_warm(&data, &[], ZstdLevel::Fastest).expect("empty input");
    assert_eq!(
        decompress(&window_only, data.len() as u32).expect("d"),
        data
    );
}
