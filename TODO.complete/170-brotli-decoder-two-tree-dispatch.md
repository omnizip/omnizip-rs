# 170: Brotli Decoder — Round-trip with `compress_fragment_two_pass` encoder

## Priority: P0 (release-blocker for self-contained round-trip)

## Status: done

## Resolution (2026-08-06)

Two decoder bugs in `omnizip-brotli/src/decoder.rs` prevented round-trip
of `compress_fragment_two_pass` output on inputs longer than the
uncompressed-metablock threshold:

1. **`decode_distance_from_code` skipped extra-bit reads for `distval ≤ 0`.**
   The special case returned `distance = 1` without consuming the
   `nbits = 1` extra bit that upstream `ReadDistanceInternal` always
   reads on the NPOSTFIX=0 fast path. Each long-distance code
   truncated the bitstream by 1+ bits and cascaded into garbage
   symbol reads. Mirrored upstream's formula verbatim:
   ```
   distval = dist_code − NUM_DISTANCE_SHORT_CODES
   nbits  = (distval >> 1) + 1
   offset = ((2 + (distval & 1)) << nbits) − 4
   distance = num_direct + offset + ReadBits(nbits) − 15
   ```

2. **The command loop always read a trailing DISTANCE + COPY after the
   INSERT literals.** `compress_fragment_two_pass`'s final command on
   inputs with a tail of uncompressed literals is INSERT-only — no
   trailing distance/copy follows. The decoder read into the
   `ISLAST + ISLASTEMPTY` terminator, derived a phantom distance from
   garbage bits, and overran `mlen`. Mirrored upstream's check at
   `ProcessCommandsInternal` line ~2437: if `output.len() ≥ mlen` after
   the literals, exit without reading distance.

With both fixes the existing `kCmdLut`-based command loop is correct for
the encoder's separate-INSERT/DISTANCE/COPY-LD command pattern, because
upstream's `BuildAndStoreCommandPrefixCode` scatters `depth[0..64]` into
the 704-entry table at positions whose canonical-code sort order matches
the encoder's 64-symbol tree. No two-tree state machine is needed.

## Verification

- `brotli_round_trips_property_fixtures` un-ignored; all 17 fixtures pass.
- `brotli -d` continues to validate encoder output on the same fixtures.
- Full workspace tests pass on Linux + macOS + Windows.

## Follow-ups

- **171** — dead-code cleanup in `omnizip-brotli/src/decoder.rs`.
- **172** — full RFC 7932 decoder (block types, context maps, NPOSTFIX).
- **173** — brotli decoder/conformance differential corpus.
