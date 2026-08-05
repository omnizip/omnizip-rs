# TODO 115: Shared bitwriter/bitreader (DRY)

## Problem

Bit packers/unpackers are duplicated across at least six crates:

| Crate | BitWriter | BitReader |
|-------|-----------|-----------|
| omnizip-flac | MSB-first custom | LSB-first custom |
| omnizip-ricepp | MSB-first custom | MSB-first custom |
| omnizip-libdeflate | LSB-first custom | LSB-first custom |
| omnizip-bzip2 | MSB-first custom | MSB-first custom |
| omnizip-zstd | LSB-first FSE-specific | LSB-first FSE-specific |
| omnizip-lzma | (range coder has its own) | (range decoder has its own) |

Each implementation is ~150-300 lines of byte/bit manipulation with
subtle differences in:
- Bit order (MSB vs LSB first)
- Refill strategy (1-byte vs multi-byte, with/without zero-padding)
- API surface (mutable struct vs builder)

## Proposed fix

`omnizip-codecs/src/bitstream.rs`:

```rust
pub struct BitWriter<M: BitOrder> { /* ... */ }
pub struct BitReader<'a, M: BitOrder> { /* ... */ }

pub trait BitOrder: Sealed {
    const MSB_FIRST: bool;
}

pub enum MsbFirst {}
pub enum LsbFirst {}

impl BitOrder for MsbFirst { const MSB_FIRST: bool = true; }
impl BitOrder for LsbFirst { const MSB_FIRST: bool = false; }

impl<M: BitOrder> BitWriter<M> {
    pub fn write_bits(&mut self, value: u64, nbits: u32);
    pub fn write_signed(&mut self, value: i64, nbits: u32);
    pub fn write_unary(&mut self, value: u32);  // FLAC uses 0-terminated
    pub fn flush_byte_aligned(&mut self);
    pub fn finish(self) -> Vec<u8>;
}
```

Each codec migrates by replacing its private BitWriter/BitReader with
the shared one, specialising only on `MsbFirst` vs `LsbFirst`.

## Acceptance criteria

- [ ] `omnizip-codecs::bitstream::{BitWriter, BitReader}` lands.
- [ ] All codecs migrate.
- [ ] All codec tests pass bit-identical.
- [ ] Workspace LOC drops by ≥ 1000.

## Priority

P2 — DRY win, but lower priority than match finder (114) because
the differences between codecs are larger.
