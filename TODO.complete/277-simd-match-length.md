# 277 — SIMD-Accelerated match_length (perf gap to C reference)

- **Priority:** P0 (perf — 5-10x speedup needed to match C reference)
- **Crate:** `omnizip-codecs`, `omnizip-brotli`
- **Depends on:** [ADR-0001](../docs/adr/0001-pure-rust-only.md) (pure-Rust)
- **Estimated effort:** 5 days

## Problem

Real-world benchmarks show we're 5-47x slower than the vendored C
reference (0.14.20):

| Dataset | 0.14.20 (C) | 0.16.29 (Rust) | Gap |
|---------|-------------|----------------|-----|
| csv-synthetic (20 MB) | 0.37s | 3.4s | 9x |
| wav-synthetic | 0.12s | 0.54s | 4.5x |
| zeros | 0.15s | 0.27s | 1.8x |
| fits-synthetic | 3.68s | 6.30s | 1.7x |

Profiling shows 60-80% of CPU time is in `HashChainMatchFinder::find_match`
and the inner `match_length` byte-comparison loop. The C reference uses
SSE2/AVX2 SIMD intrinsics for the byte comparison; we use scalar `u64`
comparisons which are ~4-8x slower.

## Design

### std::simd implementation

Replace `match_length`'s `u64::from_le_bytes` chunked compare with
`std::simd::u8x16` SIMD compare:

```rust
fn match_length_simd(data: &[u8], a: usize, b: usize, max_len: u32) -> u32 {
    let mut len = 0;
    let max = max_len as usize;
    // 16-byte SIMD chunks
    while len + 16 <= max && a + len + 16 <= data.len() && b + len + 16 <= data.len() {
        let va = u8x16::from_slice(&data[a + len..a + len + 16]);
        let vb = u8x16::from_slice(&data[b + len..b + len + 16]);
        let eq = va.lanes_eq(vb);
        if eq.all() {
            len += 16;
        } else {
            // Find first mismatch via mask trailing zeros.
            let mask = eq.to_bitmask();
            len += mask.trailing_zeros() as usize;
            return len as u32;
        }
    }
    // Scalar tail for remaining bytes.
    while len < max && a + len < data.len() && b + len < data.len()
        && data[a + len] == data[b + len]
    {
        len += 1;
    }
    len as u32
}
```

### Portable fallback

`std::simd` is stable on Rust 1.75+ but may fall back to scalar on
platforms without SIMD. Provide an explicit scalar fallback:

```rust
#[cfg(target_feature = "sse2")]
fn match_length_simd(...) { /* SSE2 path */ }

#[cfg(not(target_feature = "sse2"))]
fn match_length_simd(...) { match_length_scalar(...) }
```

### Hot path audit

Other hot paths that would benefit from SIMD:
- `dict_hash::find_match` transformed-byte comparison (~200K entries)
- Brotli `build_symbol_stream` byte-by-byte iteration
- LZMA range coder byte loop

## Acceptance criteria

- [ ] `match_length_simd` implemented with scalar fallback.
- [ ] Benchmarked on x86_64 (SSE2) and ARM (NEON): ≥4x speedup
      over current scalar.
- [ ] No regression on platforms without SIMD (WASM, embedded).
- [ ] Workspace tests pass; round-trip integrity maintained.
- [ ] CSV-synthetic benchmark drops below 2s (currently 3.4s).

## Why this matters

Without SIMD, pure-Rust can't match C reference speed. The gap is
architectural, not algorithmic. SIMD closes most of it.

Alternative considered: re-introduce the vendored C reference as
`omnizip-brotli-vendored` crate for production use. Rejected because
it violates ADR-0001 and breaks WASM/embedded targets.
