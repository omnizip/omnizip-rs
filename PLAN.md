# omnizip-rs — Porting Plan

## Source of truth

The Ruby implementations in [`omnizip/omnizip`](https://github.com/omnizip/omnizip)
are the **algorithmic reference**. Every Rust module is a line-by-line
translation of the corresponding Ruby file. C reference implementations
(`tukaani-project/xz`, `facebook/zstd`) are consulted only for performance
tuning after the Ruby port is verified correct.

### Why port from Ruby, not C

| | Ruby (omnizip) | C (reference) |
|---|---|---|
| Readability | clean OOP, named methods | pointer arithmetic, macros |
| License | MIT (Ribose) | 0BSD (liblzma) / BSD-3 (zstd) |
| Correctness | already tested via omnizip specs | reference standard |
| Porting effort | mechanical translation | requires de-obfuscation first |

The Ruby already encodes the correct algorithm — the hard work is done.
Rust adds production speed; it does not need to re-derive the algorithm.

## LZMA port — `omnizip-lzma`

Ruby source: `omnizip/lib/omnizip/algorithms/lzma/` (7,558 LOC + 906 LOC LZMA2).

### Ruby → Rust module map

| Ruby file | Rust module | LOC (Ruby) |
|---|---|---:|
| `constants.rb` | `constants.rs` | 141 |
| `bit_model.rb` | `bit_model.rs` | ~100 |
| `probability_models.rb` | `probability_models.rs` | ~150 |
| `state.rb` / `lzma_state.rb` / `xz_state.rb` | `state.rs` | ~300 |
| `range_coder.rs` / `range_encoder.rb` / `range_decoder.rb` | `range_coder/{mod,encoder,decoder}.rs` | 476 |
| `xz_range_encoder.rb` / `xz_range_encoder_exact.rb` / `xz_buffered_range_encoder.rb` | `range_coder/xz_{encoder,exact,buffered}.rs` | 860 |
| `match_finder.rb` / `match_finder_config.rb` / `match_finder_factory.rb` / `xz_match_finder_adapter.rb` | `match_finder.rs` | ~700 |
| `literal_encoder.rb` / `literal_decoder.rb` / `literal_decoder.rb` | `coder/literal.rs` | 412 |
| `length_coder.rb` | `coder/length.rs` | 172 |
| `distance_coder.rb` | `coder/distance.rs` | 326 |
| `optimal_encoder.rb` | `encoder/optimal.rs` | ~150 |
| `xz_encoder.rb` | `encoder/xz.rs` | 420 |
| `xz_encoder_fast.rb` | `encoder/xz_fast.rs` | 640 |
| `decoder.rb` / `lzip_decoder.rb` / `lzma_alone_decoder.rb` / `xz_utils_decoder.rb` | `decoder/{mod,lzip,alone,xz_utils}.rs` | 1,819 |
| `match.rb` | `match.rs` | ~50 |
| `dictionary.rb` | `dictionary.rs` | ~100 |
| `xz_price_calculator.rb` | `price.rs` | 167 |

LZMA2 (`omnizip/lib/omnizip/algorithms/lzma2/`, 906 LOC):

| Ruby file | Rust module | LOC (Ruby) |
|---|---|---:|
| `constants.rb` / `properties.rb` | `lzma2/{constants,properties}.rs` | 217 |
| `lzma2_chunk.rb` / `chunk_manager.rb` | `lzma2/{chunk,chunk_manager}.rs` | 341 |
| `encoder.rb` / `simple_lzma2_encoder.rb` / `xz_encoder_adapter.rb` | `lzma2/{encoder,simple,xz_adapter}.rs` | 348 |

### Phased delivery

**Phase A — decoder + range coder + match finder (read parity):** 2–3 weeks

Port the decoder side first. Once Rust can decompress everything the Ruby
decoder can, the encoder port has a reliable oracle.

Rust modules: `constants`, `state`, `bit_model`, `range_coder/decoder`,
`match_finder`, `decoder/*`.

Acceptance: Rust decoder reads every `.xz` / `.lzma` fixture under
`omnizip/spec/fixtures/` byte-identically to Ruby's output.

**Phase B — encoder core (level 0–3 equivalent):** 3–4 weeks

Port the encoder skeleton + fast optimal parser + literal/length/distance
coders. Produces valid LZMA streams at levels 0–3.

Rust modules: `range_coder/encoder`, `range_coder/xz_encoder`, `coder/*`,
`encoder/xz_fast`, `price`.

Acceptance: Rust encoder output at each level decompresses byte-identically
through both the Rust decoder and the reference `xz -d`.

**Phase C — optimal parser + LZMA2 chunking + XZ container (level 4–9):** 4–6 weeks

Port `optimal_encoder.rb` (DP-based), LZMA2 chunk format, XZ stream container
with CRC64.

Rust modules: `encoder/optimal`, `lzma2/*`, `xz_container.rs`, `crc64.rs`.

Acceptance: ratio within 5% of reference `xz -9` on Silesia; byte-identical
round-trip through `xz -d`.

## ZSTD port — `omnizip-zstd`

Ruby source: `omnizip/lib/omnizip/algorithms/zstandard/` (3,150 LOC).

### Ruby → Rust module map

| Ruby file | Rust module | LOC (Ruby) |
|---|---|---:|
| `constants.rb` | `constants.rs` | 141 |
| `frame/frame.rb` / `frame/header.rb` / `frame/block.rb` | `frame/{mod,header,block}.rs` | ~550 |
| `fse/fse.rb` / `fse/bitstream.rb` / `fse/table.rb` / `fse/encoder.rb` | `fse/{mod,bitstream,table,encoder}.rs` | ~775 |
| `huffman.rb` / `huffman_encoder.rb` | `huffman/{mod,encoder}.rs` | 605 |
| `literals.rb` / `literals_encoder.rb` | `literals/{mod,encoder}.rs` | 422 |
| `sequences.rb` | `sequences.rs` | 342 |
| `encoder.rb` | `encoder.rs` | 228 |
| `decoder.rb` | `decoder.rs` | 225 |

### Phased delivery

**Phase A — decoder + frame parse (read parity):** 1–2 weeks

**Phase B — encoder core (single-segment, Huffman literals):** 2–3 weeks

**Phase C — FSE entropy + sequences + multi-block frames:** 3–4 weeks

Acceptance at each phase mirrors the LZMA gates: byte-identical output
vs Ruby, then vs reference `zstd -d`.

## Cross-language conformance gate

Every PR to this repo runs:

1. Clone `omnizip/omnizip` at the pinned Ruby ref.
2. For each fixture under `omnizip/spec/fixtures/{xz,lzma,zstd}/`:
   - Run the Ruby decoder → capture output.
   - Run the Rust decoder → capture output.
   - Assert byte-identical.
3. For encoder PRs: run both encoders at matching levels, then decode both
   through the reference C tool (`xz -d` / `zstd -d`), assert byte-identical.
4. Record ratio numbers in the task file; block merge on regression beyond
   the phase budget.

## License

Each Ruby source file carries the header:
```
Copyright (C) 2025 Ribose Inc.
Permission is hereby granted, free of charge, ...
```
(MIT). The Rust port inherits MIT OR Apache-2.0; see `LICENSE-MIT`,
`LICENSE-APACHE`, `LICENSE-NOTICE.md`.
