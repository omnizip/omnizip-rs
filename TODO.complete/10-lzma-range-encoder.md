# 10 — LZMA range encoder

**Status**: ❌ Pending. Foundation for all LZMA encoders.

## Source

- `omnizip/lib/omnizip/algorithms/lzma/range_encoder.rb` (202 LOC)
- `omnizip/lib/omnizip/algorithms/lzma/xz_range_encoder.rb`
- `omnizip/lib/omnizip/algorithms/lzma/xz_range_encoder_exact.rb`
- `omnizip/lib/omnizip/algorithms/lzma/xz_buffered_range_encoder.rb`

## Architecture

```rust
pub struct RangeEncoder<W: Write> {
    out: W,
    low: u64,
    range: u32,
    cache: u32,
    cache_size: u64,
}

impl<W: Write> RangeEncoder<W> {
    pub fn new(out: W) -> Self;
    pub fn encode_bit(&mut self, prob: &mut BitModel, bit: u32);
    pub fn encode_bittree(&mut self, prob: &mut [BitModel], symbol: u32);
    pub fn encode_direct(&mut self, value: u32, num_bits: u32);
    pub fn flush(&mut self);
}
```

## Determinism requirements

- No floating-point arithmetic anywhere. All probabilities are
  `u16` (BitModel).
- All output is written via the `Write` trait — no buffering that
  depends on `Vec` reallocation order.
- `flush` must produce identical bytes every call.

## Files

- `omnizip-lzma/src/range_coder/encoder.rs` — mirror of `decoder.rs`
- `omnizip-lzma/src/range_coder/mod.rs` — re-export `RangeEncoder`

## Tests

- Round-trip: feed the same bits through `RangeEncoder` then
  `RangeDecoder`; assert identical.
- Determinism: encode the same bit sequence 10× → identical output.
- Overflow handling: encode 32 zero bits in a row; no panics.

## Acceptance

- 50+ unit tests pass.
- Used by task [11] (literal/length/distance encoders) without
  modifications.
