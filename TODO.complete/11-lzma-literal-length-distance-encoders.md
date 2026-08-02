# 11 — LZMA literal / length / distance encoders

**Status**: ❌ Pending. Depends on task [10] (range encoder).

## Source

- `omnizip/lib/omnizip/algorithms/lzma/literal_encoder.rb` (208 LOC)
- Length encoder: derived from `length_decoder.rb` with the reverse
  direction (encoder calls `RangeEncoder::encode_bit` instead of
  `RangeDecoder::decode_bit`).
- Distance encoder: derived from `distance_decoder.rb`.

## Architecture

Each encoder mirrors its decoder sibling:

```rust
pub struct LiteralEncoder {
    literal_models: Vec<BitModel>,
    lc: u32,
    lp: u32,
}

impl LiteralEncoder {
    pub fn new(lc: u32, lp: u32) -> Self;
    pub fn encode(
        &mut self,
        rc: &mut RangeEncoder<...>,
        state: &LzmaState,
        prev_byte: Option<u8>,
        output_so_far: &[u8],
        symbol: u8,
    );
}

pub struct LengthEncoder { pos_states: usize, ... }
pub struct DistanceEncoder { ... }
```

The encoder and decoder must use the SAME probability model layout
(indices into `literal_models`, slot selection for distance) — extract
those into shared `probability_models.rs` to enforce DRY.

## Determinism

- All BitModel accesses are sequential, no hash maps.
- `state` transitions are deterministic (no randomization).

## Files

- `omnizip-lzma/src/coder/literal_encoder.rs`
- `omnizip-lzma/src/coder/length_encoder.rs`
- `omnizip-lzma/src/coder/distance_encoder.rs`
- `omnizip-lzma/src/coder/mod.rs` — re-export all three

## Tests

- Round-trip: encode → decode → assert identical for each of:
  - 100 random bytes (covers literal encoder)
  - 100 length values 2..273 (length encoder)
  - All distance slots (distance encoder)
- Determinism: encode the same sequence 10× → identical output.

## Acceptance

- Used by task [13] (LZMA1 encoder) without API changes.
