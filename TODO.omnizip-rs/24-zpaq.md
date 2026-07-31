# 24 — ZPAQ (research tier)

- **Priority:** P3 (extreme-ratio archival)
- **Depends on:** [01](01-codec-trait-registry.md)
- **Estimated effort:** 4–6 weeks (complex)
- **Crate:** `omnizip-zpaq` (future)

## Why

ZPAQ (Matt Mahoney 2009) is a context-mixing compressor that achieves the
best publicly-available ratio on text and structured data — often 10–20%
better than LZMA at level 9. It's the ratio ceiling for lossless
compression.

For LimniFS's "deep archival" tier (cold storage, accessed rarely), ZPAQ
could be the best-ratio codec. The cost is encode speed: ZPAQ level 5 is
~100 KB/s — orders of magnitude slower than LZMA.

## Approach

The ZPAQ format is fully documented (the `zpaq` spec + Mahoney's paper
"PAQ8" series). Port from the C++ reference at `zpaq/zpaq` (GPL-3 → check
compatibility) or from Mahoney's public-domain reference.

**License concern:** the reference ZPAQ is GPL-3. The CAMPAIGN.md rule
"No GPL-3 anywhere" applies. We must either:
1. Port from Mahoney's public-domain reference (the spec is public domain),
2. Get a GPL-3 → MIT/Apache license exception from the author, or
3. Skip ZPAQ and accept LZMA-9 as the ratio ceiling.

This task is **deferred** until the license question is resolved.

## Phase scope (if license resolves)

1. **Decoder** (2 weeks): port the ZPAQ decoder. The format is a journaling
   archive with mixed-model context prediction.
2. **Encoder** (2 weeks): port the encoder with one model (the simplest
   `fast` model).
3. **More models** (ongoing): port `mid`, `max`, `bmp`, `jpeg`, etc.

## Acceptance

- Decode + encode round-trip on text fixtures.
- Ratio within 5% of reference `zpaq -add ... -method 5` on Silesia.
- Document the license decision in the crate README.

## Open question

Is ZPAQ worth the complexity? LZMA-9 + Brotli-11 cover most archival
needs. ZPAQ's marginal ratio gain (10–20% on text) may not justify the
maintenance cost. **Defer until LimniFS has users asking for it.**
