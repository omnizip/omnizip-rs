# 59 — ZSTD FSE-encoded Huffman weights

## Gap

The Huffman literals encoder currently only supports **direct weight
encoding** (iSize ≥ 128, max 129 symbols). For binary inputs whose
alphabet spans more than 129 byte values (e.g. compressed data, random
bytes, or any input using byte values > 128), `encode_weights_direct`
returns `Err`, and the block encoder falls back to Raw literals —
leaving compression ratio on the table.

## Root cause

ZSTD's Huffman header byte is a `u8`:
- `header < 128` → FSE-compressed weights (stream size = header + 1).
- `header ≥ 128` → direct encoding (oSize = header − 127, max 128).

So direct encoding caps the alphabet at 129 symbols. Larger alphabets
require FSE-compressed weights.

## Implementation plan

1. **Build FSE normalised counts** from the weight histogram. The
   "symbols" of this FSE stream are weight values (1..=11), and their
   counts come from how many Huffman symbols share each weight. Use
   `fse::encoder::normalize_count` with tableLog ≤ 6.
2. **FSE-compress the weight array** using `fse::encoder::compress`.
   This produces the FSE bitstream + the normalised-count header.
3. **Wire into `huffman/encoder.rs`**: when `max_symbol > 128`, take
   the FSE path; otherwise prefer direct (simpler, deterministic).
4. **Match the C reference**: `HUF_compressWeights` in
   `~/src/external/zstd/lib/compress/huf_compress.c:133-190`.

## Files

- `omnizip-zstd/src/huffman/encoder.rs` — add `encode_weights_fse`.
- `omnizip-zstd/src/huffman/weights.rs` — decoder already supports
  FSE weights via `read_fse_compressed_weights`.

## Test strategy

- Round-trip a 50K binary input using all 256 byte values.
- Verify the header byte is < 128 (FSE path).
- Compare output size against direct encoding for a small alphabet
  (direct should win or tie).
