# 26 — Newer algorithm watch (ongoing research)

- **Priority:** P3 (ongoing)
- **Depends on:** —
- **Status:** living document; updated as new algorithms appear

## Landscape snapshot (2026-07-31)

### Production-grade, worth adding (covered in tasks 20–23)

| Algorithm | Origin | Pure Rust? | Task |
|---|---|---|---|
| Snappy | Google 2011 | `snap` crate ✅ | [20](20-snappy.md) |
| libdeflate | Biggers 2016 | port needed | [21](21-libdeflate.md) |
| LZ4 HC | Collet 2011 | `lz4_flex` ✅ | [22](22-lz4-hc.md) |
| ZSTD dictionaries | Facebook 2018 | port needed | [23](23-zstd-dictionaries.md) |

### Research-grade, deferred

| Algorithm | Status | Why deferred |
|---|---|---|
| ZPAQ | P3 | GPL-3 license concern; LZMA-9 sufficient |
| GLZA | P3 | GPL-3; non-determinism risk; niche use case |
| Density v2 | P3 | Niche; LZ4 dominates the fast tier |
| LZO | P3 | Legacy; superseded by LZ4 |
| FastLZ | P3 | Legacy; superseded by LZ4 |

### Hardware / non-software (not portable pure-Rust targets)

| Algorithm | Status | Notes |
|---|---|---|
| Intel IAA | watch only | Sapphire Rapids+ hardware unit; not a software target |
| ARM SVE compression | watch only | AArch64 extension; not portable |
| AMD PMULT | watch only | Zen 4+ multiply-accumulate; not a codec |

### Rejected approaches

| Approach | Why rejected |
|---|---|
| Learned / ML-based compression | Non-deterministic (model depends on training); violates LimniFS's air-gapped build + content-addressing rules |
| C-wrapper codecs (xz2, zstd C lib) | Violates LimniFS's pure-Rust + air-gapped rules |
| Subprocess codecs (shell out) | Violates LimniFS's no-shell-out rule |

## What we'd add if it appeared tomorrow

A pure-Rust implementation of any of these would warrant a new task:

- **LZMA3** (if released): hypothetical successor to LZMA2.
- **Brotli v2** (if released): hypothetical Brotli successor.
- **CELP/ZEGI** variants: speech codecs, niche but useful for audio drops.
- **FLIF / JPEG-XL entropy core**: still-image codecs, useful if LimniFS
  adds image-specific drops.

## Review cadence

This file is reviewed quarterly. A new entry moves to its own task file
when it clears the bar: pure-Rust viable, MIT-compatible, deterministic,
fill a gap not covered by tasks 10–23.
