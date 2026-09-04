//! Fuzz target: decoder must never panic on arbitrary input.
#![no_main]
fn decode(data: &[u8]) -> Result<Vec<u8>, omnizip_codecs::OmnizipError> {
    omnizip_bzip2::Bzip2Codec.decompress(data, u32::MAX)
}

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = decode(data);
});

#[cfg(not(fuzzing))]
fn main() {
    eprintln!("This binary is a fuzz target. Build with --cfg fuzzing and cargo-fuzz.");
}
