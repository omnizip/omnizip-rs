# 01 — LZMA source map

Every Ruby file in omnizip's LZMA implementation mapped to its Rust
counterpart in omnizip-lzma. Line counts from the Ruby source.

## Core LZMA (8,464 LOC total)

### Constants & types

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/constants.rb` | 112 | `src/constants.rs` | ✅ ported |
| `lzma/bit_model.rb` | ~100 | `src/bit_model.rs` | pending |
| `lzma/match.rb` | ~50 | `src/match.rs` | pending |
| `lzma/dictionary.rb` | ~100 | `src/dictionary.rs` | pending |

### State machine

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/state.rb` | ~150 | `src/state.rs` | pending |
| `lzma/lzma_state.rb` | ~80 | `src/state/lzma.rs` | pending |
| `lzma/xz_state.rb` | ~100 | `src/state/xz.rs` | pending |
| `lzma/probability_models.rb` | ~150 | `src/probability_models.rs` | pending |

### Range coder

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/range_coder.rb` | ~50 | `src/range_coder/mod.rs` | pending |
| `lzma/range_decoder.rb` | 274 | `src/range_coder/decoder.rs` | pending |
| `lzma/range_encoder.rb` | 202 | `src/range_coder/encoder.rs` | Phase B |
| `lzma/xz_range_encoder.rb` | 223 | `src/range_coder/xz_encoder.rs` | Phase B |
| `lzma/xz_range_encoder_exact.rb` | 314 | `src/range_coder/xz_exact.rs` | Phase B |
| `lzma/xz_buffered_range_encoder.rb` | 323 | `src/range_coder/xz_buffered.rs` | Phase B |

### Match finder

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/match_finder.rb` | 233 | `src/match_finder.rs` | pending |
| `lzma/match_finder_config.rb` | ~80 | `src/match_finder_config.rs` | pending |
| `lzma/match_finder_factory.rb` | ~60 | `src/match_finder_factory.rs` | pending |
| `lzma/xz_match_finder_adapter.rb` | 224 | `src/match_finder_xz_adapter.rs` | pending |

### Coders (literal / length / distance)

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/literal_decoder.rb` | 204 | `src/coder/literal_decoder.rs` | Phase A |
| `lzma/literal_encoder.rb` | 208 | `src/coder/literal_encoder.rs` | Phase B |
| `lzma/length_coder.rb` | 172 | `src/coder/length.rs` | Phase A (decode half) |
| `lzma/distance_coder.rb` | 326 | `src/coder/distance.rs` | Phase A (decode half) |

### Decoders

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/decoder.rb` | 146 | `src/decoder/mod.rs` | Phase A |
| `lzma/lzma_alone_decoder.rb` | 191 | `src/decoder/alone.rs` | Phase A |
| `lzma/lzip_decoder.rb` | 368 | `src/decoder/lzip.rs` | Phase A |
| `lzma/xz_utils_decoder.rb` | 1,311 | `src/decoder/xz_utils.rs` | Phase A |

### Encoders

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma/xz_encoder.rb` | 420 | `src/encoder/xz.rs` | Phase B |
| `lzma/xz_encoder_fast.rb` | 640 | `src/encoder/xz_fast.rs` | Phase B |
| `lzma/optimal_encoder.rb` | ~400 | `src/encoder/optimal.rs` | Phase C |
| `lzma/xz_price_calculator.rb` | 167 | `src/price.rs` | Phase B |

## LZMA2 container (906 LOC)

| Ruby source | LOC | Rust module | Status |
|---|---:|---|---|
| `lzma2/constants.rb` | 38 | `src/lzma2/constants.rs` | pending |
| `lzma2/properties.rb` | 179 | `src/lzma2/properties.rs` | pending |
| `lzma2/lzma2_chunk.rb` | 161 | `src/lzma2/chunk.rs` | Phase C |
| `lzma2/chunk_manager.rb` | 180 | `src/lzma2/chunk_manager.rs` | Phase C |
| `lzma2/encoder.rb` | 143 | `src/lzma2/encoder.rs` | Phase C |
| `lzma2/simple_lzma2_encoder.rb` | 122 | `src/lzma2/simple.rs` | Phase C |
| `lzma2/xz_encoder_adapter.rb` | 83 | `src/lzma2/xz_adapter.rs` | Phase C |

## C reference (perf tuning only, after Ruby port verifies correct)

| C source | Consulted for | License |
|---|---|---|
| `xz/src/liblzma/rangecoder/range_decoder.h` | Range decoder perf | 0BSD |
| `xz/src/liblzma/rangecoder/range_encoder.h` | Range encoder perf | 0BSD |
| `xz/src/liblzma/lz/lz_encoder_mf.c` | Match finder (HC4, BT4) perf | 0BSD |
| `xz/src/liblzma/lzma/lzma_encoder_optimum_fast.c` | Fast parser | 0BSD |
| `xz/src/liblzma/lzma/lzma_encoder_optimum_normal.c` | Optimal parser | 0BSD |
| `xz/src/liblzma/lzma/lzma_encoder_presets.c` | Level → parameters | 0BSD |
| `xz/src/liblzma/check/crc64.c` | CRC64 for XZ container | 0BSD |
| `xz/src/liblzma/common/stream_encoder.c` | XZ stream header/footer | 0BSD |
