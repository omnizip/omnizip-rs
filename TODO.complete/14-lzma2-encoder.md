# 14 — LZMA2 chunk encoder

**Status**: ❌ Pending. Depends on [13].

## Source

- Derived from `omnizip-lzma/src/lzma2.rs` decoder (the encoder is
  the decoder in reverse, structurally).

## Architecture

Chunk the input, encode each chunk with LZMA1, prepend a control
byte that signals reset level + uncompressed size:

```rust
pub struct Lzma2Encoder {
    inner: Lzma1Encoder,
    dict_size: u32,
}

impl Lzma2Encoder {
    pub fn new(lc: u32, lp: u32, pb: u32, dict_size: u32) -> Self;
    pub fn encode(&mut self, input: &[u8]) -> Vec<u8>;
}
```

## Output format

```text
For each chunk:
  control byte: 0 (end) | 1/2 (uncompressed, reset level) | 0x80-0xFF (LZMA, reset level)
  if LZMA chunk: 2 bytes uncompressed size (high 5 bits in control) + 2 bytes compressed size
  if uncompressed chunk: 2 bytes uncompressed size + raw bytes
End with control byte 0x00.
```

Reset levels:
- 0: state persists (for chunks within a reset group)
- 1: reset state + rep distances
- 2: reset state + read new lc/lp/pb
- 3: reset state + new lc/lp/pb + reset dictionary

## Files

- `omnizip-lzma/src/encoder/lzma2.rs`
- Re-export from `encoder/mod.rs`

## Tests

- Round-trip via `decode_lzma2_stream`.
- Multi-chunk output: input > 64 KiB produces ≥ 2 chunks.
- Determinism: same input → identical chunks.

## Acceptance

- Used by task [15] (XZ container encoder).
