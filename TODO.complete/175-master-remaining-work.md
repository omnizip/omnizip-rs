# 175: Master Remaining Work — All Codecs

## Priority: P3 (enhancement, not blocker)

## Status: documented — all codecs production-ready, remaining work is ratio improvement.

## Verified workspace state (2026-08-07)

All 19 codec crates are pure-Rust with zero external dependencies.
All pass their test suites. All round-trip correctly.

| Codec | Tests | Encode | Decode | Ratio vs Reference |
|-------|-------|--------|--------|--------------------|
| LZMA | 144+1 | ✓ XZ | ✓ | 3-9% (competitive, see 176) |
| ZSTD | 174 | ✓ frame | ✓ | Full 22-level dispatch (see 177) |
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

Detailed in TODOs 176-178:

1. **LZMA full finish** (TODO 176) — LZMA2 multi-chunk state reuse,
   BT4 match finder at level 9, optimal-parser exact prices. Current
   ratio: 3-9% (competitive with reference xz on periodic data).

2. **ZSTD full finish** (TODO 177) — FSE sequence encoder completion,
   length-limited Huffman verification, dictionary support. Current
   ratio: full 22-level dispatch (PR #172), competitive with zstd -1
   through zstd -22 parameter selection.

3. **Brotli q=7..11** (TODO 173) — backward_references_hq.c port
   (~3000 LOC optimal parser). Currently compress_fragment for q>=2.

### B. Feature gaps

1. **Streaming API** (TODO 119) — no incremental encode/decode for
   any codec. All operate on full buffers.

2. **Brotli custom/shared dictionary** — decoder supports dictionary
   lookups, encoder doesn't emit dictionary references.

3. **Large window brotli** (BROTLI_LARGE_MAX_WBITS) — not supported.

### C. Architecture improvements

Detailed in TODOs 179-180:

1. **Shared match finders** (TODO 179, 114-125) — LZMA now uses the
   shared `HashChainMatchFinder` (PR #172). ZSTD, LZ4 HC, libdeflate
   still have their own. Full migration would save ~800 LOC.

2. **Shared bitstream** (TODO 179, 115) — each codec has its own bit
   reader/writer. A shared `BitReaderBE`/`BitReaderLE` module would
   save ~400 LOC.

3. **Shared checksums** (TODO 129) — **DONE**. LZMA and BZip2 already
   delegate to `omnizip_codecs::checksum::crc32_iso_hdlc`.

4. **Shared Huffman** (TODO 179) — ZSTD, Brotli, BZip2, DEFLATE each
   have their own. A shared module would save ~600 LOC.

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
- [x] LZMA ratio competitive (3-9% on test fixtures).
- [x] ZSTD full 22-level dispatch (PR #172).
- [x] Shared matchfinder consolidation for LZMA (PR #172).
- [x] Shared CRC32 via re-exports.
- [ ] LZMA LZMA2 multi-chunk state reuse (TODO 176).
- [ ] ZSTD ratio within 1.5x of reference zstd -19.
- [ ] Brotli q=7..11 optimal parser.
- [ ] Streaming API for major codecs.
- [ ] Shared bitstream module (TODO 179).
- [ ] Shared Huffman module (TODO 179).
