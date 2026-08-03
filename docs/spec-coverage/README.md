# Spec Coverage Analysis

This directory contains per-codec coverage matrices mapping format
specification clauses to test coverage.

## Available matrices

| Codec | Spec | Matrix |
|-------|------|--------|
| FLAC | [Xiph FLAC 1.4.0](https://xiph.org/flac/format.html) | [flac.md](flac.md) |
| DEFLATE | [RFC 1950/1951](https://datatracker.ietf.org/doc/html/rfc1951) | [deflate.md](deflate.md) |
| ZSTD | [RFC 8878](https://datatracker.ietf.org/doc/html/rfc8878) | [zstd.md](zstd.md) |

## Priority gaps

- bzip2: custom wire format, not standard `.bz2` (TODO 99)
- LZ4: raw blocks, not LZ4 frames (TODO 99)
- Multi-byte FSE decoder for ZSTD (TODO 84)
