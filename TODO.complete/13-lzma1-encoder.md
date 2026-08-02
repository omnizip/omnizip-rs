# 13 — LZMA1 packet encoder

**Status**: ❌ Pending. Depends on [10], [11], [12].

## Source

- `omnizip/lib/omnizip/algorithms/lzma/encoder.rb` (139 LOC)

## Architecture

Greedy match-then-emit loop:

```rust
pub struct Lzma1Encoder {
    rc: RangeEncoder<Vec<u8>>,
    literal_encoder: LiteralEncoder,
    length_encoder: LengthEncoder,
    rep_length_encoder: LengthEncoder,
    distance_encoder: DistanceEncoder,
    state: LzmaState,
    reps: [u32; 4],
    lc: u32,
    lp: u32,
    pb: u32,
}

impl Lzma1Encoder {
    pub fn new(lc: u32, lp: u32, pb: u32, dict_size: u32) -> Self;
    pub fn encode(&mut self, input: &[u8]) -> Vec<u8>;
    pub fn encode_chunk(&mut self, input: &[u8], output: &mut Vec<u8>);
}
```

## Algorithm

```
mf = MatchFinder::new(input)
while let Some(m) = mf.next_match() {
    if m is a rep (distance matches one of reps[0..4]) {
        encode_rep_match(m)
    } else {
        encode_regular_match(m)
    }
}
encode_eopm()  // end-of-payload marker (0xFFFFFFFF distance)
rc.flush()
```

## Determinism

- Greedy parser: no early-exit heuristics that depend on timing.
- All state updates are sequential and deterministic.

## Files

- `omnizip-lzma/src/encoder/lzma1.rs`
- `omnizip-lzma/src/encoder/mod.rs` — re-export

## Tests

- Round-trip: `Lzma1Decoder::decode(Lzma1Encoder::encode(x)) == x`
  for every fixture under `tests/fixtures/lzma/`.
- Determinism: encode same input 10× → byte-identical output.
- Differential: encode via Ruby ref + decode via Rust → identical.

## Acceptance

- 50+ unit tests pass.
- Round-trips all `.lzma` fixtures.
