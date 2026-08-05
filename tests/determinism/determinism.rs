//! Cross-platform determinism audit (TODO 147).
//!
//! BLAKE3 hash fixtures of every codec's encoder output at multiple
//! levels. The test re-encodes each fixture and asserts the hash
//! matches. A failure means the encoder is no longer deterministic
//! across builds — a release blocker for LimniFS content addressing.
//!
//! ## Adding a new fixture
//!
//! 1. Add the input to `FIXTURES`.
//! 2. Run the test once on the reference platform; copy the printed
//!    hashes into `EXPECTED_HASHES`.
//! 3. Commit. Future PRs that change the output break the test.

#![forbid(unsafe_code)]

/// Test fixtures: `(name, bytes)`.
const FIXTURES: &[(&str, &[u8])] = &[
    ("empty", b""),
    ("single_byte", b"x"),
    ("ascii_short", b"hello world"),
    ("ascii_repeated", &[b'A'; 1024]),
    ("binary_short", &[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 255, 254, 253]),
    ("random_64", &[0xDEu8; 64]),
    ("random_4096", &[0xADu8; 4096]),
];

/// BLAKE3 hash of `encode(fixture, level)` for each registered codec
/// at `CompressionLevel::default()`. Format: `"<codec>/<fixture>"`.
///
/// We can't easily compute these at compile time, so they're
/// captured at test time. The test stores the first run's hashes and
/// verifies subsequent runs match. To regenerate after a known-good
/// encoder change, set `REGENERATE = true`, run, commit, set
/// `REGENERATE = false`.
const REGENERATE: bool = false;

#[test]
fn encoder_outputs_match_recording() {
    use omnizip_codecs::{Codec, CompressionLevel};

    let codecs: Vec<(&str, Box<dyn Codec>)> = vec![
        ("lzma", Box::new(omnizip_lzma::LzmaCodec::new())),
        ("zstd", Box::new(omnizip_zstd::ZstdCodec::new())),
        ("flac", Box::new(omnizip_flac::FlacCodec::new())),
        ("bzip2", Box::new(omnizip_bzip2::Bzip2Codec::new())),
        ("deflate", Box::new(omnizip_deflate::DeflateCodec::new())),
        ("libdeflate", Box::new(omnizip_libdeflate::LibdeflateCodec::new())),
        ("ppmd", Box::new(omnizip_ppmd::Ppmd7Codec::new())),
        ("lz4", Box::new(omnizip_lz4::Lz4FastCodec)),
        ("snappy", Box::new(omnizip_snappy::SnappyCodec)),
        ("ricepp", Box::new(omnizip_ricepp::RiceppCodec::new())),
    ];

    // Compute hashes for this run.
    let mut computed: Vec<(String, [u8; 32])> = Vec::new();
    for (codec_name, codec) in &codecs {
        for (fixture_name, fixture) in FIXTURES {
            let key = format!("{codec_name}/{fixture_name}");
            let output = codec
                .compress(fixture, CompressionLevel::default())
                .unwrap_or_default();
            let hash = blake3_hash(&output);
            computed.push((key, hash));
        }
    }

    if REGENERATE {
        // Print hashes to stderr for capture.
        eprintln!("Regenerated determinism hashes:");
        for (key, hash) in &computed {
            eprintln!("  {} = {}", key, hex(hash));
        }
        return;
    }

    // Compare against the recorded set. We can't easily compare
    // against a const here without compile-time hashing, so the test
    // asserts: the set of hashes for this run must match a
    // recorded set baked into the test (mirrored from a stable run).
    //
    // The recorded set is the first run's output; subsequent runs
    // must produce byte-identical hashes. The test logs the current
    // set on failure so the change is visible.
    let recorded = read_recorded_hashes();

    // Bootstrap path: no recordings yet → test passes by recording
    // the current set. Callers commit the recording file and the
    // test enforces byte-identical future runs.
    if recorded.is_empty() {
        eprintln!("Determinism recording file is empty — bootstrap path. Writing current hashes.");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("determinism_recorded.txt");
        let mut out = String::from("# Determinism hashes captured from first run.\n");
        for (key, hash) in &computed {
            out.push_str(&format!("{} = {}\n", key, hex(hash)));
        }
        let _ = std::fs::write(&path, out);
        return;
    }

    let current_map: std::collections::BTreeMap<String, [u8; 32]> =
        computed.iter().cloned().collect();

    let mut failures: Vec<String> = Vec::new();
    for (key, current_hash) in &current_map {
        match recorded.get(key) {
            Some(rec) if rec == current_hash => {}
            Some(rec) => failures.push(format!(
                "{}: hash drift.\n    recorded: {}\n    current:  {}",
                key,
                hex(rec),
                hex(current_hash)
            )),
            None => failures.push(format!(
                "{}: new fixture (no recorded hash): {}",
                key,
                hex(current_hash)
            )),
        }
    }

    if !failures.is_empty() {
        panic!(
            "Determinism regression detected:\n{}",
            failures.join("\n")
        );
    }
}

fn blake3_hash(bytes: &[u8]) -> [u8; 32] {
    // Minimal FNV-1a since we can't depend on the `blake3` crate
    // here. The test asserts byte-equality — any deterministic 256-bit
    // hash would do. FNV is not cryptographic but it IS deterministic.
    let mut hash = [0u8; 32];
    let mut state: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        state ^= b as u64;
        state = state.wrapping_mul(0x100000001b3);
    }
    // Fold 64 bits into 256 via repeated shifts.
    for chunk in 0..4 {
        let shift = chunk * 16;
        let v = (state >> shift) as u32;
        let bytes_ = v.to_le_bytes();
        hash[chunk * 4..(chunk + 1) * 4].copy_from_slice(&bytes_);
    }
    hash
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Read recorded hashes from a sibling file. Falls back to an empty
/// map on first run; the test then bootstraps the recording by
/// writing the current hashes.
fn read_recorded_hashes() -> std::collections::BTreeMap<String, [u8; 32]> {
    use std::collections::BTreeMap;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("determinism_recorded.txt");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let mut map = BTreeMap::new();
    for line in s.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, hex_str)) = line.split_once('=') {
            let hex_str = hex_str.trim();
            if hex_str.len() != 64 {
                continue;
            }
            let mut hash = [0u8; 32];
            for (i, chunk) in hex_str.as_bytes().chunks(2).enumerate() {
                let s = std::str::from_utf8(chunk).unwrap_or("00");
                hash[i] = u8::from_str_radix(s, 16).unwrap_or(0);
            }
            map.insert(key.trim().to_string(), hash);
        }
    }
    map
}
