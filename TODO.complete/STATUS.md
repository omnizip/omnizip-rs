# Final Status — omnizip-rs Full Port

**Last updated**: 2026-08-06 (post v0.14.24)
**Build**: 0 workspace warnings.
**Test results**: 1033 tests passing across 52 suites, 0 failures, 0 ignored.
**Latest release**: [v0.14.24](https://github.com/omnizip/omnizip-rs/releases/tag/v0.14.24)

## End-to-end working features

### LZMA crate (`omnizip-lzma`)

**Decoders**:
- ✅ LZMA-Alone (`.lzma`) — all fixtures
- ✅ XZ container (`.xz`) including BCJ-x86 filter
- ✅ Lzip (`.lz`) — 6/8 fixtures (v0 multi-member edge case)

**Encoders** (all round-trip via own decoder):
- ✅ LZMA1 packet encoder (literals + EOPM)
- ✅ LZMA-Alone (`.lzma`) container
- ✅ LZMA2 chunk encoder
- ✅ XZ container encoder
- ✅ Lzip container encoder
- ✅ Range encoder, literal/length/distance encoders
- ✅ Hash-chain match finder
- ✅ `LzmaCodec` wired into `omnizip-codecs::Codec` trait

### ZSTD crate (`omnizip-zstd`)

**Decoders**:
- ✅ Frame header parse
- ✅ Raw/RLE block decode
- ✅ Compressed literals decode (direct Huffman weights)
- ✅ Single + 4-stream Huffman literal decode
- ✅ Treeless literals (set_repeat) decode
- ✅ Sequences decode (PREDEFINED + RLE modes)
- ✅ XXHash64 → u32 frame checksum verification
- ✅ Sliding-window BitStream (handles arbitrary-length FSE streams)
- ✅ FSE table-from-stream reader (`fse/from_stream.rs`)
- 7/11 golden fixtures fully decode

**Encoders**:
- ✅ Frame encoder (Raw blocks; multi-block for inputs > 128 KiB)
- ✅ Round-trips through reference `zstd -d` for inputs up to 200,000 bytes
- ✅ Huffman tree builder (`huffman/encoder.rs`)
- ✅ XXHash64 (`xxhash.rs`)
- ✅ `ZstdCodec` wired into `omnizip-codecs::Codec` trait

### Filters crate (`omnizip-filters`)
- ✅ BCJ-x86 filter (encoder + decoder, reversible)
- ✅ Delta filter

### Wrapper crates
- ✅ Snappy, LZ4, DEFLATE, Brotli — all wrap their respective ecosystem
  crates and round-trip through `omnizip-codecs::Codec`.

### Brotli (pure-Rust) crate (`omnizip-brotli`)

**Encoder** (vendored from upstream `compress_fragment_two_pass`, BSD-3-Clause):
- ✅ Valid Brotli streams (verified by `brotli -d`) across all 17 property
  fixtures + extra CLI corpus.
- ✅ Bit-exact with `brotli -q 1 -w 22` on inputs up to ~120 bytes.

**Decoder**:
- ✅ Frame header (RFC 7932 §9.1) — all WBITS variants.
- ✅ Metablock header (§9.2) — ISLAST/ISLASTEMPTY/MNIBBLES/MLEN/ISUNCOMPRESSED.
- ✅ Uncompressed metablocks.
- ✅ Huffman-coded metablocks in the trivial layout produced by
  `compress_fragment_two_pass` (single block type per category,
  single literal/distance Huffman tree).
- ✅ Simple-form Huffman tables (NSYM=1..4) + complex-form (with
  iterated symbol 16/17 repeat decoding).
- ✅ Distance formula for general NPOSTFIX + NDIRECT case (mirrors
  upstream `ReadDistanceInternal`).
- ✅ `ContextMode::context_id_2(p1, p2)` with full UTF-8 + SIGNED
  lookup tables (K_UTF8_CONTEXT_LOOKUP, K_SIGNED_3BIT_CONTEXT_LOOKUP).
- ✅ Metablock-end short-circuit — exits after INSERT literals once
  `output.len() >= mlen` (mirrors upstream `ProcessCommandsInternal`).
- ✅ Round-trip via our encoder + decoder on all 17 property fixtures.

**Decoder still rejects** (see TODO 172 + 174):
- Multiple block types per category (NBLTYPES > 1).
- Literal or distance context maps (NTREESL > 1 or NTREESD > 1).
- Static dictionary references (distance_code > max_distance).
- These are required to decode reference brotli streams from
  `brotli -q N` for N ≥ 2 on real-world inputs.

## Known remaining gaps

1. **FSE-compressed Huffman weights** (`huffman-compressed-larger.zst`):
   FSE table reader + sliding-window BitStream in place, but the FSE
   symbol decoder produces wrong weight values for one fixture. The
   weights fail the Kraft-inequality check. Needs differential
   debugging against `HUF_readStats_body` in the C reference.

2. **MODE_FSE for sequence tables**: same root cause as #1.

3. **LZMA xz interop**: my LZMA encoder round-trips via my decoder,
   but `xz -d` rejects the EOPM marker. The distance encoder's
   direct-bits path was polarity-fixed but a residual bit-pattern
   issue remains.

4. **ZSTD Huffman encoder for production**: standard Huffman works
   but can produce codes longer than `HUF_TABLELOG_MAX` for skewed
   distributions. Length-limited package-merge algorithm would close
   this gap. The frame encoder uses Raw literals by default so this
   is not blocking.

5. **LZMA optimal parser** (Phase C): the current encoder emits only
   literals + EOPM (no matches). Match finder is implemented but not
   integrated into the encoder loop. Adding greedy matching would
   give 2-5x compression improvement on text inputs.

## Architecture

The codebase follows the principles outlined in CLAUDE.md:

- **`#![forbid(unsafe_code)]`** workspace-wide — no `unsafe` blocks
  anywhere. SIMD acceleration deferred to `std::simd` (Phase C task).
- **OCP**: Adding a new codec = one new crate + one `register()` call
  on `CodecRegistry`. Dispatch code never changes.
- **DRY**: Encode/decode helpers (e.g. `encode_tree`/`decode_tree`)
  live in a shared module and are used by both halves.
- **MECE**: Each module has a single responsibility. The encoder
  module mirrors the decoder module but doesn't share state.
- **Determinism**: All encoders are deterministic — same input + level
  produces byte-identical output across runs (verified by dedicated
  determinism tests).

## Test coverage

- 257+ unit tests across 24 test suites
- Differential parity against reference `xz -d` and `zstd -d` oracles
- Round-trip tests for every container format
- Determinism tests for every encoder
- All clippy `pedantic` warnings either auto-fixed or strategically
  allowed (cast lints at crate level for codec code; similar_names
  in domain-named LZMA parameters)

## Reproducibility

```bash
cargo test --workspace                 # 257+ tests pass
cargo clippy --workspace --all-targets # ~20 minor warnings
cargo build --workspace                # no errors, no critical warnings
```
