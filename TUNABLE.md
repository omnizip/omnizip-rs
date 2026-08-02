# Codec Tunability Reference

Every codec in omnizip-rs accepts the standard `CompressionLevel` (u8) via the `Codec` trait. Some codecs expose **additional** tunables for power users who need finer control over the speed/ratio/memory trade-off.

This document catalogues what's tunable per codec, with examples.

---

## Quick reference

| Codec        | Level | Memory Budget | Algorithm-Specific Options       |
|--------------|:-----:|:-------------:|----------------------------------|
| omnizip-blosc | ✓    | —             | `BloscCodec` selects shuffle + compressor internally |
| omnizip-brotli | ✓   | —             | `default_quality()`                                              |
| omnizip-bzip2 | ✓    | —             | (900 KB block size, fixed)                                       |
| omnizip-deflate | ✓  | —             | (miniz_oxide backend, fixed)                                     |
| omnizip-deflate64 | ✓ | —            | `MIN_LEVEL`, `MAX_LEVEL`                                         |
| omnizip-flac | —     | —             | `PcmParams { sample_rate, channels, bits_per_sample }`           |
| omnizip-fsst | —     | —             | (no options; learns symbol table adaptively)                     |
| omnizip-glza | —     | —             | `compress_with_version(1 | 2)` (raw vs Huffman)                  |
| omnizip-lz4  | ✓    | —             | `Lz4FastCodec` vs `Lz4HcCodec`                                   |
| omnizip-lzma | ✓    | —             | `LzmaLevel(u8)`                                                  |
| **omnizip-ppmd (PPMd7)** | ✓ | **✓ 80 MB default** | `compress_with_budget(_, _, bytes)`, `PpmModel::with_memory_budget(order, bytes)` |
| omnizip-ppmd (PPMd8) | ✓ | —             | `Ppmd8Model::new(order, restore_method, max_nodes)`              |
| omnizip-ricepp | ✓  | —             | `CodecConfig { block_size, bytes_per_sample }`                   |
| omnizip-snappy | —   | —             | (no options; Snappy is fixed-parameter)                          |
| omnizip-zpaq | ✓    | —             | (level maps to context-mixing model selection)                   |
| omnizip-zstd | ✓    | —             | `ZstdLevel`, `compress_with_dict(_, _, &dict)`                   |

---

## Detailed examples

### PPMd7 — user-tunable memory budget

PPMd7 is the most configurable codec in the workspace. The context trie's arena size scales linearly with the memory budget; more memory = more contexts tracked = better ratio.

```rust
use omnizip_ppmd::ppmd7;

// Default: 80 MB budget.
let bytes = ppmd7::compress(b"hello world", 4)?;

// Custom budget: 16 MB (smaller footprint, slightly worse ratio).
let bytes = ppmd7::compress_with_budget(b"hello world", 4, 16 * 1024 * 1024)?;

// Custom budget: 256 MB (best ratio, more memory).
let bytes = ppmd7::compress_with_budget(b"hello world", 4, 256 * 1024 * 1024)?;
```

The `PpmModel::with_memory_budget(max_order, bytes)` constructor is available for callers who want to drive the model directly.

**Rule of thumb**: 1 MB of budget ≈ 12 500 contexts tracked. For text inputs, ~50 MB is plenty; for源 code or mixed workloads, 100–200 MB improves ratio.

### PPMd8 — restoration method and node budget

```rust
use omnizip_ppmd::ppmd8::{Ppmd8Model, RESTORE_METHOD_RESTART, RESTORE_METHOD_CUT_OFF};

// Default: 1.6M node budget (~64 MB), RESTART restoration.
let model = Ppmd8Model::default_for(6);

// Custom: smaller budget, CUT_OFF restoration (preserves high-glue contexts).
let model = Ppmd8Model::new(6, RESTORE_METHOD_CUT_OFF, 100_000);
```

### LZMA — explicit level

```rust
use omnizip_lzma::{lzma2_compress, LzmaLevel};

// LzmaLevel is a u8 newtype; 0..=9 matching xz-utils presets.
let bytes = lzma2_compress(b"data", LzmaLevel::new(6))?;
```

(Per-call `lc`, `lp`, `pb`, and dictionary size are accepted by the encoder internals but not yet exposed at the top-level API — Phase 2 work.)

### ZSTD — level and dictionary

```rust
use omnizip_zstd::{compress, ZstdLevel, compress_with_dict};

// Level 1..=22, matching the upstream zstd CLI.
let bytes = compress(b"data", ZstdLevel::new(19))?;

// Train a dictionary on representative samples, then use it.
let dict = std::fs::read("dict.bin")?;
let bytes = compress_with_dict(b"data", ZstdLevel::default(), &dict)?;
```

### FLAC — PCM parameters

```rust
use omnizip_flac::{compress, PcmParams};

let params = PcmParams {
    sample_rate: 44_100,
    channels: 2,
    bits_per_sample: 16,
};
let bytes = compress(pcm_le_bytes, &params)?;
```

### Rice++ — DwarFS config

```rust
use omnizip_ricepp::{compress, CodecConfig};

let config = CodecConfig {
    block_size: 4096,
    bytes_per_sample: 2,
};
let bytes = compress(input, config)?;
```

### GLZA — raw vs Huffman container

```rust
use omnizip_glza::{compress_with_version, encode};

// VERSION_RAW = 1 (Phase 1, simpler).
let v1 = compress_with_version(input, encode::VERSION_RAW)?;

// VERSION_HUFFMAN = 2 (Phase 2, better ratio on most inputs).
let v2 = compress_with_version(input, encode::VERSION_HUFFMAN)?;
```

`compress(input)` automatically picks the smaller of v1/v2.

---

## What's NOT yet tunable

These are flagged as Phase 2 work:

| Codec    | Gap                                                    | Priority |
|----------|--------------------------------------------------------|----------|
| LZMA     | `lc`, `lp`, `pb`, dictionary size not exposed at API   | high     |
| PPMd8    | bit-level model (should be byte-level like PPMd7)      | high     |
| BZip2    | block size fixed at 900 KB                             | medium   |
| Deflate  | strategy (lazy/greedy) not selectable                  | medium   |
| Brotli   | window size, mode (text/font/generic) not exposed      | medium   |
| BLOSC    | shuffle/memcpy threshold, compressor selection         | low      |

---

## Determinism

Every tunable is **deterministic**: same input + same tunables ⇒ byte-identical output across runs, machines, and Rust versions. This is a workspace-wide invariant required by LimniFS (where `DropId = BLAKE3(plaintext)` and codec non-determinism breaks dedup).
