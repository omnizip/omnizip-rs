# Determinism cross-platform audit

LimniFS requires byte-identical encoder output across runs,
machines, and Rust versions. A single platform-dependent
operation in an encode path would break content addressing
(`DropId = BLAKE3(plaintext)`).

This document audits every source of cross-platform risk in the
workspace and the test infrastructure that catches regressions.

## Sources of cross-platform risk

| Source | Risk | Mitigation |
|--------|------|------------|
| `f64` arithmetic | Low — IEEE 754 round-to-nearest is standard | None needed (all f64 ops are `+`, `-`, `*`, `sqrt`, `sin`, `cos`, `log2`, `exp`) |
| `HashMap` iteration | **Medium** — different hash seeds yield different order | Use `BTreeMap` everywhere deterministic order matters |
| `DefaultHasher` | Low — only used in tests, not encode paths | Audit `find_hasher` usage |
| `Instant::now()` seeding | Low — only used in tests for timing | No Instant usage in encode paths |
| `std::simd` | High — not on stable | We use `wide` which is portable |
| Random number generation | None — no RNGs in encode paths | Verified by audit |
| Floating-point rounding mode | Low — default is round-to-nearest | No `fsetround` calls; audit confirms |
| Sort stability | Low | `Vec::sort` is stable since 1.55 |
| Filesystem | None | Encoders operate on byte slices, not files |
| Locale | None | No `printf`-style formatting in encode paths |

## Specific audit results

### `f64` operations
All `f64` operations across codecs use only the standard IEEE 754
arithmetic operators + `f64::sqrt`, `f64::log2`, `f64::sin`, `f64::cos`,
`f64::exp`, `f64::round`, `f64::abs`. None use `f64::powi`, `f64::sin_cos`,
or platform-specific functions.

Grep audit:
```
grep -rn 'powi\|sin_cos\|fma\|remainder' omnizip-*/src/
```
Result: 0 hits.

### `HashMap` usage in encode paths
- `omnizip-ppmd`: `HashMap<u32, u16>` for next-byte frequencies. We
  explicitly use a BTree-equivalent in the encode path via
  `bit_aggregate` (TODO 118 fix). Verified.
- `omnizip-zpaq`: `HashMap` used only for model aggregation. The
  encode path uses a sorted-vec representation in the mixer.
- `omnizip-brotli`: `HashMap` used only in tests, not encode paths.

### `Instant` and timing
Zero usage in encode paths. Used in `omnizip-bench` only.

### Bit packing / endianness
All bit writers use LSB-first packing into `u64`/`u32` buffers with
explicit masks. Endianness conversion via `u64::from_le_bytes` /
`to_le_bytes` is documented. No implicit endianness anywhere.

## Test infrastructure

### Cross-platform determinism test

`omnizip-codecs/src/determinism.rs` ships 20 fixture inputs (text,
binary, periodic, random) with their BLAKE3 hashes of encoder
output. The hash fixtures are committed to the repo.

The test re-encodes each fixture at every level and asserts the
output hash matches. A failure means we shipped a non-deterministic
change.

### CI matrix
TODO 150: `.github/workflows/ci.yml` runs on
`ubuntu-latest` + `macos-latest` + `windows-latest` × stable Rust.
A failure on any platform is a determinism regression.

### Manual audit
The above tables are reviewed by hand every major release. See the
`omnizip-ppmd/src/ppmd7/model.rs` `reset()` implementation as an
example of how per-codec reset methods must rebuild their adaptation
state deterministically.
