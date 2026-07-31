# 02 — ZSTD source map

Every Ruby file in omnizip's Zstandard implementation mapped to its Rust
counterpart in omnizip-zstd. Line counts from the Ruby source.

## ZSTD (3,150 LOC total)

### Constants

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/constants.rb` | 141 | `src/constants.rs` | pending |

### Frame

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/frame/frame.rb` | ~100 | `src/frame/mod.rs` | pending |
| `zstandard/frame/header.rb` | 220 | `src/frame/header.rs` | pending |
| `zstandard/frame/block.rb` | 126 | `src/frame/block.rs` | pending |

### FSE (Finite State Entropy)

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/fse/fse.rb` | 34 | `src/fse/mod.rs` | pending |
| `zstandard/fse/bitstream.rb` | 186 | `src/fse/bitstream.rs` | pending |
| `zstandard/fse/table.rb` | 266 | `src/fse/table.rs` | pending |
| `zstandard/fse/encoder.rb` | 322 | `src/fse/encoder.rs` | Phase C |

### Huffman

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/huffman.rb` | 269 | `src/huffman/mod.rs` | Phase A |
| `zstandard/huffman_encoder.rb` | 336 | `src/huffman/encoder.rs` | Phase B |

### Literals

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/literals.rb` | 174 | `src/literals/mod.rs` | Phase A |
| `zstandard/literals_encoder.rb` | 248 | `src/literals/encoder.rs` | Phase B |

### Sequences & top-level

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `zstandard/sequences.rb` | 342 | `src/sequences.rs` | Phase A |
| `zstandard/decoder.rb` | 225 | `src/decoder.rs` | Phase A |
| `zstandard/encoder.rb` | 228 | `src/encoder.rs` | Phase B |

## C reference (perf tuning only)

| C source | Consulted for | License |
|---|---|---|
| `zstd/lib/decompress/zstd_decompressBlock.c` | Block decode perf | BSD-3 |
| `zstd/lib/decompress/huf_decompress.c` | Huffman decode tables | BSD-3 |
| `zstd/lib/common/fse.h` | FSE API and inlines | BSD-3 |
| `zstd/lib/compress/fse_compress.c` | FSE encode (Phase C) | BSD-3 |
| `zstd/lib/compress/zstd_compress.c` | Encoder parameter table | BSD-3 |
| `zstd/lib/compress/zstd_opt.c` | Optimal parser (Phase C) | BSD-3 |
| `zstd/lib/dictBuilder/fastcover.c` | Dictionary training | BSD-3 |

## RFC

- RFC 8878: Zstandard Compression and the application/zstd Media Type
  (September 2021). Normative for the frame, block, and sequence formats.
