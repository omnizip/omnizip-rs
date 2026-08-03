# Codec Tunability Reference

Every codec in omnizip-rs accepts the standard `CompressionLevel` (u8) via the `Codec` trait. Most codecs expose **additional** tunables for power users who need finer control over the speed/ratio/memory trade-off.

This document catalogues what's tunable per codec, with examples.

---

## Quick reference

| Codec        | Level | Memory Budget | Format / Mode | Strategy / Parser | Algorithm-Specific |
|--------------|:-----:|:-------------:|:-------------:|:-----------------:|--------------------|
| omnizip-blosc | ✓    | —             | —             | —                 | `BloscCodec::compress_with_options(_, item_size, shuffle)`, item_size ∈ {1,2,4,8}, shuffle ∈ {None, Byte, Bit} |
| omnizip-brotli | ✓   | —             | mode + dict   | —                 | `BrotliOptions { quality, window_size, mode, custom_dictionary }` |
| omnizip-bzip2 | ✓    | —             | —             | —                 | `Bzip2Codec::compress_with_block_size(_, bytes)`, 100 KB..=900 KB |
| omnizip-deflate | ✓  | —             | format        | **strategy**      | `DeflateOptions { level, format, strategy }`, strategy ∈ {Default, Filtered, HuffmanOnly, Rle, Fixed} |
| omnizip-deflate64 | ✓ | —            | —             | —                 | `MIN_LEVEL`, `MAX_LEVEL` |
| omnizip-flac | —     | —             | —             | —                 | `PcmParams { sample_rate, channels, bits_per_sample }` |
| omnizip-fsst | —     | —             | —             | —                 | (no options; learns symbol table adaptively) |
| omnizip-glza | —     | —             | version       | —                 | `compress_with_version(1 | 2)` |
| omnizip-lz4  | ✓    | —             | —             | —                 | `Lz4FastCodec` vs `Lz4HcCodec` |
| **omnizip-lzma** | ✓ | —             | —             | **parser**        | `LzmaOptions { lc, lp, pb, dict_size, use_optimal_parser }` (`.lzma`); `encode_lzma2_stream_with_options`; `xz_compress_with_options` |
| **omnizip-ppmd (PPMd7)** | ✓ | **✓ 80 MB** | —    | —                 | `compress_with_budget`, `PpmModel::with_memory_budget` |
| **omnizip-ppmd (PPMd8)** | ✓ | **✓ 64 MB** | —    | —                 | `compress_with_budget`, restore method, max_nodes |
| omnizip-ricepp | ✓  | —             | —             | —                 | `CodecConfig { block_size, bytes_per_sample }` |
| omnizip-snappy | —   | —             | —             | —                 | (no options; Snappy is fixed-parameter) |
| omnizip-zpaq | ✓    | —             | —             | —                 | (level maps to context-mixing model) |
| omnizip-zstd | ✓    | —             | —             | —                 | `ZstdLevel`, `compress_with_dict` |

---

## Detailed examples

### PPMd7 — user-tunable memory budget + byte-level context tree

PPMd7 is the most configurable codec in the workspace. The context trie's arena size scales linearly with the memory budget; more memory = more contexts tracked = better ratio.

```rust
use omnizip_ppmd::ppmd7;

// Default: 80 MB budget, byte-level PPM with PPM*C escape.
let bytes = ppmd7::compress(b"hello world", 4)?;

// Custom budget: 16 MB (smaller footprint, slightly worse ratio).
let bytes = ppmd7::compress_with_budget(b"hello world", 4, 16 * 1024 * 1024)?;

// Custom budget: 256 MB (best ratio, more memory).
let bytes = ppmd7::compress_with_budget(b"hello world", 4, 256 * 1024 * 1024)?;
```

**Compression ratio on 100 KB Gutenberg text**: 0.417 (down from 0.683 with the old bit-level model — 39% improvement).

### PPMd8 — memory budget + byte-level PPM through trie + RLE

```rust
use omnizip_ppmd::ppmd8;

// Default: 64 MB budget, byte-level PPM, RESTART restoration.
let bytes = ppmd8::compress(b"hello world", 6)?;

// Custom budget.
let bytes = ppmd8::compress_with_budget(b"hello world", 6, 128 * 1024 * 1024)?;
```

PPMd8 also exposes `Ppmd8Model::new(order, restore_method, max_nodes)` for callers driving the model directly.

**Compression ratio on 100 KB Gutenberg text**: 0.465.

### LZMA — lc, lp, pb, dict_size, parser choice (.lzma, LZMA2, XZ)

Full LZMA parameter exposure matching `xz`/`lzma` CLI flags. Three entry points:

```rust
use omnizip_lzma::{
    LzmaOptions, lzma_alone_compress_with_options,
    encode_lzma2_stream_with_options, xz_compress_with_options,
};

// 1. .lzma (LZMA-Alone) container.
let bytes = lzma_alone_compress_with_options(b"text", &LzmaOptions {
    lc: 4, lp: 0, pb: 0,
    dict_size: 1 << 20, // 1 MB
    use_optimal_parser: true,
})?;

// 2. LZMA2 raw stream (no container — used inside XZ).
let bytes = encode_lzma2_stream_with_options(b"text", &LzmaOptions::default())?;

// 3. Full XZ container (stream header + block + index + footer).
let bytes = xz_compress_with_options(b"text", &LzmaOptions::default())?;
```

**Spec hard limit**: `lc + lp <= 4` (enforced at validation).

| Parameter    | Range     | Default | Effect                                                       |
|--------------|-----------|---------|--------------------------------------------------------------|
| `lc`         | 0..=8     | 3       | Literal-context bits — higher = more literal coding precision |
| `lp`         | 0..=4     | 0       | Literal-position bits                                        |
| `pb`         | 0..=4     | 2       | Position bits — higher = more position-dependent coding      |
| `dict_size`  | 4 KB..=4 GB | 16 MB | Match-finder window                                          |
| `use_optimal_parser` | bool | false | DP parser (slower, better ratio) vs lazy parser               |

### ZSTD — level and dictionary

```rust
use omnizip_zstd::{compress, ZstdLevel, compress_with_dict};

// Level 1..=22, matching the upstream zstd CLI.
let bytes = compress(b"data", ZstdLevel::new(19))?;

// Train a dictionary on representative samples, then use it.
let dict = std::fs::read("dict.bin")?;
let bytes = compress_with_dict(b"data", ZstdLevel::default(), &dict)?;
```

### Brotli — quality, window size, mode, custom dictionary

```rust
use omnizip_brotli::{BrotliCodec, BrotliOptions, BrotliMode};

let codec = BrotliCodec::new();
let opts = BrotliOptions {
    quality: Some(11),           // default 11
    window_size: Some(20),       // 1 MB window (default 22 = 4 MB)
    mode: BrotliMode::Text,      // hint: ASCII text
    custom_dictionary: Some(&dict_bytes), // optional shared dictionary
};
let bytes = codec.compress_with_options(b"hello world".repeat(100), opts)?;
```

| Field          | Range       | Default | Notes                                              |
|----------------|-------------|---------|----------------------------------------------------|
| `quality`      | 0..=11      | 11      | Higher = better ratio, slower                       |
| `window_size`  | 10..=24     | 22      | `log2(bytes)`. 10 = 1 KB, 22 = 4 MB, 24 = 16 MB     |
| `mode`         | enum        | Generic | `Generic`, `Text`, `Font`                           |
| `custom_dictionary` | `Option<&[u8]>` | `None` | Pre-shared decoder history (caller and decoder must agree) |

### BZip2 — explicit block size

```rust
use omnizip_bzip2::Bzip2Codec;

let codec = Bzip2Codec::new();
// 100 KB blocks (fastest), 200 KB, ..., 900 KB (best ratio).
// Must be a multiple of 100_000 in [100_000, 900_000].
let bytes = codec.compress_with_block_size(b"data", 500_000)?;
```

### Deflate — output format + match-finder strategy

```rust
use omnizip_deflate::{DeflateCodec, DeflateFormat, DeflateOptions, DeflateStrategy};
use omnizip_codecs::Codec;

let codec = DeflateCodec::new();
let opts = DeflateOptions {
    level: 9,
    format: DeflateFormat::Gzip,           // Zlib, Raw, or Gzip
    strategy: DeflateStrategy::Filtered,   // match-finder heuristic
};
let bytes = codec.compress_with_options(b"data", opts)?;
```

| Strategy       | Effect                                                              |
|----------------|---------------------------------------------------------------------|
| `Default`      | Standard LZ77 + Huffman (zlib default)                              |
| `Filtered`    | Only matches ≥5 bytes. Better for structured data (tables, code)    |
| `HuffmanOnly` | Skip LZ77 entirely. Fastest on high-entropy input                   |
| `Rle`         | Run-length only — only matches at distance 1                        |
| `Fixed`       | Fixed Huffman codes only (no dynamic tables)                        |

### BLOSC — item size + shuffle mode

```rust
use omnizip_blosc::{BloscCodec, ShuffleMode};

let codec = BloscCodec::new();
// 8-byte items (f64 / i64), bit shuffle (best for float arrays).
let bytes = codec.compress_with_options(&float_bytes, 8, ShuffleMode::Bit)?;
```

| `item_size` | Use case                              |
|-------------|---------------------------------------|
| 1           | Generic byte stream (shuffle = no-op) |
| 2           | u16 / i16 / half-float arrays          |
| 4           | u32 / i32 / f32 arrays (default)       |
| 8           | u64 / i64 / f64 arrays                 |

Shuffle modes: `None`, `Byte` (transposes bytes within each item group), `Bit` (transposes individual bits across items).

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

// VERSION_RAW = 1, VERSION_HUFFMAN = 2.
let v1 = compress_with_version(input, encode::VERSION_RAW)?;
let v2 = compress_with_version(input, encode::VERSION_HUFFMAN)?;
```

`compress(input)` automatically picks the smaller of v1/v2. Inputs > 512 KB auto-chunk into multi-chunk streams.

---

## Determinism

Every tunable is **deterministic**: same input + same tunables ⇒ byte-identical output across runs, machines, and Rust versions. This is a workspace-wide invariant required by LimniFS (where `DropId = BLAKE3(plaintext)` and codec non-determinism breaks dedup).

All codec APIs are `#![forbid(unsafe_code)]` workspace-wide.
