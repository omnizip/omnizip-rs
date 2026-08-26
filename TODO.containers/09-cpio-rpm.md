# 09 — CPIO + RPM

- **Priority:** P1
- **Depends on:** [01](01-archive-core.md), `omnizip-deflate`, `omnizip-bzip2`, `omnizip-lzma`, `omnizip-zstd`
- **Estimated effort:** 2 weeks
- **Crate:** `omnizip-cpio`, `omnizip-rpm`

## Goal

CPIO read/write (newc + CRC formats) — including its role as the RPM payload
container. RPM read/write: lead + signature/header structures (tags, regions),
metadata extraction, payload compression selection (gzip/bzip2/xz/zstd),
payload decompression on read.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/cpio/reader.rb`, `writer.rb` | `omnizip-cpio` | 936 |
| `formats/rpm/reader.rb`, `writer.rb`, tags | `omnizip-rpm` | 1,282 |

## Acceptance

- [x] cpio round-trip; `cpio -it` reads our archives
- [x] `rpm2cpio | cpio -id` path verified against our RPM reader
- [x] RPMs we write install with `rpm -i` (signature/digest consistency)
- [ ] Deterministic RPMs (fixed build-time, sorted file list) — the
      reproducible-builds use case is a first-class consumer here
