//! Fuzz target: brotli decoder must never panic on arbitrary input.
//!
//! Round-trip is one thing; decoder safety against malformed input is
//! another. This target feeds random bytes to the decoder and asserts
//! no panic. Errors are fine; crashes are not.

#![no_main]

use omnizip_codecs::Codec;

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let codec = omnizip_brotli::BrotliCodec::new();
    // Errors are fine; we just check no panic.
    let _ = codec.decompress(data, 65536);
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
