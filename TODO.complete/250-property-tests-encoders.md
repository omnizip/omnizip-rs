# 250 — Property-Based Testing for All Encoders

- **Priority:** P1 (correctness — exceeds hand-written test coverage)
- **Crate:** workspace (`tests/proptest/`)
- **Depends on:** [247](247-real-world-test-corpora.md) (uses real data shapes)
- **Estimated effort:** 3 days

## Problem

Existing tests use hand-picked inputs: "the quick brown fox", CSV
rows, random byte sequences. Each test verifies ONE input. Bugs in
edge cases (empty input, single byte, 1 MiB boundary, all-same-byte,
alternating-byte patterns) are caught only when someone thinks to
add a test.

Hand-written tests are:
- **Sparse**: 86 brotli tests cover ~150 inputs. Real corpus has
  billions of meaningful inputs.
- **Repetitive**: each codec repeats the same round-trip pattern
  with different inputs. DRY violation.
- **Brittle**: a test that passes today might fail under a refactor
  because the assertion is too specific.

## Design

### Strategy: `proptest` with structured generators

Use the `proptest` crate (already a dev-dep). Generate structured
inputs that exercise meaningful shapes:

```rust
/// Generates inputs that match real-world data shapes.
pub fn realistic_input() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Empty input
        Just(Vec::new()),
        // Single byte
        any::<u8>().prop_map(|b| vec![b]),
        // Short text (English-like)
        r"[a-zA-Z0-9 ,.!?]{0,256}".prop_map(|s| s.into_bytes()),
        // CSV-like
        csv_stream(1..100),
        // JSON-like
        json_stream(),
        // Highly repetitive
        repetitive_byte_stream(),
        // Random binary
        prop::collection::vec(any::<u8>(), 0..8192),
        // Large text
        r"[a-z ]{0,65536}".prop_map(|s| s.into_bytes()),
    ]
}

fn csv_stream(rows: impl Strategy<Value = usize>) -> impl Strategy<Value = Vec<u8>> {
    rows.prop_flat_map(|n| {
        prop::collection::vec(
            r"[a-z_]+,(0|[1-9][0-9]*),[a-z]+\n",
            n,
        ).prop_map(|rows| rows.concat().into_bytes())
    })
}
```

### Invariant-based testing

For each codec + each input shape, verify invariants hold:

```rust
proptest! {
    #[test]
    fn brotli_round_trip(input in realistic_input()) {
        let codec = BrotliCodec::new();
        for level in [1, 5, 11] {
            let compressed = codec.compress(&input, CompressionLevel::new(level))?;
            let decompressed = codec.decompress(&compressed, input.len() as u32)?;
            prop_assert_eq!(decompressed, input, "round-trip failed at level {}", level);
        }
    }

    #[test]
    fn brotli_deterministic(input in realistic_input(), seed in any::<u64>()) {
        // Run twice with same input + level, must produce byte-identical output.
        let codec = BrotliCodec::new();
        let a = codec.compress(&input, CompressionLevel::new(5))?;
        let b = codec.compress(&input, CompressionLevel::new(5))?;
        prop_assert_eq!(a, b);
    }

    #[test]
    fn brotli_monotonic_ratio(input in realistic_input()) {
        // Higher quality should never produce larger output.
        let codec = BrotliCodec::new();
        let q1 = codec.compress(&input, CompressionLevel::new(1))?;
        let q11 = codec.compress(&input, CompressionLevel::new(11))?;
        prop_assert!(q11.len() <= q1.len() + 32, // allow tiny overhead
            "q11 ({}) > q1 ({}) + 32", q11.len(), q1.len());
    }

    #[test]
    fn brotli_cross_decoder(input in realistic_input()) {
        // Encode via Rust, decode via vendored C `brotli -d`.
        // Catches wire-format bugs our own decoder would miss.
        let codec = BrotliCodec::new();
        let compressed = codec.compress(&input, CompressionLevel::new(5))?;
        let decoded = brotli_cli_decode(&compressed)?;
        prop_assert_eq!(decoded, input);
    }
}
```

### Per-codec test files

```
tests/proptest/
├── brotli.rs
├── zstd.rs
├── lzma.rs
├── lz4.rs
├── deflate.rs
├── libdeflate.rs
├── snappy.rs
├── bzip2.rs
├── ppmd.rs
└── shared.rs   // realistic_input() and helpers
```

### CI integration

Run proptests in CI with `PROPTEST_CASES=1024` (default is 256).
On failure, the failing case is shrunk and persisted to
`proptest-regressions/` for reproducibility.

## Acceptance criteria

- [ ] All 10 codec test files exist with at least round-trip,
      determinism, monotonic ratio tests.
- [ ] `cargo test --test proptest --workspace` passes 1000+ cases
      per codec.
- [ ] At least 3 latent bugs found and fixed via proptest (typical
      for first adoption).
- [ ] `proptest-regressions/` gitignored.
- [ ] CI runs proptests on every PR.

## Why this matters

Property tests catch bugs that hand-written tests miss:
- Empty input edge case
- 1-byte input edge case
- All-same-byte patterns
- Inputs that trigger specific algorithm paths
- Race conditions in reused compressor state

With 1000 cases per codec per PR, we get coverage far beyond what
hand-written tests provide. This is the single highest-value testing
investment.
