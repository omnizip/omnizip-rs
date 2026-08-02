# 30 — Determinism verification

**Status**: ❌ Pending. Applies to every encoder task.

## What

A test harness that encodes the same input N=10 times and asserts
byte-identical output across runs.

## Why

LimniFS uses content-addressed storage (`DropId = BLAKE3(plaintext)`).
Codec non-determinism breaks dedup. This is a hard release blocker
(see CLAUDE.md invariant #2).

## Implementation

```rust
// tests/determinism.rs
#[test]
fn lzma_encode_is_deterministic() {
    let input = include_bytes!("../fixtures/lzma/good-1-v1.lz");
    let plaintext = lzma_decompress(input).unwrap();
    let mut outputs = Vec::new();
    for _ in 0..10 {
        outputs.push(lzma2_compress(&plaintext, LzmaLevel::default()).unwrap());
    }
    for w in outputs.windows(2) {
        assert_eq!(w[0], w[1], "LZMA encoder non-deterministic");
    }
}

#[test]
fn zstd_encode_is_deterministic() {
    // same structure for ZSTD
}
```

## Files

- `tests/determinism/lzma.rs`
- `tests/determinism/zstd.rs`
- Both in `tests/differential/Cargo.toml` as `[[test]]` targets.

## Acceptance

- Tests pass on linux + macOS + stable Rust with the same byte output.
- Test runs with various inputs (small, medium, large) all pass.
