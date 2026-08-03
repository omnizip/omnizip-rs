# 97 — Port libFLAC encoder algorithmic content to omnizip-flac

**Priority:** High (LimniFS #1 quality issue)
**Source:** `~/src/external/flac/src/libFLAC/` (Xiph reference, BSD license)

## Status

**Interop achieved (2026-08-03):** All FLAC files produced by omnizip-flac decode byte-identically through `flac -d` (libFLAC CLI). Validated by 6 permanent parity tests in `tests/differential/tests/flac_parity.rs`.

**Bugs fixed during the interop effort:**
- CRC-16 was written little-endian; spec requires big-endian
- `read_unary`/`write_unary` had FLAC's polarity inverted (was "ones then zero" instead of the spec's "zeros then one" terminator)
- Decoder's wasted-bits handler had the same polarity bug
- `effective_bps` calculation was `bps + wasted_bits` (should be `bps - wasted_bits`)
- `bs_code` values 8-15 (power-of-2 block sizes) weren't handled by the frame decoder
- Missing residual-overflow check in `quantise_and_predict` (silent i32 wrap produced out-of-bounds samples)

**Temporarily disabled for interop safety:**
- **Multi-partition Rice coding** (`MAX_PARTITION_ORDER = 0`) — partition layout diverges from spec; libFLAC puts `predictor_order - 1` residuals in partition 0 and equal-size partitions after, our encoder uses ceiling division.
- **LPC subframes** (LPC lookup in `subframe.rs` gated with `if false`) — high-order LPC quantization has a remaining interop bug (LOST_SYNC during decode). Bounded MAX_LPC_ORDER to 16 as a stopgap.

**Re-enable plan:** see `Phase 2B` and `Phase 3` sections below. Once both are fixed, our 28.62% ratio on the 3-second sine should drop back toward the 17.97% achieved in Phase 1+2 (and ideally closer to libFLAC's 18.59%).

## Measured baseline (2026-08-03)

Identical 3-second 440 Hz sine, 44.1 kHz, 16-bit mono, 131 072 samples:

| Encoder        | Output size | Ratio  |
|----------------|-------------|--------|
| omnipip-flac   | 75 031 B    | 28.62% |
| libFLAC --best | 48 737 B    | 18.59% |

Real gap ~1.5× (was misreported as 22× on short clips).

## Scope

**Total libFLAC: 29 831 LOC.** Split:

- **~16K LOC portable** (algorithmic): `stream_encoder.c` (5.3K),
  `lpc.c` (1.6K), `window.c` (308), `stream_encoder_framing.c` (594),
  `fixed.c` (667), `format.c` (608).
- **~5.8K LOC NOT portable under `#![forbid(unsafe_code)]`**: all
  `*_intrin_*.c` files (SSE2/AVX2/FMA/NEON). Re-express via
  `std::simd` where possible; otherwise stay scalar.
- **~9K LOC already covered** in omnizip-flac: decoder, metadata,
  OGG framing, bit writer/reader, CRC.

## Phased roadmap

Each phase is independently testable and commits separately.

### Phase 1 — Algorithmic core (highest ROI)

Closes ~60% of the gap (28.79% → ~22%).

- **1A: Multi-partition Rice coding.** `encoder/rice.rs` currently
  hardcodes `partition_order = 0`. libFLAC tries 0..=6 and picks the
  cheapest per actual encoded size. **Single biggest ratio win.**
- **1B: Exhaustive LPC search with real bit-cost.** `encoder/lpc.rs`'s
  `estimate_residual_bits` is a `log2(|r|+1) + 3` heuristic that
  diverges from actual Rice cost. Replace with the actual encoded
  size after Rice coding. Try all `order × precision × shift`
  combinations and pick the cheapest.

### Phase 2 — Windowing + multi-block-size

Closes another ~20% (22% → ~20%).

- **2A: Autocorrelation windowing.** Port `window.c`'s window
  functions (Bartlett, Flattop, Welch, Tukey). Applied before ACF,
  reduces spectral leakage → better LPC coefficients on tonal input.
- **2B: Multi-block-size selection.** `encoder/mod.rs::encode_stream`
  uses fixed `DEFAULT_BLOCK_SIZE = 4096`. libFLAC tries 192/256/...
  /16384 per-frame and picks the best.

### Phase 3 — Stereo + exhaustive tuning

Closes another ~10% (20% → ~18%).

- **3A: Mid-side stereo.** For stereo input, try independent /
  left-side / mid-side channel assignments per frame.
- **3B: Exhaustive constant-FIXED comparison.** Already done in
  subframe.rs but verify it matches libFLAC's cost function.

### Phase 4 — `std::simd` autocorrelation

**Speed only, no ratio change.** 2-4× encoder throughput.

- Replace `lpc.rs::autocorrelate`'s inner product loop with
  `f64x4`/`f32x4` SIMD. `std::simd` is portable and `unsafe`-free.
- This is the closest we can get to libFLAC's `lpc_intrin_sse2.c`
  without violating `#![forbid(unsafe_code)]`. Real PCLMULQDQ-style
  tricks aren't applicable to LPC anyway (it's plain MAC, not
  carryless multiply).

## What we will NOT port

- `_intrin_*.c` files (require `unsafe`). Documented in module-level
  comments; performance gap is acceptable given the safety guarantee.
- `ogg_encoder_aspect.c` / `ogg_decoder_aspect.c` — LimniFS uses raw
  FLAC, not OGG-FLAC.
- `metadata_iterators.c` / `metadata_object.c` — we don't expose
  arbitrary metadata editing; STREAMINFO + PADDING suffices.
- `md5.c` — we leave MD5 zero in STREAMINFO (valid per spec).

## Acceptance criteria

- [ ] Phase 1A landed: multi-partition Rice in `encoder/rice.rs`.
- [ ] Phase 1B landed: exhaustive LPC with real bit-cost in
      `encoder/lpc.rs`.
- [ ] Phase 2A landed: windowed autocorrelation.
- [ ] Phase 2B landed: multi-block-size selection in `encode_stream`.
- [ ] Phase 3A landed: mid-side stereo.
- [ ] Phase 4 landed: `std::simd` autocorrelation (with measure).
- [ ] Sine-wave ratio ≤ 20% (was 28.79%, libFLAC = 18.59%).
- [ ] All 25+ existing omnizip-flac tests still pass.
- [ ] `#![forbid(unsafe_code)]` preserved.

## License

libFLAC is BSD-3-Clause. Ported code carries the Xiph copyright
notice in the relevant Rust files. omnizip-flac remains dual
MIT OR Apache-2.0; the ported code is BSD-compatible with both.

## References

- Xiph libFLAC source: https://github.com/xiph/flac (mirrored at
  `~/src/external/flac/`)
- FLAC format spec: https://xiph.org/flac/format.html
- Coalson, J. (2003). *FLAC — Free Lossless Audio Codec.*
