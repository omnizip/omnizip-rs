# 23 — ZSTD sequences encoder

**Status**: ❌ Pending. Depends on [22].

## Source

- Embedded in `omnizip/lib/omnizip/algorithms/zstandard/encoder.rb` (228 LOC).

## Architecture

Encodes a `Vec<Sequence>` into the wire format:

```rust
pub fn encode_sequences(sequences: &[Sequence]) -> Vec<u8>;
```

1. Sequence count (1-3 bytes per RFC §3.1.1.3.2.1).
2. Symbol-compression-modes byte.
3. Per-mode: PREDEFINED (no extra bytes), RLE (1 byte), FSE (full
   table), REPEAT (no bytes, reuse previous).
4. FSE bitstream containing LL / OF / ML symbols + extra bits.

## Sequence construction (LZ77 stage)

The encoder builds sequences from a match finder (similar to LZMA but
for ZSTD's sequence format):

```rust
pub struct SequenceBuilder {
    reps: [u32; 3],
    output: Vec<u8>,
}

impl SequenceBuilder {
    pub fn emit_literal(&mut self, bytes: &[u8]);
    pub fn emit_match(&mut self, length: u32, offset: u32);
    pub fn finish(self) -> (Vec<u8>, Vec<Sequence>);  // (literals, sequences)
}
```

## Files

- `omnizip-zstd/src/sequences/encoder.rs`
- `omnizip-zstd/src/encoder/match_finder.rs` (LZ77 stage)
- Re-export from `sequences/mod.rs` and `encoder/mod.rs`

## Tests

- Round-trip: `decode_sequences_section(encode_sequences(s)) == s`.
- Determinism: encode same sequences 10× → identical output.

## Acceptance

- Used by task [24] (frame encoder).
