# TODO 161: Deflate64 encoder

## Problem

`omnizip-deflate64` is decode-only today (1,308 LOC of decoder +
container code). LimniFS reads Deflate64 streams (some legacy ZIP
files use it) but cannot write them.

## Scope

DEFLATE64 differs from DEFLATE in:
- 64 KB sliding window (vs 32 KB).
- Larger max match length (65538 vs 258).
- Two extra distance codes (30, 31) covering larger offsets.

The encoder reuses the existing `omnizip-libdeflate` LZ77 pipeline
with a wider window + match length cap.

## Implementation plan

1. Lift the 32 KB window cap in `deflate_lz77.rs` to 64 KB.
2. Allow match length up to 65538 in the LZ77 token type.
3. Add distance codes 30/31 to the symbol table.
4. New `omnizip-deflate64/src/encoder.rs` that wraps the LZ77 +
   Huffman output.

## Acceptance criteria

- [ ] `Deflate64Codec::compress` lands.
- [ ] Round-trips through 7-Zip's Deflate64 decoder.
- [ ] Round-trips through own decoder.
- [ ] Ratio ≥ DEFLATE level 9 on text fixtures.

## Priority

P2 — LimniFS reads but doesn't write Deflate64. Encode unblocks
future use cases.
