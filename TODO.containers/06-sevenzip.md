# 06 — 7z (read → write → solid/volumes/AES)

- **Priority:** P1 (largest single format; phased)
- **Depends on:** [01](01-archive-core.md), `omnizip-lzma` (LZMA/LZMA2), `omnizip-bzip2`, `omnizip-deflate`, `omnizip-ppmd`, [05](05-zip-encryption.md) for AES
- **Estimated effort:** 4–6 weeks across three phases
- **Crate:** `omnizip-sevenzip`

## Goal

7z header structure (signature, header encoding, property pages), packed +
unpacked streams, coder mapping onto the existing codec crates, and the
7z-specific machinery: solid blocks, multi-volume splits, AES-256 + SHA-256
header integrity.

## Phases

- **A — read:** open + list + extract everything `7z x` can produce
  (LZMA/LZMA2, BZip2, Deflate, PPMd, copy, deltas/filters/BCJ).
- **B — write:** non-solid archives, all coder methods we ship.
- **C — advanced:** solid compression, multi-volume, encrypted headers,
  header compression.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/seven_zip/reader.rb` | `reader.rs` | ~1,700 |
| `formats/seven_zip/writer.rb` | `writer.rs` | ~1,600 |
| `formats/seven_zip/*.rb` (headers, coders, volume) | `header/`, `coders.rs`, `volume.rs` | ~1,700 |

(5,020 LOC total.)

## Acceptance

- [ ] Phase A: extract every fixture under `omnizip/spec/fixtures/7z/`;
      `7z t` on Ruby-made archives we read identically
- [ ] Phase B: `7z t` clean on our archives; byte-exact extraction
- [ ] Phase C: solid archives within 5% of `7z -9` ratio on Silesia text
- [ ] Determinism per [17](17-determinism-normalization.md) (fixed header
      serialization order, no wall-clock in headers)
