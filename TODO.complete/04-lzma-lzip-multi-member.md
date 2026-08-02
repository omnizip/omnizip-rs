# 04 — Lzip multi-member decode

**Status**: ❌ Pending. 1/8 fixtures decode; 7/8 fail because the
decoder stops at the first member and treats the rest as garbage.

## Spec

`.lz` files are one or more concatenated lzip members. Each member:

```
Magic           4 bytes: "LZIP"
Version         1 byte
Dict_Size_Code  1 byte
LZMA1 stream    variable
Trailer         20 bytes:
                   CRC32 (LE u32)
                   Data_Size (LE u64)
                   Member_Size (LE u64)  ← use this to skip ahead
```

## Fix

`omnizip-lzma/src/lzip.rs::lzip_decompress` currently decodes one
member and returns. It should loop:

```rust
let mut output = Vec::new();
let mut cursor = 0;
while cursor < input.len() {
    let (member_output, consumed) = decode_one_member(&input[cursor..])?;
    output.extend_from_slice(&member_output);
    cursor += consumed;
    // Verify member_size matches actual consumed bytes.
    // Verify CRC32 (after task XXHash32-equivalent for CRC32 is wired).
}
Ok(output)
```

The single-member decoder must return the number of bytes consumed
(read the trailer's Member_Size field).

## Files

- `omnizip-lzma/src/lzip.rs` — refactor `lzip_decompress` into a
  per-member helper + a loop.
- `omnizip-lzma/src/crc32.rs` — already exists; wire into the
  per-member CRC check.

## Tests

- `tests/fixtures/lzma/good-*.lz` (currently 7/8 fail) — all should
  decode byte-identical to `xz -d` oracle.
- Multi-member fixture (hand-crafted concatenation of two single
  members) round-trips.
- Trailing data (a few bytes after a valid member) is rejected with
  `Corrupt` rather than silently swallowed.

## Acceptance

- All 8 `.lz` fixtures pass differential parity with `xz -d`.
- CRC mismatch in the trailer returns `Corrupt`.
