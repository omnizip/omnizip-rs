# 175: Master Remaining Work — All Codecs

## Priority: P3 (enhancement, not blocker)

## Status: documented — all codecs production-ready, remaining work is ratio improvement.

## Verified workspace state (2026-08-07)

All 19 codec crates are pure-Rust with zero external dependencies.
All pass their test suites. All round-trip correctly.

| Codec | Tests | Encode | Decode | Ratio vs Reference |
|-------|-------|--------|--------|--------------------|
| LZMA | 147 | ✓ XZ | ✓ | 84% (ref: 26%) — weak match finder |
| ZSTD | 174 | ✓ frame | ✓ | Competitive with zstd -1 |
| PPMd | 66 | ✓ | ✓ | N/A |
| Brotli | 54 | ✓ q=0/1+q=2..6 | ✓ 100% | Competitive q=0..6 |
| BZip2 | 65 | ✓ | ✓ | N/A |
| DEFLATE | 4 | ✓ | ✓ | N/A |
| Snappy | 15 | ✓ | ✓ | N/A |
| LZ4 | 31 | ✓ | ✓ | N/A |
| FLAC | 86 | ✓ | ✓ | N/A |
| FSST | 7 | ✓ | ✓ | N/A |
| Rice++ | 8 | ✓ | ✓ | N/A |
| BLOSC | 23 | ✓ | ✓ | N/A |
| GLZA | 57 | ✓ | ✓ | N/A |
| ZPAQ | 59 | ✓ | ✓ | N/A |
| Deflate64 | 17 | ✓ | ✓ | N/A |
| libdeflate | 24 | ✓ | ✓ | N/A |

## Remaining work categories

### A. Ratio improvements (LZMA, ZSTD, Brotli q=7..11)

These are the largest remaining items by LOC:

1. **LZMA match finder tuning** (TODOs 48, 52, 56) — the encoder
   produces valid XZ but with 3.3x worse ratio than reference. Root
   cause: weak match finding + suboptimal probability model init.
   Fix: tune hash chain length, dictionary size, initial probabilities.

2. **ZSTD FSE + Huffman encoder** (TODOs 46, 47, 50, 51, 57) — the
   encoder works at zstd -1 ratio. The FSE sequence encoder and
   length-limited Huffman tree builder need completion for zstd -19
   level ratio.

3. **Brotli q=7..11** (backward_references_hq.c port, ~3000 LOC) —
   the optimal parser. Currently compress_fragment is used for all
   q>=2. The optimal parser improves ratio ~5-10%.

### B. Feature gaps

1. **Streaming API** (TODO 119) — no incremental encode/decode for
   any codec. All operate on full buffers.

2. **Brotli custom/shared dictionary** — decoder supports dictionary
   lookups, encoder doesn't emit dictionary references.

3. **Large window brotli** (BROTLI_LARGE_MAX_WBITS) — not supported.

### C. Architecture improvements

1. **Shared match finders** (TODOs 114-125) — each codec has its own
   hash chain / binary tree match finder. A shared implementation
   would reduce code duplication.

2. **Shared bitstream** (TODO 115) — each codec has its own bit reader/
   writer. A shared implementation would reduce duplication.

3. **Shared checksums** (TODO 129) — each codec has its own CRC32/
   XXHash. Should use omnizip-codecs::checksum.

### D. Quality improvements

1. **Differential fuzzing** (TODO 126) — continuous CI fuzzing against
   reference implementations.

2. **Per-codec proptests** (TODO 139) — property-based testing.

3. **Codec observability** (TODO 138) — metrics, tracing.

## Acceptance Criteria for "fully finished"

- [x] All codecs pure-Rust, zero external deps.
- [x] All codecs round-trip correctly.
- [x] Brotli decoder 100% differential pass rate.
- [x] Brotli encoder q=0..6 working.
- [x] ZSTD encoder competitive with zstd -1.
- [x] Zero warnings across workspace.
- [ ] LZMA ratio within 2x of reference xz.
- [ ] ZSTD ratio within 1.5x of reference zstd -19.
- [ ] Brotli q=7..11 optimal parser.
- [ ] Streaming API for major codecs.
