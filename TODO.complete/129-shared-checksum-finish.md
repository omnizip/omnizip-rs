# TODO 129: Shared checksum module — finish DRY sweep

## Problem

Checksums (CRC-32, XXHash-32, XXHash-64, Adler-32) are scattered
across crates with subtle differences:

- LZMA: CRC-32 in `omnizip-lzma/src/crc32.rs`.
- ZSTD: XXHash-64 in `omnizip-zstd/src/xxhash.rs`.
- DEFLATE / libdeflate: Adler-32 in `omnizip-libdeflate/src/lib.rs`.
- BZIP2: CRC-32 (different poly) in `omnizip-bzip2/src/bz2/crc32.rs`.
- omnizip-codecs already has CRC-32, XXHash-32/64 (TODOs 94, 96
  landed).

## Proposed fix

Migrate every crate to use the shared checksums from
`omnizip-codecs::checksum` and `omnizip-codecs::xxhash`.

For CRC-32, the shared module has the standard ISO-HDLC polynomial.
BZIP2 uses a different polynomial — either add a `Bzip2` variant to
the shared module or keep BZIP2's local impl as a documented
exception.

Adler-32 is currently inline in `omnizip-libdeflate`; promote it to
`omnizip-codecs::checksum::adler32`.

## Acceptance criteria

- [ ] `omnizip-codecs::checksum::adler32` lands.
- [ ] All CRC-32 users (LZMA, bzip2, libdeflate) use the shared impl
  (or document why they keep a local one).
- [ ] All XXHash users use the shared impl.
- [ ] All codec tests pass byte-identical.

## Priority

P2 — pure DRY.
