# ZSTD offset symbol indexing — 0-indexed FSE symbols

## Status

**Resolved.** The Rust port uses 0-indexed offset symbols, matching
the C reference. RFC 8878's prose uses 1-indexed "Offset_Value"; the
two are reconciled by the mapping `Offset_Value = FSE_symbol + 1`.

## Affected code

`omnizip-zstd/src/sequences.rs` — `decode_offset_value`,
`SequenceExecutor::resolve_offset`.

## What RFC 8878 says

RFC 8878 §3.1.2.3.3 describes offset decoding in terms of
"Offset_Value":

> Offset_Value 1: offset = repeat_offset_1
> Offset_Value 2: offset = repeat_offset_2
> Offset_Value 3: offset = repeat_offset_3
> Offset_Value > 3: N = Offset_Value - 3;
>                   offset = (1 << N) + readNBits(N)

Reading the prose literally, the repeat offsets are values 1, 2, 3.

## What the C reference does

The C reference (`lib/decompress/zstd_decompress_block.c`,
`ZSTD_execSequence`) treats the FSE-decoded offset symbol as
0-indexed:

```c
if (ofCode <= 2) {
    // Repeat offset: 0 → rep[0], 1 → rep[1], 2 → rep[2]
    offset = rep[ofCode];
} else {
    // New offset
    U32 const offBase = ofCode - 2;
    offset = (1 << offBase) + BIT_readBitsFast(&seqState->DStream, offBase);
}
```

The FSE symbol values are 0-indexed: symbol 0 maps to the first
repeat offset, symbol 1 to the second, symbol 2 to the third,
symbol ≥ 3 introduces a new offset.

The RFC's "Offset_Value" is `FSE_symbol + 1`: symbol 0 → Offset_Value 1,
symbol 1 → Offset_Value 2, etc.

## What the Rust port does

`decode_offset_value` and `SequenceExecutor::resolve_offset` both use
the C reference's 0-indexed convention:

```rust
fn decode_offset_value(symbol: u32, bitstream: &mut BitStream<'_>) -> u32 {
    if symbol <= 2 {
        return symbol;  // Repeat-offset code; resolved by executor
    }
    let n = symbol - 2;
    let extra = bitstream.read_bits(n);
    (1u32 << n) + extra
}

fn resolve_offset(&mut self, offset_symbol: u32) -> u32 {
    match offset_symbol {
        0 => self.repeat_offsets[0],
        1 => { /* swap rep[0] ↔ rep[1] */ }
        2 => { /* rotate rep[2] → rep[0] */ }
        actual => { /* new offset; rotate all */ }
    }
}
```

## What the Ruby port does (bug)

The Ruby's `decode_offset` ignores extra bits entirely and uses
1-indexed conventions inconsistently:

```ruby
def decode_offset(symbol, _bitstream)
  return symbol if symbol <= 3   # Treats 1, 2, 3 as repeats
  symbol - 3                     # Wrong: should read extra bits
end
```

See `../omnizip/BUGREPORT.05-offset-extra-bits-ignored.md`.

## Why the divergence exists

The divergence is between RFC prose and C reference. The Rust port
matches the C reference because:

1. The C reference is the actual interoperable implementation.
2. The RFC prose is ambiguous about whether "Offset_Value" is the raw
   FSE symbol or a 1-indexed presentation value.
3. Empirically, real `.zst` files only decode correctly with the
   0-indexed convention.

## Impact

Before the fix, the executor returned distance 0 for any sequence
whose FSE offset symbol was 0 (treating it as invalid). After the
fix, symbol 0 correctly maps to `repeat_offsets[0]`.
