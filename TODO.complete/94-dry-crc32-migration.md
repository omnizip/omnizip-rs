# 94 — DRY: migrate per-crate CRC-32 impls to shared checksum module

**Priority:** Medium (DRY cleanup)
**Source:** `omnizip-codecs::checksum` (landed in TODO 82)

## Context

Three crates currently ship their own CRC-32 implementation:

- `omnizip-bzip2/src/crc32.rs`
- `omnizip-deflate/src/lib.rs` (inline)
- `omnizip-lzma/src/crc32.rs`

The shared `omnizip_codecs::checksum::crc32_iso_hdlc` (slice-by-8,
~3× faster than byte-by-byte) landed in TODO 82. The per-crate
versions should now delegate to it.

This is a pure DRY refactor — behavior is byte-identical (verified
by the existing CRC tests in each crate + the differential test
against Python's `zlib.crc32`).

## Approach

For each crate:

1. Add `omnizip-codecs` to its dependencies in `Cargo.toml` (most
   crates already depend on it).
2. Replace the local CRC table + function with a thin re-export:

   ```rust
   pub use omnizip_codecs::checksum::crc32_iso_hdlc as crc32;
   ```

3. Remove the local table-generation code.
4. Run the crate's tests; verify 0 failures.

For incremental-update callers (streaming checksums), the shared
module also exposes `crc32_iso_hdlc_update(state, data)`. Migrate
those too where applicable.

## Acceptance criteria

- [ ] All three crates use the shared CRC-32.
- [ ] No `POLY = 0xEDB8_8320` constant remains in codec crates.
- [ ] Per-crate tests still pass.
- [ ] `cargo clippy --workspace` clean of new warnings.

## Files

- `omnizip-bzip2/src/crc32.rs` — replace with re-export
- `omnizip-deflate/src/lib.rs` — inline CRC removed
- `omnizip-lzma/src/crc32.rs` — replace with re-export
