# 258 — Shared Bitstream Module (Architectural: DRY)

- **Priority:** P3 (architecture quality)
- **Crate:** `omnizip-codecs/src/bitstream.rs`
- **Depends on:** none
- **Estimated effort:** 2 days

## Problem

Bit-level read/write logic is reimplemented in every codec:

| Crate | File | Approx LOC |
|---|---|---|
| omnizip-brotli | `BitWriter` (encoder) + `BitReader` (decoder) | ~200 |
| omnizip-lzma | `range_encoder.rs` + `range_decoder.rs` | ~400 |
| omnizip-zstd | `bitstream.rs` | ~250 |
| omnizip-libdeflate | `bitstream.rs` | ~150 |
| omnizip-codecs | `bitstream.rs` (existing shared) | ~200 |

Total: ~1,200 LOC, of which ~600 is duplicated.

Bugs in one (e.g., the recent bit-writer flush fix in brotli) must
be fixed in each.

## Design

### Two existing patterns

The shared `omnizip-codecs::bitstream` already has `BitReader` and
`BitWriter`. The issue is that per-codec versions exist alongside
with subtly different APIs.

The path forward:

1. **Audit the shared module** for missing capabilities.
2. **Extend shared** to cover all use cases.
3. **Migrate per-codec** versions to use shared.

### API unification

```rust
pub struct BitWriter { /* shared */ }
pub struct BitReader<'a> { /* shared */ }

impl BitWriter {
    pub fn new() -> Self;
    pub fn write_bits(&mut self, value: u32, nbits: u32);
    pub fn write_bytes(&mut self, bytes: &[u8]);
    pub fn flush(&mut self) -> Vec<u8>;
    pub fn bit_position(&self) -> u64;
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self;
    pub fn read_bits(&mut self, nbits: u32) -> u32;
    pub fn read_bytes(&mut self, nbytes: usize) -> &'a [u8];
    pub fn bit_position(&self) -> u64;
    pub fn align_to_byte(&mut self);
}
```

### Per-codec migration

For each codec, in priority order:

1. **omnizip-brotli**: replace local `BitWriter`/`BitReader` with
   shared. Brotli uses MSB-first bit packing.
2. **omnizip-libdeflate**: DEFLATE uses LSB-first. Shared module
   must support both orders.
3. **omnizip-zstd**: already uses shared `BitReader`. Just verify
   `BitWriter` is also shared.
4. **omnizip-lzma**: range coder uses arithmetic coding, not bit
   packing. Different abstraction — keep separate but ensure the
   byte-level writer it builds on uses shared.

### Bit order parameter

```rust
pub enum BitOrder {
    /// Most-significant bit first. Used by Brotli.
    MSB,
    /// Least-significant bit first. Used by DEFLATE, ZSTD.
    LSB,
}

impl BitWriter {
    pub fn new(order: BitOrder) -> Self;
}
```

## Acceptance criteria

- [ ] Shared `BitReader` / `BitWriter` support both bit orders.
- [ ] Brotli migrated; local impl removed.
- [ ] libdeflate migrated.
- [ ] ZSTD fully uses shared.
- [ ] All workspace tests pass byte-identical.
- [ ] LOC reduction: ~400 lines removed across codecs.

## Why this matters

Bit I/O is the most fundamental shared primitive. Duplicating it
means N places where a flush bug could land, N places to optimize,
N APIs to learn. Centralizing is the foundation for all other
DRY work.
