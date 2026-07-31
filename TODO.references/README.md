# References — source materials and mappings

These documents map the omnizip-rs Rust modules to their sources: the
omnizip Ruby reference, the C reference implementations, the format
specifications (RFCs, LZMA spec), and the test corpora.

## Source hierarchy

```text
1. omnizip Ruby (primary porting source)
   ↓ port Ruby → Rust (line-by-line)
2. C reference (performance tuning only)
   ↓ consult AFTER Ruby port verifies correct
3. Format specification (normative authority)
   ↓ verify wire format matches spec
4. Test corpus (conformance gate)
   ↓ run through both implementations
```

## Why Ruby is the primary source

| Criterion | omnizip Ruby | C reference |
|---|---|---|
| Readability | clean OOP, named methods | pointer arithmetic, macros |
| License | MIT (Ribose Inc.) | 0BSD (liblzma) / BSD-3 (zstd) |
| Algorithm correctness | already tested via omnizip specs | reference standard |
| Porting effort | mechanical translation | requires de-obfuscation |

The Ruby encodes the **correct algorithm** — that's the hard work.
Rust adds production speed; it does not need to re-derive the
algorithm from C.

## Reference repos

| Repo | URL | License | Use |
|---|---|---|---|
| omnizip Ruby | `github.com/omnizip/omnizip` | MIT (Ribose) | Primary porting source |
| tukaani-project/xz | `github.com/tukaani-project/xz` | 0BSD (liblzma) | LZMA perf tuning + C fixtures |
| facebook/zstd | `github.com/facebook/zstd` | BSD-3-Clause | ZSTD perf tuning + C fixtures |
| brotli | `github.com/dropbox/rust-brotli` | BSD-3/MIT | Already pure Rust (used directly) |
| miniz_oxide | `github.com/Frommi/miniz_oxide` | MIT | Already pure Rust (used directly) |
| snap | `github.com/BurntSushi/rust-snappy` | BSD-3 | Already pure Rust (used directly) |
| lz4_flex | `github.com/pseitz/lz4_flex` | MIT | Already pure Rust (used directly) |

## Format specifications

| Spec | Location | Applies to |
|---|---|---|
| LZMA specification | LZMA SDK `lzma-specification.txt` (Igor Pavlov) | LZMA1 raw format |
| XZ format | `tukaani.org/xz/format/` | XZ container |
| RFC 8878 | `datatracker.ietf.org/doc/html/8878` | Zstandard |
| RFC 1951 | `datatracker.ietf.org/doc/html/rfc1951` | DEFLATE |
| RFC 1950 | `datatracker.ietf.org/doc/html/rfc1950` | zlib container |
| Brotli RFC 7932 | `datatracker.ietf.org/doc/html/rfc7932` | Brotli |
| Snappy format | `github.com/google/snappy/blob/main/format_description.txt` | Snappy |
| bzip2 format | `bzip.org/1.0.6/manual.txt` | bzip2 |

## Per-codec source maps

| # | File | Codec |
|---|---|---|
| 01 | [01-lzma-source-map.md](01-lzma-source-map.md) | LZMA — every Ruby file → Rust module |
| 02 | [02-zstd-source-map.md](02-zstd-source-map.md) | ZSTD — every Ruby file → Rust module |
| 03 | [03-test-corpus.md](03-test-corpus.md) | Fixtures: sources, licenses, checksums |
