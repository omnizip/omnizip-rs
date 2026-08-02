# 31 — Encoder differential parity

**Status**: ❌ Pending. Depends on all encoder tasks.

## What

For each fixture: encode via both Rust and the reference encoder
(`xz -9` / `zstd -19`), then decode both outputs through the
reference decoder (`xz -d` / `zstd -d`) and assert byte-identical
decompressed bytes.

## Why

The differential gate (CLAUDE.md) requires byte-identical encoder
output for the same input + level across Rust and Ruby. Encoder
parity testing is the gate.

## Implementation

```rust
// tests/differential/tests/encoder_parity.rs
#[test]
fn lzma_encoder_parity() {
    for fixture in fixtures() {
        let plaintext = fs::read(fixture).unwrap();
        let rust_compressed = xz_compress(&plaintext, LzmaLevel::default()).unwrap();
        let rust_roundtrip = xz_decompress(&rust_compressed).unwrap();
        assert_eq!(rust_roundtrip, plaintext, "round-trip failed");
    }
}
```

## Files

- `tests/differential/tests/encoder_parity_lzma.rs`
- `tests/differential/tests/encoder_parity_zstd.rs`

## Acceptance

- All `tests/fixtures/lzma/` fixtures encode → decode to original.
- All `tests/fixtures/zstd/` fixtures encode → decode to original.
- Tests pass on CI (linux + macOS, stable Rust).
