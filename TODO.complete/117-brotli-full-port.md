# TODO 117: Brotli full pure-Rust implementation

## Problem

`omnizip-brotli` currently wraps the upstream `brotli` crate (~15 K
LOC). The user has indicated this should be a full pure-Rust port from
the spec (RFC 7932), matching the workspace convention that every
codec is implemented from the spec rather than wrapped.

Brotli is the most complex compression format in the workspace. The
existing crates (LZMA, ZSTD, bzip2) each took 3-6 weeks of focused
work. Brotli is comparable in scope.

## Scope

Brotli components (per RFC 7932):

1. **LZ77 layer**: sliding window up to 16 MiB, with 16-bit distance
   coding, 7-bit length coding, custom distance short codes.
2. **Static dictionary**: 1226 entries (12 KiB uncompressed) with 121
   transforms (suffix, prefix, case-folding, etc.). Total effective
   dictionary ~1.5 MiB.
3. **Context modeling**: 6 context modes (LITERAL, CONTEXT_LSB6,
   CONTEXT_MSB6, CONTEXT_UTF8, CONTEXT_SIGNED, CONTEXT_LSB6_CONT).
4. **Huffman coding**: with predefined alphabets for lengths and
   distances, custom context-mode-aware tables.
5. **Block types**: 7 block types (UNCOMPRESSED, METABLOCK_HEADER,
   etc.), metablock chains, ISLAST / ISLASTEMPTY semantics.
6. **Stream header**: 4-bit WBITS variant.
7. **Encoder strategies**: 0-11 quality levels, each with different
   match-finder + parser strategies.

## Phased plan

### Phase A — decoder only (3 weeks)

Mirror the LZMA / ZSTD porting workflow: decode first, encode later.

Files:
- `omnizip-brotli/src/{state, prefix, huffman, transform, dictionary, metablock, decode}.rs`

Acceptance:
- Decode reference `.br` fixtures bit-exactly.
- Decoder throughput ≥ 100 MB/s on text.

### Phase B — fixed-Huffman encoder (1 week)

Mirror `omnizip-libdeflate` Phase 1: stored + fixed-context blocks.

Files:
- `omnizip-brotli/src/encoder/{stored, fixed_context, simple}.rs`

Acceptance:
- Round-trips through own decoder.
- Ratio ≥ 50% of `brotli -q 11` on text fixtures.

### Phase C — full encoder (4 weeks)

Match reference `brotli -q 11` ratio within 5% on Silesia/Enwik8.

Files:
- `omnizip-brotli/src/encoder/{dictionary_lookup, context_modes, metablock_optimiser, quality_levels}.rs`

Acceptance:
- Differential parity vs `brotli -d` on all reference fixtures.
- Quality 0-11 selectable via `CompressionLevel`.

## Acceptance criteria (overall)

- [ ] No external `brotli` dependency in `omnizip-brotli/Cargo.toml`.
- [ ] Decoder: all RFC 7932 fixtures round-trip.
- [ ] Encoder: round-trip via own decoder and via `brotli -d`.
- [ ] Encoder quality 11 within 5% of `brotli -q 11` on Enwik8.
- [ ] Encoder throughput ≥ 5 MB/s at quality 6.

## Priority

P0 — workspace convention: every codec is pure-Rust from spec. The
wrapper dependency is the last remaining exception.

## Notes

This is the largest single TODO in the workspace. Should be split
across multiple PRs (Phase A, B, C) with differential gates between
each phase.
