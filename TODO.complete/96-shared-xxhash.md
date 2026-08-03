# 96 — Shared XXHash-64 in omnizip-codecs

**Priority:** Low (DRY cleanup)
**Source:** `omnizip-zstd/src/xxhash.rs` (currently only used by ZSTD)

## Context

XXHash-64 is the ZSTD frame checksum. The implementation in
`omnizip-zstd/src/xxhash.rs` (~250 LOC) is currently ZSTD-private.
Other codecs (BLOSC, FSST in some modes) may want to use XXHash-64
for content-defined identity.

Like CRC-32 (TODO 82, 94), this should live in
`omnizip-codecs::checksum` as the canonical shared impl.

## Approach

1. Move `omnizip-zstd/src/xxhash.rs` to
   `omnizip-codecs/src/xxhash.rs`.
2. Update `omnizip-codecs/src/lib.rs`:

   ```rust
   pub mod xxhash;
   pub use xxhash::{xxhash32, xxhash64, XxHasher64};
   ```

3. In `omnizip-zstd/src/xxhash.rs`, replace the contents with a
   re-export:

   ```rust
   pub use omnizip_codecs::xxhash::{xxhash32, xxhash64};
   ```

4. Run omnizip-zstd tests; verify the frame checksum still validates.

## Acceptance criteria

- [ ] `omnizip-codecs::xxhash` module published.
- [ ] `omnizip-zstd` consumes the shared impl via re-export.
- [ ] ZSTD frame-checksum test (`zstd_frame_checksum_of_1mib_zeros_matches_fixture`)
      still passes.
- [ ] No code duplication between crates.

## Files

- `omnizip-codecs/src/xxhash.rs` — new (moved from omnizip-zstd)
- `omnizip-codecs/src/lib.rs` — re-export
- `omnizip-zstd/src/xxhash.rs` — replace with re-export
