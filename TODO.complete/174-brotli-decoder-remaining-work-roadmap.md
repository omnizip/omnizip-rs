# 174: Brotli Decoder — Remaining Work Roadmap

## Priority: P3

## Status: 98% complete — only q=10/q=11 edge cases remain.

## What landed (2026-08-07)

See [TODO 172](172-brotli-full-rfc-7932-decoder.md) for the full list.
Highlights since the last update:

- ✅ **kCmdLut** regenerated from upstream's `BrotliDecoderInitCmdLut`
  algorithm (was using an incorrect offline-generated table with 1415
  wrong entries).
- ✅ **Static dictionary** fully implemented (122784-byte dictionary.bin
  + 121 transforms via `dictionary_lookup`).
- ✅ **LZ77 dist_rb write-back** in both decoder paths (was missing,
  causing dist_rb_idx drift).
- ✅ **prev_code_len** semantics corrected in `read_complex_form` (only
  update on sym != 0).

## Differential test matrix (2026-08-07)

216/220 pass (98%). See TODO 172 for details.

## What remains

### q=10/q=11 on specific inputs (4 failures)

Failing inputs:
- `compression is the process of reducing the size of data` (55 bytes).
- `<html><body>Hello</body></html>` (30 bytes).

Error: `invalid code-length code lengths (space not consumed)`.

Root cause analysis: at the command Huffman table read position,
our decoder produces an over-complete prefix code (Kraft sum > 32).
The check matches upstream's `BROTLI_DECODER_ERROR_FORMAT_CL_SPACE`
exactly, yet the reference decoder accepts these streams.

This suggests a subtle bit-position drift earlier in the parse for
these specific stream patterns. Likely candidates:

1. **Distance context map reading** — the `max_run_length_prefix`
   computation may diverge from upstream's `DecodeContextMap` for
   certain RLE patterns.
2. **Inverse MTF transform** — the sliding-window algorithm may have
   an off-by-one for edge cases.
3. **Block-type tree reading** — when NBLTYPES=1, the trivial-path
   dispatch may skip a bit that upstream reads.

Debugging approach:
- Add per-bit trace logging to both our decoder and a debug build of
  upstream's `brotli-decompressor` (C reference).
- Compare bit positions after each major state transition.
- Identify the first divergence point.

### Step 2: Long-term hardening

- Stream API (incremental decode for streaming use cases).
- Custom dictionary support (currently ignored).
- Large-window mode (BROTLI_LARGE_MAX_WBITS).
- Compound dictionary (multi-dictionary attachments).

## Acceptance Criteria status

- [x] Decode all 11 brotli fixtures from upstream's test corpus.
- [x] Decode every `.br` produced by `brotli -q 1` through `brotli -q 9`.
- [ ] Decode every `.br` produced by `brotli -q 10`/`-q 11` (4 known
  failures on specific inputs).
