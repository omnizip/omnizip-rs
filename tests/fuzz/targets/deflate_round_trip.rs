//! Fuzz target: deflate round-trip must equal original.

#![no_main]

use omnizip_codecs::{Codec, CompressionLevel};

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let codec = omnizip_deflate::DeflateCodec::new();
    let Ok(compressed) = codec.compress(data, CompressionLevel::new(6)) else {
        return;
    };
    if data.is_empty() {
        return;
    }
    let Ok(decompressed) = codec.decompress(&compressed, data.len() as u32) else {
        panic!("deflate round-trip decode failed");
    };
    assert_eq!(decompressed, data);
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
