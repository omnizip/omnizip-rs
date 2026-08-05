# TODO 169: Brotli Huffman encoder wire-format debugging

## Problem

The pure-Rust Brotli Huffman-coded encoder (`encode_huffman` in
`omnizip-brotli/src/encoder.rs`) compiles and produces output for
inputs with ≤ 4 unique byte values, but the upstream `brotli -d`
reference decoder rejects the output with "Invalid Data" for all
tested inputs that have LZ77 matches.

Inputs WITHOUT matches (e.g., "aaaabbbb") decode correctly via the
uncompressed fallback. Inputs WITH matches (e.g., "aaaaa",
"abababab", "abcabcabc") fail.

## What's been verified correct

The following pieces work in isolation (all unit-tested):

- `static_codes.rs`: 704-symbol command + 64-symbol distance Huffman
  tables match upstream constants exactly.
- `commands.rs`: `GetInsertLengthCode`, `GetCopyLengthCode`,
  `combine_length_codes`, `compute_distance_code`,
  `prefix_encode_copy_distance` are line-by-line ports.
- `huffman.rs`: `build_huffman_tree` (min-heap + Kraft inequality
  repair), `convert_bit_depths_to_symbols`, `store_simple_form`.
- Frame header, metablock header, 13-zero-bit prelude, static
  command tree (59 bits), static distance tree (28 bits) all match
  upstream bit-for-bit (verified by manual trace against actual
  upstream output bytes).

## Suspected bugs (in priority order)

### 1. dist_cache initialization mismatch

**Status:** Partially addressed.

The decoder's `dist_rb` starts at `[16, 15, 11, 4]` (state.rs:295).
The encoder's `ComputeDistanceCode` expects `dist_cache = [4, 11, 15, 16]`
(forward order). My encoder now initializes `dist_cache` to
`commands::INITIAL_DIST_CACHE = [4, 11, 15, 16]`.

However, the encoder's `Command::init_distance` call site (which I
mirrored) does NOT call `ComputeDistanceCode` first. It calls
`PrefixEncodeCopyDistance` directly with a `distance_code` that's
already been computed elsewhere. The full call chain in upstream is:

```
backward_references → ComputeDistanceCode → distance_code
                  → Command::init → PrefixEncodeCopyDistance(distance_code, ...)
```

My port conflates these. Verify by adding a unit test that round-trips
`compute_distance_code(distance, MAX, &cache)` → `prefix_encode_copy_distance(code, 0, 0)`
→ decoder reconstruction → original distance.

### 2. Static command tree bit pattern

**Status:** Suspected.

The 56+3 bit pattern `0x0092_6244_1630_7003` followed by 3 zero bits
is taken verbatim from upstream `StoreStaticCommandHuffmanTree`. The
decoder should recognize this as "use the static command Huffman
table" (K_STATIC_COMMAND_CODE_DEPTH/BITS).

However, the static distance tree constant `0x0369_dc03` (28 bits)
similarly should signal "use the static distance Huffman table".
Both patterns need verification against the decoder's recognition
logic.

### 3. cmd_prefix >= 128 distance emission condition

**Status:** Probably correct but worth verifying.

My encoder emits the distance Huffman code only when
`cmd.copy_len > 0 && cmd_code >= 128`. Upstream uses the same
condition. But the boundary at 128 depends on how
`combine_length_codes` packs the (insert_code, copy_code, use_last)
into the 704-symbol command alphabet.

For (insert=1, copy=4, use_last=false), my code computes cmd_code=138
(>= 128, so distance emitted). For (insert=4, copy=2, use_last=true),
my code computes cmd_code=32 (< 128, no distance emitted).

The decoder side: for commands with `dist_prefix & 0x3ff == 0`, the
"use last distance" path is taken. Otherwise the distance Huffman
code is read. The boundary at 128 in upstream's encoder corresponds
to specific (insert, copy) combinations.

### 4. Simple-form Huffman symbol bit width

**Status:** Possibly incorrect.

Upstream encoder passes `max_bits = 8` to
`BrotliBuildAndStoreHuffmanTreeFast`. The decoder reads
`Log2Floor(alphabet_size - 1) = Log2Floor(255) = 7` bits per simple-form
symbol. There's a 1-bit mismatch.

Either:
- The upstream encoder is intentionally over-width (8 bits) and the
  decoder reads the low 7 bits (ignoring the high bit).
- The upstream encoder's `max_bits` parameter means something
  different than "bits per symbol value".

Verify by checking what `max_bits=8` actually controls in upstream.

## Debugging strategy

1. **Trace upstream output byte-by-byte.** Take a known input like
   "hello world hello world" encoded via `brotli -c -q 6`. Decode
   each bit position against the brotli spec. This is the ground
   truth.

2. **Match upstream's exact bytes.** Once we know what bytes upstream
   produces, mirror that output exactly in our encoder. We don't
   need to use the same algorithm — just produce the same bytes.

3. **Add a property test** that decodes our output via the upstream
   `brotli` crate (already wired as a dev-dep) and asserts
   round-trip equality. Once this passes for all property fixtures,
   the encoder is correct.

4. **Use the brotli-decompressor source** as the authoritative spec.
   The decoder at `~/src/external/brotli-decompressor/src/decode.rs`
   shows exactly what bits it expects.

## Acceptance criteria

- [ ] `cargo test -p omnizip-brotli --lib huffman_output_decodes_via_upstream_brotli`
      passes (all 6 inputs decode successfully via upstream brotli).
- [ ] `brotli -d` accepts our encoder output for inputs with ≤ 4
      unique byte values.
- [ ] Encoder output is smaller than uncompressed for text inputs
      ≥ 100 bytes with low entropy.
- [ ] Determinism preserved (same input → same output bytes).

## Stretch goals

- [ ] Complex-form Huffman tree emitter (for inputs with > 4 unique
      bytes). Port `BrotliStoreHuffmanTree` from upstream
      `brotli_bit_stream.rs:835`.
- [ ] Full LZ77 with chain walking (not just single-probe).
- [ ] Quality levels 0-11 mapped to different strategies.

## Priority

P0 — the user has explicitly requested Brotli be "fully finished".
This TODO blocks that goal. The foundation (modules + tests) is in
place; this is purely wire-format debugging.

## Time estimate

1-2 days of focused debugging. The bug is small (likely a single
bit-position off-by-one or a missing dist_cache update); finding it
requires patient state-machine tracing against the decoder.