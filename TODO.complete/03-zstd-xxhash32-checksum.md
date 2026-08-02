# 03 — XXHash32 checksum verification

**Status**: ❌ Pending. `decoder.rs:155-163` accepts the 4 checksum
bytes but skips verification. A `TODO` is in the code.

## Source

- RFC 8878 §4.2.4 — frame checksum.
- C reference: `~/src/external/zstd/lib/common/xxhash.c`.
- Ruby reference: `omnizip/lib/omnizip/algorithms/zstandard/decoder.rb`
  uses a non-standard polynomial (see `BUGREPORT.06-xxhash32-wrong-algorithm.md`).
  Use the RFC/C implementation, not the Ruby.

## Algorithm (RFC)

XXHash32 with seed=0:
1. Initialize 4 accumulators (`v1..v4`) to `prime32_1 + prime32_2`,
   `prime32_2`, `0`, `-prime32_1`.
2. Process input in 16-byte blocks, mixing each accumulator with
   `+` / `*` of byte words.
3. Finalize: combine accumulators, mix in length, avalanche to
   32-bit result.

## Files

- New: `omnizip-zstd/src/xxhash.rs` — pure-Rust XXHash32 with `seed=0`.
- Modify: `omnizip-zstd/src/decoder.rs` — compute checksum of all
  decoded output and compare against the trailing 4 bytes.
- Modify: `omnizip-zstd/src/lib.rs` — re-export `xxhash32`.

## Public API

```rust
pub fn xxhash32(data: &[u8]) -> u32;       // seed = 0
pub fn xxhash32_seeded(data: &[u8], seed: u32) -> u32;
```

## Tests

- Known-answer vectors from the C reference test suite
  (`~/src/external/zstd/tests/xxhash.c`).
- Empty input: `0x02CC5D05`.
- 1-byte input (`b"a"`): `0x550D7EB6`.
- 14-byte input (`"Hello, world!"`): exact value TBD.

## Acceptance

- All KATs match.
- `decode_frame` returns `ZstdError::Corrupt` when the trailing 4
  checksum bytes don't match the computed hash of the output.
- Frames without the checksum flag (`descriptor.checksum == 0`)
  skip the check.
