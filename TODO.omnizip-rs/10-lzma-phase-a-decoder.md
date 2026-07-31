# 10 — LZMA Phase A: decoder + range coder + match finder

- **Priority:** P0 (the encoder port's oracle)
- **Depends on:** [01](01-codec-trait-registry.md), [02](02-cross-language-differential-harness.md)
- **Estimated effort:** 2–3 weeks
- **Crate:** `omnizip-lzma`

## Goal

Port the LZMA decoder side completely: constants, state, range coder decoder,
match finder, and the LZMA / LZMA-alone / XZ-utils decoder variants. Once
Rust can decode everything the Ruby decoder can, the encoder port (Phase B)
has a reliable oracle.

This is the highest-leverage first port: every encoder test in Phase B/C
asks "does my encoder output decode correctly?" — that question requires a
working decoder.

## Ruby → Rust module map

### Core (7,558 LOC across 31 Ruby files)

| Ruby source | Rust module | LOC | Notes |
|---|---|---:|---|
| `lzma/constants.rb` | `constants.rs` | 141 | numeric constants |
| `lzma/bit_model.rb` | `bit_model.rs` | ~100 | probability model |
| `lzma/probability_models.rb` | `probability_models.rs` | ~150 | literal/length/distance models |
| `lzma/state.rb` | `state/mod.rs` | ~150 | LZMA state machine |
| `lzma/lzma_state.rb` | `state/lzma.rs` | ~80 | raw LZMA1 state |
| `lzma/xz_state.rb` | `state/xz.rs` | ~100 | XZ-container state |
| `lzma/range_coder.rb` | `range_coder/mod.rs` | ~50 | trait |
| `lzma/range_decoder.rb` | `range_coder/decoder.rs` | 274 | the decoder |
| `lzma/range_encoder.rb` | `range_coder/encoder.rs` | 202 | Phase B |
| `lzma/xz_range_encoder.rb` | `range_coder/xz_encoder.rs` | 223 | Phase B |
| `lzma/xz_range_encoder_exact.rb` | `range_coder/xz_exact.rs` | 314 | Phase B |
| `lzma/xz_buffered_range_encoder.rb` | `range_coder/xz_buffered.rs` | 323 | Phase B |
| `lzma/match_finder.rb` | `match_finder.rs` | 233 | hash chain |
| `lzma/match_finder_config.rb` | `match_finder_config.rs` | ~80 | per-level config |
| `lzma/match_finder_factory.rb` | `match_finder_factory.rs` | ~60 | factory |
| `lzma/xz_match_finder_adapter.rb` | `match_finder_xz_adapter.rs` | 224 | XZ variant |
| `lzma/literal_encoder.rb` | `coder/literal_encoder.rs` | 208 | Phase B |
| `lzma/literal_decoder.rb` | `coder/literal_decoder.rs` | 204 | **Phase A** |
| `lzma/length_coder.rb` | `coder/length.rs` | 172 | shared encode+decode |
| `lzma/distance_coder.rb` | `coder/distance.rs` | 326 | shared |
| `lzma/match.rb` | `match.rs` | ~50 | data class |
| `lzma/dictionary.rb` | `dictionary.rs` | ~100 | sliding window |
| `lzma/decoder.rb` | `decoder/mod.rs` | 146 | top-level |
| `lzma/lzma_alone_decoder.rb` | `decoder/alone.rs` | 191 | `.lzma` legacy |
| `lzma/lzip_decoder.rb` | `decoder/lzip.rs` | 368 | `.lz` format |
| `lzma/xz_utils_decoder.rs` | `decoder/xz_utils.rs` | 1,311 | XZ container |

### XZ encoder (Phase B — NOT in this task)

| `lzma/xz_encoder.rb` | `encoder/xz.rs` | 420 | Phase B |
| `lzma/xz_encoder_fast.rb` | `encoder/xz_fast.rs` | 640 | Phase B |
| `lzma/optimal_encoder.rb` | `encoder/optimal.rs` | ~150 | Phase B |
| `lzma/xz_price_calculator.rb` | `price.rs` | 167 | Phase B |

## Phase A scope (decode-side only)

Port every Ruby file marked **Phase A** above. Specifically:

1. **Constants & types** (1 day): `constants.rs`, `bit_model.rs`,
   `state/*.rs`, `match.rs`, `dictionary.rs`. Pure data translation.
2. **Range decoder** (2 days): `range_coder/decoder.rs`. The range coder is
   the heart of LZMA decode — bit-by-bit probability-driven decoding.
3. **Match finder** (2 days): `match_finder.rs`,
   `match_finder_config.rs`, `match_finder_factory.rs`. Hash chain
   algorithm. Needed by decoder for sliding-window reconstruction.
4. **Literal/length/distance coders (decode side)** (2 days):
   `coder/literal_decoder.rs`, `coder/length.rs` (decode half),
   `coder/distance.rs` (decode half).
5. **Top-level decoders** (3 days): `decoder/mod.rs`, `decoder/alone.rs`,
   `decoder/lzip.rs`, `decoder/xz_utils.rs`. These wire the coder pieces
   together for each container format.

## Acceptance

- `cargo test -p omnizip-lzma` passes.
- **Differential gate:** every `.xz`, `.lzma`, `.lz` fixture under
  `tests/differential/fixtures/` decodes byte-identically between Ruby and
  Rust. The harness (task 02) reports per-fixture parity.
- **C reference gate:** every fixture produced by reference `xz` at
  levels 0–9 round-trips through our decoder.
- Clippy clean, no `unsafe`, public API documented.
- Decode throughput ≥ 20 MB/s on Apple M1 single core (Ruby is ~1 MB/s; Rust
  should be 20–50x faster).

## Implementation notes

- The range coder uses 32-bit unsigned arithmetic with careful carry
  handling. Translate the Ruby's `Integer` arithmetic to `u32`/`u64`
  explicitly; don't rely on Rust's auto-promotion.
- The match finder's hash chain is `Vec<u32>` of positions; the hash table
  is `Vec<u32>` of head pointers. Keep allocations amortised — the Ruby
  rebuilds on every call; Rust should reuse via `reset()`.
- The XZ-utils decoder (1,311 LOC) is the largest single file. Port it last,
  after the simpler alone + lzip decoders validate the coder pieces.
- Add `#[cfg(test)]` module-level tests for each coder, then integration
  tests at the container level.

## Why this is Phase A and not just "the decoder"

Phasing lets us ship incrementally. After Phase A:
- LimniFS can read every existing `.xz`-compressed drop in pure Rust (no
  more `lzma-rs` dependency for decode).
- The encoder work in Phase B/C has its oracle ready.
- Users get immediate value: pure-Rust LZMA decode for all legacy archives.

Phase B (encoder) and Phase C (optimal parser + containers) build on this.
