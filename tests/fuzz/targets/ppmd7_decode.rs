//! Fuzz target: decoder must never panic on arbitrary input.
#![no_main]

use omnizip_codecs::Codec;

fn decode(data: &[u8]) -> Result<Vec<u8>, omnizip_codecs::OmnizipError> {
    omnizip_ppmd::Ppmd7Codec.decompress(data, u32::MAX)
}

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = decode(data);
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
