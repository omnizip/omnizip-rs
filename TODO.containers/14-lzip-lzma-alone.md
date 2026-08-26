# 14 — lzip / .lzma as formats

- **Priority:** P2
- **Depends on:** `omnizip-lzma` (already ships the codecs + lzip decoder)
- **Estimated effort:** 2–4 days
- **Crate:** `omnizip-lzma` (format facade)

## Goal

Surface the already-implemented lzip (.lz) and LZMA_Alone (.lzma) framings
as `ArchiveReader`/single-file format facades so the format registry and the
CLI treat them uniformly with gzip/bzip2. Mostly wiring: the decoders exist;
add the encoder-side lzip container (magic, version byte, CRC32 footer).

## Ruby → Rust module map

| Ruby source | Rust module | Notes |
|---|---|---|
| `formats/lzip.rb` | `lzip.rs` | container facade |
| `formats/lzma_alone.rb` | `alone.rs` | facade over existing |

## Acceptance

- [x] `lzip -d` / `lzma -d` decode our outputs; we decode theirs
- [ ] Registered in the format registry with magic sniffing
