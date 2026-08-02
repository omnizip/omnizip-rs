//! Encoder differential parity tests.
//!
//! Currently tests round-trip via our own decoder. The xz interop
//! path is documented as a known gap (TODO.complete/13) — the EOPM
//! marker encoding has a residual bit-pattern issue that xz rejects
//! but our decoder accepts.

#![forbid(unsafe_code)]

use omnizip_lzma::{lzma_alone_compress, lzma_alone_decompress};

#[test]
fn rust_encoder_round_trips_via_rust_decoder() {
    let inputs: &[&[u8]] = &[
        b"",
        b"a",
        b"hello world",
        &vec![0x42u8; 1000],
        &(0..2000).map(|i| (i % 251) as u8).collect::<Vec<u8>>(),
    ];
    for (i, input) in inputs.iter().enumerate() {
        let compressed = lzma_alone_compress(input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert_eq!(decompressed.as_slice(), *input, "input {i}: round-trip mismatch");
        eprintln!("input {i}: {} bytes -> {} bytes -> {} bytes (OK)",
                  input.len(), compressed.len(), decompressed.len());
    }
}
