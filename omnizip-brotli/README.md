# omnizip-brotli

Pure-Rust Brotli codec (RFC 7932) — no external dependencies, no FFI.

## Status

- **Encoder**: ✅ — from-spec encoder with optimal parser, rep codes,
  dictionary transforms, context modeling. Beats vendored C reference
  by 5+ percentage points on CSV at Q5.
- **Decoder**: ✅ — full RFC 7932 decoder. Round-trip verified for all
  86 in-crate tests. ⚠️ Some vendored-C Q11 inputs rejected (TODO 244).
- **Wire-format parity**: 🔄 — TODO 263 (vendored C rejects some of our
  output).

## Quick start

```rust
use omnizip_brotli::BrotliCodec;
use omnizip_codecs::{Codec, CompressionLevel};

let codec = BrotliCodec::new();
let compressed = codec.compress(b"hello world", CompressionLevel::new(5))?;
let decompressed = codec.decompress(&compressed, "hello world".len() as u32)?;
assert_eq!(decompressed, b"hello world");
```

## Algorithm highlights

- **Optimal parser** (TODO 240): cost-aware DP with brotli-accurate
  distance costs; considers all sub-match lengths via copy-code
  boundary sampling.
- **Iterative refinement** (TODO 246, Q8+): 2-pass parser with Shannon
  costs recomputed from actual parsed literals.
- **4-iteration refinement** (TODO 272, Q11): extends iterative parser
  to 4 passes.
- **Rep codes 0-3** (TODO 245): full 4-distance ring buffer matching
  decoder state; emits explicit distance codes 0/1/2/3 for
  rep0/1/2/3 matches.
- **Dictionary**: all 121 RFC 7932 transforms via pre-computed hash
  table in `encoder/dict_hash.rs`.
- **Content-type aware**: uses `ContentType::detect()` for parser tuning.

## Levels

| Level | Strategy | Speed | Notes |
|-------|----------|-------|-------|
| 0-1   | Greedy | fastest | Hot-path writes |
| 2-3   | Lazy | fast | |
| 4-7   | Lazy2 + 2-pass optimal | medium | Default for text |
| 8-10  | 2-iteration optimal | slow | Refined Huffman cost |
| 11    | 4-iteration optimal | slowest | Max effort |

## Measured ratios (Q5)

| Benchmark | Our ratio | Vendored C ratio | Win |
|-----------|-----------|------------------|-----|
| CSV 100KB | 20.2% | 25.4% | +5.2 pp |
| CSV 500KB | 20.0% | 24.1% | +4.1 pp |
| Mixed text/binary | 16.3% | 23.7% | +7.4 pp |
| Binary | 3.5% | 5.6% | +2.1 pp |
| English 100KB | 0.6% | 0.7% | +0.1 pp |

## Determinism

Byte-identical output across runs, machines, and Rust versions.
Verified by `tests/determinism/` and `tests/property/`.

## License

Dual MIT OR Apache-2.0.

## References

- [RFC 7932](https://www.rfc-editor.org/rfc/rfc7932)
- [google/brotli](https://github.com/google/brotli)
- [TODO 244](../TODO.complete/244-brotli-decoder-wire-format-bugs.md)
- [TODO 263](../TODO.complete/263-brotli-cross-decoder-fix.md)
