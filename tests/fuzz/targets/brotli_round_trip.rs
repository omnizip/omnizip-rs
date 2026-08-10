//! Fuzz target: brotli round-trip must equal original.
//!
//! Run with:
//!   cargo fuzz run brotli_round_trip -- -max_total_time=60

#![no_main]

use omnizip_codecs::{Codec, CompressionLevel};

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let codec = omnizip_brotli::BrotliCodec::new();
    for &level in &[1u8, 5, 11] {
        let Ok(compressed) = codec.compress(data, CompressionLevel::new(level)) else {
            return;
        };
        if data.is_empty() {
            continue;
        }
        let Ok(decompressed) = codec.decompress(&compressed, data.len() as u32) else {
            panic!(
                "brotli level {} round-trip decode failed (input len {})",
                level,
                data.len()
            );
        };
        assert_eq!(
            decompressed, data,
            "brotli level {} round-trip mismatch (input len {})",
            level,
            data.len()
        );
    }
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
