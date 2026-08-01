# ZSTD literals size format — C-reference interpretation

## Status

**Resolved.** The Rust port uses the C reference's bit layout for
the literals section header.

## Affected code

`omnizip-zstd/src/literals/mod.rs` — `decode_size_format_raw_rle`.

## What RFC 8878 says

RFC 8878 §3.1.1.3.1.1 (Literals_Section_Header layout):

> Byte 0:
>   Bits 6-7: Literals_Block_Type
>   Bits 0-5: Size_Format (interpretation depends on block type)

The RFC describes the Size_Format as occupying bits 0-5 of byte 0,
but the exact bit-slicing for the regenerated size is left somewhat
ambiguous in the prose. The normative reference is the C
implementation.

## What the C reference does

The C reference (`lib/decompress/zstd_decompress_block.c`,
`ZSTD_decodeLiteralsBlock`) parses the header as:

```c
U32 const lhc = MEM_readLE32(istart);   // First 4 bytes, little-endian
switch (lhc & 3) {                       // Bits 0-1
case 0: case 1:                          // Raw or RLE
    litEncType = (lhc >> 6) & 3;         // Bits 6-7
    if (lhc & 1) {                       // Bit 0 set → 2-byte header
        lhSize = 2;
        litSize = (lhc >> 4) & 0xFFF;    // Bits 4-15 (12 bits)
    } else {                             // Bit 0 clear → 1-byte header
        lhSize = 1;
        litSize = (lhc >> 3) & 0x1F;     // Bits 3-7 (5 bits)
    }
    ...
}
```

So for a 1-byte Raw/RLE header:
- Bit 0 selects 1-byte vs 2-byte header.
- Bits 3-7 (5 bits) encode the regenerated size.
- Bits 1-2 are unused.

For a 2-byte Raw/RLE header:
- Bits 4-15 (12 bits) encode the regenerated size.

## What the Rust port does

`decode_size_format_raw_rle` follows the C reference:

```rust
fn decode_size_format_raw_rle(header0: u8, input: &[u8]) -> Result<(u32, usize), ZstdError> {
    if header0 & 1 == 0 {
        // 1-byte header, regen_size = byte0 >> 3.
        Ok((u32::from(header0 >> 3), 1))
    } else {
        // 2-byte header, regen_size = (lhc >> 4) & 0xFFF.
        let lhc = u16::from_le_bytes([input[0], input[1]]);
        Ok((u32::from((lhc >> 4) & 0x0FFF), 2))
    }
}
```

## What the Ruby port does (bug)

The Ruby's `decode_raw` uses `header1 & 0x1F` (bits 0-4, 5 bits),
which includes the size-format selector bit in the size value and
uses the wrong bit slice. See
`../omnizip/BUGREPORT.08-literals-size-format-wrong.md`.

## Why the divergence exists

The Ruby port was written without consulting the C reference; the
author appears to have guessed the bit layout from the RFC prose.
The Rust port consults the C reference for the normative behaviour
because the RFC prose is ambiguous.

## Impact

Before the fix, every ZSTD frame with non-trivial literals decoded
the wrong regenerated size, causing the sequences section to
mis-align and the decoder to fail or produce wrong output.

After the fix, literals parse correctly for the `test-aaaa.zst`
fixture (verified: regen_size = 2 for the "aa" literal prefix).
