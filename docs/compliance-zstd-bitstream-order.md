# ZSTD reverse bitstream — LSB-first within each byte

## Status

**Resolved in Rust.** The Ruby port still has the bug (see
`../omnizip/BUGREPORT.10-reverse-bitstream-wrong-bit-order.md`).

## Affected code

`omnizip-zstd/src/fse/bitstream.rs` — `BitStream::read_single_bit`.

## What RFC 8878 says

RFC 8878 §4.1.1 (reading the backward bitstream):

> The bitstream is read backward, starting from the end.
> Within each byte, the bit positioned at index 0 (the least
> significant bit) is read first.

So the first bit read from `[B0, B1, B2]` is bit 0 (LSB) of B2 (the
last byte).

## What the C reference does

The C reference (`lib/common/bitstream.h`, `BIT_initDStream` /
`BIT_readBits`) loads a `bitContainer` (a `size_t` word) in
little-endian form from the end of the stream, then extracts bits
from the low end of the container. The net effect: within each byte,
bits are consumed LSB first, matching the RFC.

## What the Rust port does

`BitStream::read_single_bit` tracks `bits_consumed` as a forward
counter and maps to `(byte_from_end, bit_within_byte)`:

```rust
let byte_within_last = i / 8;        // 0 = last byte
let bit_within_byte = i % 8;         // 0 = LSB
let memory_byte_index = self.data.len() - 1 - byte_within_last;
```

This produces the RFC-correct order: first bit = LSB of last byte.

## What the Ruby port does (bug)

The Ruby's `FSE::BitStream#read_single_bit`:

```ruby
@bit_position -= 1
byte_index = @bit_position / 8
bit_index = @bit_position % 8
(byte >> bit_index) & 1
```

With `@bit_position` starting at `data.bytesize * 8`, the first read
sets `@bit_position = data.bytesize * 8 - 1`, so `bit_index =
(data.bytesize * 8 - 1) % 8 = 7` — the **MSB** of the last byte. This
inverts every FSE state initialisation.

See `../omnizip/BUGREPORT.10-reverse-bitstream-wrong-bit-order.md`
for the Ruby-side fix proposal.

## Why the divergence exists

The Ruby bug was discovered while debugging the `test-aaaa.zst`
fixture in omnizip-rs. The Rust port was originally written to match
the Ruby (MSB-first), then corrected to match the RFC (LSB-first)
once the bug was identified.

## Impact

Before the fix, every FSE decode produced wrong symbols because the
initial state was computed from the wrong bits. After the fix, the
initial state is correct (verified by hand-tracing the bitstream).

The remaining `test-aaaa.zst` failure is downstream of this fix — see
[compliance-zstd-fse-table.md](compliance-zstd-fse-table.md).
