//! Fuzz target: lzma round-trip must equal original.

#![no_main]

use omnizip_codecs::{Codec, CompressionLevel};

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let codec = omnizip_lzma::LzmaCodec::new();
    for &level in &[1u8, 5, 9] {
        let Ok(compressed) = codec.compress(data, CompressionLevel::new(level)) else {
            return;
        };
        if data.is_empty() {
            continue;
        }
        let Ok(decompressed) = codec.decompress(&compressed, data.len() as u32) else {
            panic!(
                "lzma level {} round-trip decode failed (input len {})",
                level,
                data.len()
            );
        };
        assert_eq!(decompressed, data);
    }
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
