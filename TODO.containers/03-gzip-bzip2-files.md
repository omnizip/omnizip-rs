# 03 — GZIP / BZIP2 single-file formats

- **Priority:** P0 (quick wins over shipped codecs)
- **Depends on:** [01](01-archive-core.md), `omnizip-deflate`, `omnizip-bzip2`
- **Estimated effort:** 2–4 days
- **Crate:** `omnizip-archive-core` (as `Formats::Gzip` / `Formats::Bzip2File`)

## Goal

The `.gz` / `.bz2` single-file containers: header, original-name field,
mtime, CRC32/ISIZE trailer; bzip2's zero-container framing. Thin wrappers
that make the CLI's `ozip gzip` / `ozip bzip2` drop-in comparable to
`gzip(1)` / `bzip2(1)`.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/gzip.rb` | `gzip.rs` | ~110 |
| `formats/bzip2_file.rb` | `bzip2_file.rs` | ~100 |

## Acceptance

- [x] `gzip -d` / `bzip2 -d` decode our outputs byte-exactly
- [x] We decode everything `gzip`/`bzip2 -9` produce (incl. multi-member gzip)
- [ ] Trailer CRC32 + ISIZE verified on decode
