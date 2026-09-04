//! Deterministic malformed-input smoke gate (TODO.remaining/17).
//!
//! Every PR runs this: seeded mutation of valid compressed streams
//! (bit flips, byte substitutions, truncations, splices, range
//! zeroing) plus pure-random inputs, fed to every decoder. A decoder
//! may return any error; it must NEVER panic. Cases run under
//! `catch_unwind` so one failure reports the whole batch, and the
//! seed reproduces the exact failing case.
//!
//! This is the stable-toolchain complement to `tests/fuzz/`
//! (cargo-fuzz/libFuzzer, nightly CI): the smoke gate catches the
//! panic class on every PR; the libFuzzer targets add depth.

use omnizip_codecs::{Codec, CompressionLevel};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Deterministic PCG-style generator — no external deps, identical
/// sequence on every platform.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(
            seed.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407),
        )
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 ^ (self.0 >> 29)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fixture classes chosen to stress different decoder paths:
/// text (context modeling), periodic structure (long matches),
/// binary (incompressible-ish), and zeros (run-length extremes).
fn fixtures() -> Vec<Vec<u8>> {
    let mut text = Vec::new();
    for i in 0..1024 {
        text.extend_from_slice(format!("row {i}: alpha beta gamma delta {i} epsilon\n").as_bytes());
    }
    let mut periodic = Vec::new();
    for i in 0..2048u32 {
        let (m, d) = (i % 100, i / 7);
        periodic.extend_from_slice(format!("{i},{m},{d}\n").as_bytes());
    }
    let binary: Vec<u8> = (0..8_000u32)
        .map(|i| ((i.wrapping_mul(2654435761) >> 19) & 0xFF) as u8)
        .collect();
    let zeros = vec![0u8; 4_000];
    vec![text, periodic, binary, zeros]
}

fn mutate(data: &mut Vec<u8>, rng: &mut Rng) {
    if data.is_empty() {
        return;
    }
    let len = data.len();
    match rng.below(6) {
        0 => {
            let i = rng.below(len);
            data[i] ^= 1u8 << (rng.below(8));
        }
        1 => {
            data[rng.below(len)] = (rng.next() & 0xFF) as u8;
        }
        2 => {
            data.truncate(rng.below(len));
        }
        3 => {
            // Splice a copy of a slice over another region — keeps the
            // stream mostly-valid while desynchronizing any length or
            // offset arithmetic.
            let a = rng.below(len);
            let b = rng.below(len);
            let (from, to) = if a < b { (a, b) } else { (b, a) };
            let span = rng.below(len - to).min(64);
            for k in 0..span {
                data[to + k] = data[from + k];
            }
        }
        4 => {
            let start = rng.below(len);
            let span = rng.below(len - start).min(128);
            for b in &mut data[start..start + span] {
                *b = 0;
            }
        }
        _ => {
            // Corrupt a run of consecutive bytes at the head — header
            // fields carry lengths, flags, and table counts.
            let span = rng.below(len.min(24));
            for k in 0..span {
                data[k] = (rng.next() & 0xFF) as u8;
            }
        }
    }
}

/// Run `cases` mutated decodes of `valid` plus a few random inputs.
/// Returns the failing case descriptors (empty = clean).
fn run_batch(
    codec_name: &str,
    valid: &[u8],
    decode: impl Fn(&[u8]) -> Result<Vec<u8>, omnizip_codecs::OmnizipError>,
    seed_base: u64,
    cases: usize,
) -> Vec<String> {
    let mut failures = Vec::new();
    for case in 0..cases {
        let seed = seed_base.wrapping_add(case as u64 * 7919);
        let mut input = valid.to_vec();
        mutate(&mut input, &mut Rng::new(seed));
        let desc = format!("{codec_name} seed={seed} len={}", input.len());
        let r = catch_unwind(AssertUnwindSafe(|| decode(&input).is_ok()));
        if r.is_err() {
            failures.push(desc);
        }
    }
    failures
}

fn random_batch(
    codec_name: &str,
    decode: impl Fn(&[u8]) -> Result<Vec<u8>, omnizip_codecs::OmnizipError>,
    seed_base: u64,
) -> Vec<String> {
    let mut failures = Vec::new();
    for case in 0..8u64 {
        let mut rng = Rng::new(seed_base.wrapping_add(case * 104729));
        let len = 1 + rng.below(4096);
        let input: Vec<u8> = (0..len).map(|_| (rng.next() & 0xFF) as u8).collect();
        let desc = format!("{codec_name} random seed={seed_base} case={case}");
        if catch_unwind(AssertUnwindSafe(|| decode(&input).is_ok())).is_err() {
            failures.push(desc);
        }
    }
    failures
}

/// One codec's full gate: encode every fixture at `levels`, mutate,
/// decode. `cases` per (fixture, level) pair.
fn gate<C: Codec>(name: &str, codec: &C, levels: &[u8], cases: usize) -> Vec<String> {
    let mut failures = Vec::new();
    for (fi, fixture) in fixtures().iter().enumerate() {
        for &lv in levels {
            let Ok(valid) = codec.compress(fixture, CompressionLevel::new(lv)) else {
                failures.push(format!("{name} encode fixture={fi} level={lv} FAILED"));
                continue;
            };
            let seed = 0x5EED_0000 + (fi as u64) * 1009 + u64::from(lv) * 101;
            failures.extend(run_batch(
                name,
                &valid,
                |data| codec.decompress(data, fixture.len() as u32),
                seed,
                cases,
            ));
        }
    }
    failures.extend(random_batch(
        name,
        |data| codec.decompress(data, u32::MAX),
        0x5EED_FFFF ^ name.len() as u64,
    ));
    failures
}

#[test]
fn no_decoder_panics_on_malformed_input() {
    let mut all = Vec::new();
    all.extend(gate(
        "brotli",
        &omnizip_brotli::BrotliCodec::new(),
        &[1, 5],
        16,
    ));
    all.extend(gate("zstd", &omnizip_zstd::ZstdCodec::new(), &[1, 6], 16));
    all.extend(gate("xz", &omnizip_lzma::LzmaCodec::new(), &[1], 16));
    all.extend(gate("deflate", &omnizip_deflate::DeflateCodec, &[1, 6], 24));
    all.extend(gate("bzip2", &omnizip_bzip2::Bzip2Codec, &[1, 9], 16));
    all.extend(gate("lz4", &omnizip_lz4::Lz4FastCodec, &[1, 9], 16));
    all.extend(gate("snappy", &omnizip_snappy::SnappyCodec, &[1], 24));
    // Container-reachable and remaining decoders (task 22). The
    // statistical/slow codecs (zpaq, glza, ppmd) get fewer cases —
    // their encoders dominate the gate's runtime.
    all.extend(gate("ppmd7", &omnizip_ppmd::Ppmd7Codec, &[1], 8));
    all.extend(gate("ppmd8", &omnizip_ppmd::Ppmd8Codec, &[1], 8));
    all.extend(gate(
        "deflate64",
        &omnizip_deflate64::Deflate64Codec,
        &[1, 6],
        16,
    ));
    all.extend(gate("flac", &omnizip_flac::FlacCodec, &[1], 8));
    all.extend(gate("fsst", &omnizip_fsst::FsstCodec, &[1], 16));
    all.extend(gate("glza", &omnizip_glza::GlzaCodec, &[1], 8));
    all.extend(gate("zpaq", &omnizip_zpaq::ZpaqCodec, &[1], 4));
    all.extend(gate("ricepp", &omnizip_ricepp::RiceppCodec::new(), &[1], 8));
    all.extend(gate("blosc", &omnizip_blosc::BloscCodec, &[1], 16));
    assert!(
        all.is_empty(),
        "decoders panicked on {} malformed inputs:\n{}",
        all.len(),
        all.join("\n")
    );
}
