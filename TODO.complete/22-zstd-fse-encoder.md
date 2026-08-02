# 22 — ZSTD FSE encoder

**Status**: ❌ Pending.

## Source

- `omnizip/lib/omnizip/algorithms/zstandard/fse/encoder.rb` (referenced
  from `encoder.rb`; not a separate file in the Ruby).

## Architecture

The inverse of `fse/from_stream.rs::read_fse_table`:

```rust
pub fn build_fse_table(symbols: &[u8], accuracy_log: u8) -> Vec<u8>;
```

1. Compute normalized distribution from `symbols` (using the same
   algorithm the C reference uses for predefined tables).
2. Encode the distribution into the wire format (4-bit tableLog +
   per-symbol counts).
3. Return the byte stream.

Plus the FSE bitstream encoder:

```rust
pub fn encode_fse_bitstream(symbols: &[u8], table: &Table) -> Vec<u8>;
```

Builds the reverse-direction bitstream from the symbol sequence + FSE
table.

## Determinism

- Distribution normalization: use a fixed tie-breaking order (symbol
  index ascending).
- Bitstream write order: deterministic (reverse byte order, with the
  encoder's state-init bytes appended at the end).

## Files

- `omnizip-zstd/src/fse/encoder.rs`
- Re-export from `fse/mod.rs`

## Tests

- Round-trip: `read_fse_table(build_fse_table(...))` succeeds.
- Determinism: encode same symbols 10× → identical output.

## Acceptance

- Used by task [23] (sequences encoder).
