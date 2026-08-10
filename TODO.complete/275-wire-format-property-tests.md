# 275 — Wire-Format Property Tests with Reference Decoders

- **Priority:** P1 (correctness — cross-decoder parity)
- **Crate:** workspace (`tests/property/`)
- **Depends on:** [250](250-property-tests-encoders.md), [247](247-real-world-test-corpora.md)
- **Estimated effort:** 2 days

## Problem

Current property tests verify round-trip via OUR encoder + OUR decoder.
This catches internal inconsistencies but misses:

- Our encoder produces bytes that `brotli -d` (C reference) rejects.
- Our decoder rejects bytes that `brotli` CLI produces.

These are wire-format bugs. The current "DECODE-FAIL" lines in
`brotli_benchmark.rs` are exactly this — they need to be zero.

## Design

### Reference CLI subprocess

For each codec, install the reference implementation as a CLI:

```bash
# Tested via CI image
apt install -y brotli xz-utils zstd lz4
```

### Property test using reference decoders

```rust
proptest! {
    #[test]
    fn brotli_encodes_to_cross_decoder(input in arbitrary_bytes()) {
        let our_compressed = omnizip_brotli::BrotliCodec::new()
            .compress(&input, CompressionLevel::new(5))?;
        // Spawn `brotli -d`, feed our_compressed, check output == input.
        let cli_decoded = run_cli("brotli", &["-d"], &our_compressed)?;
        prop_assert_eq!(cli_decoded, input);
    }
}
```

### Reference encoder → our decoder

```rust
proptest! {
    #[test]
    fn our_decoder_accepts_reference_encoder(input in arbitrary_bytes()) {
        let ref_compressed = run_cli("brotli", &["-qf"], &input)?;
        let our_decoded = omnizip_brotli::BrotliCodec::new()
            .decompress(&ref_compressed, input.len() as u32)?;
        prop_assert_eq!(our_decoded, input);
    }
}
```

### CI integration

CI runs these tests on Linux where CLI tools are available. macOS /
Windows skip them (the property tests still run for internal parity).

## Acceptance criteria

- [ ] `brotli -d` accepts 99%+ of our encoder output (proptest).
- [ ] Our decoder accepts 99%+ of `brotli -qf` output.
- [ ] Same for zstd, lz4, xz.
- [ ] Failing cases persisted to `proptest-regressions/` for fixing.

## Why this matters

Until our wire format is verified by external tools, "Brotli" /
"ZSTD" / "LZMA" claims are aspirational. Cross-decoder proptest is
the gold standard.
