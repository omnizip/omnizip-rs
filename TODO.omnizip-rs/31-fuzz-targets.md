# 31 — Fuzz targets

- **Priority:** P1 (catches panics on malformed input)
- **Depends on:** [03](03-conformance-corpus.md)
- **Estimated effort:** 3 days
- **Location:** `fuzz/`

## Goal

One `cargo-fuzz` target per codec decoder. Every fuzz target runs
continuously in CI (nightly) and on-demand. Catches panics, overflows,
and infinite loops on malformed input.

## Targets

```
fuzz/
├── Cargo.toml
├── fuzz_targets/
│   ├── lzma_decode.rs       # fuzz lzma2_decompress(arbitrary bytes)
│   ├── lzma_alone_decode.rs # fuzz .lzma legacy format
│   ├── xz_decode.rs         # fuzz XZ container
│   ├── zstd_decode.rs       # fuzz ZSTD frame
│   ├── deflate_decode.rs    # fuzz DEFLATE stream
│   ├── bzip2_decode.rs      # fuzz bzip2 stream
│   ├── ppmd7_decode.rs      # fuzz PPMd7 stream
│   └── bcj_filter.rs        # fuzz each BCJ variant
└── corpora/
    └── (seed from conformance corpus, task 03)
```

## Design

Each target calls the decoder on arbitrary bytes and asserts no panic:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Decoder must NEVER panic on malformed input. Errors are fine;
    // panics are bugs.
    let _ = omnizip_lzma::lzma2_decompress(data, u32::MAX);
});
```

For encoders, a separate target exercises encode-then-decode round-trips
on arbitrary input to catch encoder bugs that produce invalid streams:

```rust
fuzz_target!(|data: &[u8]| {
    if let Ok(compressed) = omnizip_lzma::lzma2_compress(data, LzmaLevel::default()) {
        let _ = omnizip_lzma::lzma2_decompress(&compressed, data.len() as u32);
    }
});
```

## CI integration

```yaml
# .github/workflows/fuzz.yml (nightly)
- cargo +nightly install cargo-fuzz
- for target in fuzz_targets/*; do
    cargo fuzz run "$target" -- -max_total_time=300
  done
- upload any crash artifacts
```

## Acceptance

- Every decoder has a fuzz target.
- Every encoder has a round-trip fuzz target.
- Nightly CI runs each target for 5 minutes minimum.
- A `corpora/` directory seeds each target with bytes from the conformance
  corpus (task 03) so fuzzing starts from realistic inputs.
- Clippy clean on fuzz target code.

## Implementation notes

- `cargo-fuzz` uses libFuzzer under the hood. Output is `data: &[u8]`.
- For decoders, the `expected_len` parameter can be arbitrary — don't pass
  a "correct" value; fuzz with `u32::MAX` and with `0` to exercise the
  length-validation path.
- Crash artifacts go in `fuzz/artifacts/<target>/` and are committed as
  regression tests once root-caused.
- See LimniFS's existing `fuzz/` setup (9 targets) for the pattern.
