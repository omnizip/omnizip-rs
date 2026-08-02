# 02 — Sliding-window BitStream + FSE-from-stream

**Status**: 🚧 Blocked. Current `BitStream` loads at most 7 bytes into
a `u64` container. FSE weight bitstreams are typically 6–25 bytes;
FSE sequence bitstreams can be hundreds of bytes.

## Architecture

Replace `omnizip-zstd/src/fse/bitstream.rs::BitStream` with a proper
sliding-window reader matching the C reference `BIT_DStream`
(`~/src/external/zstd/lib/common/bitstream.h`).

### Public API stays the same

```rust
pub struct BitStream<'a> { /* fields private */ }
impl<'a> BitStream<'a> {
    pub fn new(data: &'a [u8]) -> Self;
    pub fn read_bits(&mut self, count: u32) -> u32;
    pub fn peek_bits(&mut self, count: u32) -> u32;
    pub fn remaining_bits(&self) -> usize;
    pub fn is_exhausted(&self) -> bool;
}
```

### Internal state

```rust
struct BitStream<'a> {
    data: &'a [u8],
    /// Position of the next byte to load into the container (advanced
    /// as bits are consumed). Points at the BYTE AT OR AFTER the
    /// container's high edge.
    byte_pos: usize,
    /// Current 64-bit window with byte[0] at the lowest bits, loaded
    /// MSB-first from the END of the data.
    container: u64,
    /// Number of bits consumed from the HIGH end of the container.
    bits_consumed: u32,
}
```

### Reload protocol (C reference)

When `bits_consumed >= 8`, shift the container by a byte-aligned
amount and load a fresh byte from the next position in `data`. The
end-mark (trailing zero bits in the last byte) is computed once in
`new()`.

## Files to change

- `omnizip-zstd/src/fse/bitstream.rs` — rewrite BitStream (keep API).
- `omnizip-zstd/src/fse/from_stream.rs` — already exists; verify it
  still works with the new BitStream.
- `omnizip-zstd/src/huffman/weights.rs` — re-enable
  `read_fse_compressed_weights`.
- `omnizip-zstd/src/sequences.rs::get_table` — wire up `MODE_FSE`
  branch to call `read_fse_table`.

## Tests

- Existing `bitstream::tests` must pass.
- Add a 50-byte random stream test that round-trips through
  `new()` + N × `read_bits()`.
- `cargo test --test zstd_parity` should pass `huffman-compressed-larger.zst`.

## Acceptance

- `cargo test --workspace` passes (200+ tests).
- 11/11 zstd fixtures decode (was 7/11).
- `MODE_FSE` no longer returns `Unsupported`.
