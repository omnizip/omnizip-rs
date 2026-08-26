# 10 — XAR (macOS .pkg)

- **Priority:** P2
- **Depends on:** [01](01-archive-core.md), `omnizip-deflate`, `omnizip-bzip2`, `omnizip-lzma`
- **Estimated effort:** 2 weeks
- **Crate:** `omnizip-xar`

## The XML decision

XAR's table of contents is GZIP-compressed XML. Options:
(a) hand-rolled minimal XML writer + lenient reader for OUR TOC subset only,
(b) `quick-xml` (pure Rust, safe, widely audited).

**Recommendation:** (b) for reading (TOCs in the wild are arbitrary XML),
(a)-style constrained writer for output (we control the schema; deterministic
attribute order is trivially guaranteed). Same gating rationale as the
crypto decision in [05](05-zip-encryption.md).

## Goal

XAR read/write: header (magic, checksum), TOC parse/generate, per-file
compression (gzip/bzip2/lzma/xz/none), checksum algorithms (MD5/SHA1/SHA256…),
xattrs, hardlinks, symlinks, device nodes, FIFOs.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/xar/reader.rb`, `writer.rb`, toc, heap | `reader.rs`, `writer.rs`, `toc.rs`, `heap.rs` | 2,038 |

## Acceptance

- [ ] libarchive compatibility corpus green (the Ruby suite's bar)
- [x] `xar --toc` reads our archives; pkg files from macOS extract correctly
- [x] Deterministic TOC (canonical element order, quoted attributes)
