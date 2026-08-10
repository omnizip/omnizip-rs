# TODO.complete — Master Plan

All remaining work to reach "fully ported" on LZMA + ZSTD. Files are
numbered by dependency order. Mark progress in each file's header.

## Reading order

- [00-overview.md](00-overview.md) — scope, prioritization, invariants
- [01-zstd-compressed-literals-decode.md](01-zstd-compressed-literals-decode.md) — unblock BUG 3 from BUGREPORT-zstd-0.1.0.md
- [02-zstd-fse-table-reader.md](02-zstd-fse-table-reader.md) — MODE_FSE for sequence tables
- [03-zstd-xxhash32-checksum.md](03-zstd-xxhash32-checksum.md) — verify frame checksums
- [04-lzma-lzip-multi-member.md](04-lzma-lzip-multi-member.md) — 7/8 failing .lz fixtures
- [10-lzma-range-encoder.md](10-lzma-range-encoder.md) — range coder (foundation)
- [11-lzma-literal-length-distance-encoders.md](11-lzma-literal-length-distance-encoders.md) — symbol encoders
- [12-lzma-match-finder.md](12-lzma-match-finder.md) — hash-chain match finder
- [13-lzma1-encoder.md](13-lzma1-encoder.md) — LZMA1 packet encoder
- [14-lzma2-encoder.md](14-lzma2-encoder.md) — LZMA2 chunk encoder
- [15-xz-container-encoder.md](15-xz-container-encoder.md) — XZ stream/block/footer
- [16-lzma-encoder-dispatch.md](16-lzma-encoder-dispatch.md) — wire encoder into Codec
- [20-zstd-huffman-encoder.md](20-zstd-huffman-encoder.md) — Huffman literal encoder
- [21-zstd-literals-encoder.md](21-zstd-literals-encoder.md) — literals section writer
- [22-zstd-fse-encoder.md](22-zstd-fse-encoder.md) — FSE encoder for sequences
- [23-zstd-sequences-encoder.md](23-zstd-sequences-encoder.md) — sequences section writer
- [24-zstd-frame-encoder.md](24-zstd-frame-encoder.md) — frame/block writer
- [25-zstd-encoder-dispatch.md](25-zstd-encoder-dispatch.md) — wire encoder into Codec
- [30-determinism-tests.md](30-determinism-tests.md) — encode-N-times byte-identical
- [31-encoder-differential-parity.md](31-encoder-differential-parity.md) — encode via both, decode via reference
- [32-yank-0.1.0-and-publish-0.1.1.md](32-yank-0.1.0-and-publish-0.1.1.md) — release process

## Invariants

Every encoder PR must satisfy:

1. **Determinism.** Same input + level → byte-identical output across
   runs, machines, and Rust versions. No `HashSet` iteration in encode
   paths, no time-seeded RNGs.
2. **Round-trip correctness.** `decompress(encode(x)) == x` for every
   input. Differential parity against `xz -d` / `zstd -d` oracle.
3. **No `unsafe`.** `#![forbid(unsafe_code)]` is workspace-wide.
4. **OCP / DRY / MECE.** Adding a new mode = new variant + dispatch
   arm, not edits to existing match arms. No copy-paste between
   encoder and decoder.
5. **Spec-first.** Wire-format changes update `docs/` before code.

## Code style (Rust equivalents of the user's Ruby rules)

- No reflection-like hacks: no `std::any::Any` for type-erased
  dispatch, no `transmute`, no `mem::forget` of owned data.
- No type-erasure via trait objects where generics work.
- All imports via `use` from crate root; no path-based imports.
- Field access via methods, not direct struct field writes from
  outside the module (encapsulate state).

---

# 2026-08-03 update — RESEARCH-driven backlog (items 80+)

A new wave of TODOs (80–89) was added based on `RESEARCH.md` — a
review of recent (2024–2026) academic compression literature. The
historical items (00–76) above document the path that got us here;
the new items document the path forward.

## Status legend (for items 80+)

- ⏳ **Pending** — not started
- 🔄 **In progress** — actively being worked
- ✅ **Done** — landed and tested
- 🚫 **Rejected** — research determined not worth pursuing

## High priority

| # | Title | Status |
|---|-------|--------|
| 81 | [ZSTD dictionary trainer (FastCover)](81-zstd-dict-trainer.md) | ✅ |
| 82 | [SIMD CRC-32 / XXHash-64](82-simd-crc32-xxhash.md) | ✅ |
| 86 | [Benchmark suite (Silesia/Enwik8/Calgary)](86-benchmark-suite.md) | ✅ |
| 87 | [Differential parity harness vs C/Ruby refs](87-differential-harness.md) | ✅ |
| 90 | [Add AIT 2026 corpus to benchmark](90-ait-2026-corpus.md) | ✅ (synthetic stand-in) |
| 91 | [Add LLM-generated text corpus to benchmark](91-llm-generated-corpus.md) | ✅ |

## Medium priority

| # | Title | Status |
|---|-------|--------|
| 80 | [ZPAQ: more context-mixing sub-models](80-zpaq-more-models.md) | ✅ (7-model Best portfolio incl. word-level; Fast/Default/Best selection via level) |
| 83 | [SIMD Huffman decode](83-simd-huffman-decode.md) | 🚫 Blocked → see TODO 102 for unblock via `wide` crate |
| 84 | [Multi-byte FSE decoder](84-multibyte-fse.md) | 🚫 Blocked → see TODO 103 for decoupled implementation |
| 88 | [Architecture audit (OCP/MECE/DRY)](88-architecture-audit.md) | ✅ |
| 89 | [Spec coverage analysis](89-spec-coverage.md) | ✅ |
| 94 | [DRY CRC-32 migration to shared checksum module](94-dry-crc32-migration.md) | ✅ |
| 96 | [Shared XXHash-64 in omnizip-codecs](96-shared-xxhash.md) | ✅ |

## Low priority

| # | Title | Status |
|---|-------|--------|
| 85 | [Document convergent-encryption boundary](85-convergent-encryption-note.md) | ✅ |
| 92 | [Track DCC 2026 proceedings quarterly](92-dcc-2026-tracking.md) | process |
| 93 | [Track ZipServ GPU insights for future SIMD work](93-zipserv-insights.md) | process |
| 95 | [Wire FSST/Rice++/FLAC/BLOSC/GLZA/Deflate64 into omnizip-bench](95-more-bench-codecs.md) | ✅ |
| 98 | [LPC subframe interop bug: LOST_SYNC on high-order LPC](98-lpc-interop-bug.md) | ✅ |
| 99 | [Differential harness: fix bzip2/lz4/DEFLATE framing gaps](99-framing-gaps.md) | ✅ |
| 100 | [Code review sweep: OCP/MECE/DRY improvements](100-code-review-sweep.md) | ✅ |
| 101 | [ZSTD encoder: per-call hash-table allocation](101-zstd-encoder-reuse-hash-table.md) | ✅ (hash_log cap + `ZstdCompressor` reuse) |
| 102 | [SIMD Huffman via `wide` crate](102-simd-huffman-wide.md) | ✅ (Phase 1 + Phase 2 `simd-huffman` feature landed) |
| 103 | [Multi-byte FSE — decoupled impl](103-multibyte-fse-unblocked.md) | ✅ (existing 2-state interleave addresses the goal) |
| 104 | [Libdeflate pure-Rust codec](104-libdeflate-codec.md) | ✅ (Phase 1+2+3 all landed; in-house encoder+decoder) |
| 105 | [FLAC LPC verification corpus](105-flac-lpc-finish.md) | 🔄 (omnizip-rs harness landed; LimniFS provides corpus) |
| 106 | [LZMA optimal parser: exact prices](106-lzma-optimal-parser-exact-prices.md) | ✅ (Phase 1-3 all landed) |
| 107 | [ZSTD BT match finder for levels 16-22](107-zstd-bt-match-finder.md) | ⏳ (P0 — 5-15% ratio gap) |
| 108 | [LZMA BT4 match finder](108-lzma-bt4-match-finder.md) | ⏳ (P1 — 3-8% ratio gap) |
| 109 | [BZip2 SA-IS suffix array](109-bzip2-sais-bwt.md) | ⏳ (P2 — 2-5x speed) |
| 110 | [ZSTD encoder perf cliff (infinite loop)](110-zstd-perf-cliff.md) | ✅ (root cause: backward-extension `ip` mutation) |
| 111 | [FLAC block-size sweep pruning](111-flac-block-size-pruning.md) | ✅ (heuristic `pick_block_size` landed; 11× speedup) |
| 112 | [FLAC FFT-based autocorrelation](112-flac-fft-autocorrelation.md) | ✅ (radix-2 FFT + Wiener-Khinchin landed behind `fft-acf` feature) |
| 113 | [ricepp SIMD via wide](113-ricepp-simd-via-wide.md) | ✅ (`delta_zigzag_sum` SIMD path landed) |
| 114 | [Shared match-finder (DRY)](114-shared-match-finder.md) | ✅ (`HashChainMatchFinder` in omnizip-codecs; migration TODOs 122-125) |
| 115 | [Shared bitstream (DRY)](115-shared-bitstream.md) | ⏳ (P2 — DRY) |
| 116 | [DEFLATE dynamic-Huffman](116-deflate-dynamic-huffman.md) | ✅ (correct standard Huffman + zlib CPI length-limiting in #116) |
| 117 | [Brotli full pure-Rust port](117-brotli-full-port.md) | 🔄 (Phase D landed in #119: pure-Rust uncompressed encoder+decoder, brotli crate removed) |
| 118 | [PPMd context-tree init profiling](118-ppmd-context-tree-init.md) | ✅ (`PpmdCompressor` landed in #87) |
| 119 | [Codec streaming API](119-codec-streaming-api.md) | ⏳ (P2 — LimniFS scaling) |
| 120 | [Continuous differential harness](120-continuous-differential-harness.md) | ⏳ (P1 — regression safety) |
| 121 | [ZSTD L12+ repetition regression](121-zstd-l12-repetition-regression.md) | ✅ (length-cap alignment fix in #90) |
| 122 | [LZMA → shared match finder](122-lzma-use-shared-match-finder.md) | ⏳ (P2 — DRY) |
| 123 | [LZ4 HC → shared match finder](123-lz4-hc-use-shared-match-finder.md) | ⏳ (P2 — DRY) |
| 124 | [libdeflate → shared match finder](124-libdeflate-use-shared-match-finder.md) | ⏳ (P2 — DRY) |
| 125 | [ZSTD → shared match finder](125-zstd-use-shared-match-finder.md) | ⏳ (P1 — biggest DRY) |
| 126 | [Differential fuzz testing](126-differential-fuzz-testing.md) | ⏳ (P1 — regression safety) |
| 127 | [Codec parallel batch](127-codec-parallel-batch.md) | ⏳ (P2 — throughput) |
| 128 | [Per-codec memory budgets](128-per-codec-memory-budgets.md) | ⏳ (P2 — embedded) |
| 129 | [Shared checksum finish](129-shared-checksum-finish.md) | ⏳ (P2 — DRY) |
| 130 | [Brotli Phase B — Huffman + block-type](130-brotli-phase-b-huffman-blocktype.md) | ✅ (Phase B + B-cont landed in #92, #94) |
| 131 | [Snappy pure-Rust port](131-snappy-pure-rust-port.md) | ✅ (pure-Rust `codec::encode`/`decode` in #98; `snap` is dev-dep only) |
| 132 | [LZ4 pure-Rust port](132-lz4-pure-rust-port.md) | ✅ (in-house `block`+`frame` modules; `lz4_flex` removed) |
| 133 | [DEFLATE pure-Rust port](133-deflate-pure-rust-port.md) | ✅ (delegates to `omnizip_libdeflate`; `miniz_oxide` removed) |
| 134 | [BLOSC pure-Rust port](134-blosc-pure-rust-port.md) | ✅ (uses in-house `omnizip_lz4::block`; no `lz4_flex`) |
| 135 | [Filters — drop lz4_flex](135-filters-no-external-lz4.md) | ✅ (uses in-house `omnizip_lz4::block`) |
| 136 | [libdeflate — drop miniz_oxide fallback](136-libdeflate-no-miniz-fallback.md) | ✅ (pure-Rust dynamic-Huffman landed) |
| 137 | [Async Codec trait](137-async-codec-trait.md) | ⏳ (P2) |
| 138 | [Codec observability](138-codec-observability.md) | ⏳ (P2) |
| 139 | [Per-codec proptests](139-per-codec-proptests.md) | ⏳ (P1) |
| 140 | [Spec compliance matrix](140-spec-compliance-matrix.md) | ⏳ (P2) |
| 141 | [GHA differential workflow](141-gha-differential-workflow.md) | ⏳ (P1) |
| 142 | [Benchmark regression detection](142-benchmark-regression-detection.md) | ⏳ (P2) |
| 143 | [Error type unification](143-error-type-unification.md) | ⏳ (P2) |
| 144 | [Wire-format conformance corpus](144-wire-format-corpus.md) | ⏳ (P2) |
| 145 | [Hash function quality audit](145-hash-function-audit.md) | ⏳ (P2) |
| 146 | [Reusable-state pattern sweep](146-reusable-compressor-pattern.md) | ⏳ (P1) |
| 147 | [Determinism cross-platform audit](147-determinism-cross-platform-audit.md) | ⏳ (P1) |
| 148 | [Code review sweep OCP/MECE/DRY](148-code-review-sweep-ocp-mece-dry.md) | ⏳ (P2) |
| 149 | [Per-codec READMEs](149-per-codec-readmes.md) | ⏳ (P2) |
| 150 | [Multi-platform CI matrix](150-multi-platform-ci.md) | ⏳ (P1) |
| 151 | [Brotli Phase C — encoder + dictionary](151-brotli-phase-c-encoder.md) | ⏳ (P0 — TODO 117 continuation) |
| 152 | [ZSTD per-se SIMD](152-zstd-per-se-simd.md) | ⏳ (P1 — LimniFS #11) |
| 153 | [BCJ filter coverage ARM/ARM64/IA64/SPARC/PPC](153-bcj-filter-coverage.md) | ⏳ (P2) |
| 154 | [Deflate64 from-spec finish](154-deflate64-from-spec.md) | ⏳ (P2) |
| 155 | [Security audit + cargo-audit CI](155-security-audit-ci.md) | ⏳ (P1) |
| 156 | [Coverage measurement](156-coverage-measurement.md) | ⏳ (P2) |
| 157 | [Architecture guide + ADRs](157-architecture-guide-adrs.md) | ⏳ (P2) |
| 158 | [FSST v2 + GLZA tuning](158-fsst-v2-glza-tuning.md) | ⏳ (P2) |
| 159 | [Filter-Codec composition](159-filter-codec-composition.md) | ✅ (`FilteredCodec<C, F>` adapter landed in omnizip-filters) |
| 160 | [Release automation](160-release-automation.md) | ⏳ (P2) |
| 161 | [Deflate64 encoder](161-deflate64-encoder.md) | ⏳ (P2 — LimniFS-flagged) |
| 162 | [FSST preprocessor wiring](162-fsst-preprocessor-wiring.md) | ⏳ (P2) |
| 163 | [GLZA O(N²) cap → linear-time](163-glza-linear-time.md) | ⏳ (P2) |
| 164 | [Snappy encoder](164-snappy-encoder.md) | ✅ (pure-Rust `codec::encode` landed in #98) |
| 165 | [LZMA real reusable state](165-lzma-real-reusable-state.md) | ✅ (`LzmaCompressor` + `MatchFinder::reuse` landed) |
| 166 | [FLAC remainder — finish 10× gap](166-flac-remainder.md) | ⏳ (P2) |
| 167 | [ricepp remainder — unary emission](167-ricepp-remainder.md) | ⏳ (P2) |
| 168 | [Brotli Huffman-coded encoder — static tree path](168-brotli-huffman-static-tree.md) | 🔄 (foundation landed: static_codes + commands + huffman modules; wire-format bugs being traced) |
| 169 | [Brotli Huffman wire-format debugging](169-brotli-huffman-wire-format-debug.md) | ⏳ (P0 — TODO 168 continuation; isolates the remaining bit-level bugs) |

## 2026-08-10 update — Post-CSV-ratio-closure wave (items 244+)

After closing the synthetic-test CSV ratio gap (we now beat vendored C
on CSV 100KB and 500KB), this wave captures the remaining architectural,
correctness, and feature work needed to call the workspace "done".

### Status legend (for items 244+)

- ⏳ **Pending** — not started
- 🔄 **In progress** — actively being worked
- ✅ **Done** — landed and tested
- 🚫 **Superseded** — newer approach replaced this

### Highest priority (P0/P1)

| # | Title | Status |
|---|-------|--------|
| 244 | [Brotli decoder wire-format bugs](244-brotli-decoder-wire-format-bugs.md) | ⏳ (P0 — unblocks 228, 229, 232, 242) |
| 247 | [Real-world test corpora](247-real-world-test-corpora.md) | 🔄 (P1 — setup.sh landed; LimniFS csv-synthetic pending) |
| 250 | [Property-based tests for all encoders](250-property-tests-encoders.md) | 🔄 (P1 — scaffold landed; full proptest migration pending) |
| 252 | [Encoder regression benchmark suite](252-encoder-regression-benchmark-suite.md) | ✅ (regression.rs + baseline.json landed in 0.16.24) |
| 253 | [Wire-format differential fuzzer](253-wire-format-differential-fuzzer.md) | 🔄 (P1 — 6 cargo-fuzz targets landed; cross-decoder pending) |
| 257 | [LZMA BT4 match finder](257-lzma-bt4-match-finder.md) | ⏳ (P1 — 5-15% LZMA ratio gap) |

### Architectural quality (P2/P3)

| # | Title | Status |
|---|-------|--------|
| 245 | [Brotli rep codes 1/2/3 via explicit distance codes](245-brotli-rep-codes-1-2-3-explicit.md) | ✅ (RepBuffer landed in 0.16.24; 3-7 pp CSV win) |
| 246 | [Iterative optimal parser refinement](246-iterative-optimal-parser-refinement.md) | ✅ (2-pass at Q8+ landed in 0.16.24) |
| 248 | [Codec profile enum](248-codec-profile-enum.md) | ✅ (Profile + ProfileKind landed in 0.16.23) |
| 249 | [Shared Huffman module unification](249-shared-huffman-module.md) | ⏳ (P2 — DRY ~2K LOC) |
| 251 | [Codec streaming API](251-codec-streaming-api.md) | 🔄 (P2 — shared trait exists; LZ4 impl landed in 0.16.25) |
| 254 | [Architecture decision records](254-architecture-decision-records.md) | ✅ (10 ADRs landed in 0.16.23) |
| 255 | [Code review sweep OCP/MECE/DRY](255-code-review-sweep-ocp-mece-dry.md) | ✅ (audit doc + 3 quick refactors landed in 0.16.24) |
| 256 | [Encoder profile auto-detection](256-encoder-profile-auto-detection.md) | ✅ (ContentType::detect() landed in 0.16.23) |
| 258 | [Shared bitstream module](258-shared-bitstream-module.md) | 🔄 (P3 — shared BitReaderBE/LE exists; per-codec migration pending) |
| 259 | [Error type unification](259-error-type-unification.md) | 🔄 (P3 — helpers + per-codec sub-errors landed in 0.16.25) |
| 260 | [Codec parallel batch API](260-codec-parallel-batch-api.md) | ✅ (ParallelBatch trait landed in 0.16.23) |
| 261 | [Codec capability metadata](261-codec-capability-metadata.md) | ✅ (Capabilities struct + per-codec overrides landed in 0.16.26) |
| 262 | [Unified codec options builder](262-unified-codec-options-builder.md) | ✅ (Options builder + compress_with_options landed in 0.16.26) |
| 263 | [Brotli cross-decoder wire-format fix](263-brotli-cross-decoder-fix.md) | ⏳ (P0 — vendored C rejects our output) |
| 264 | [Per-codec memory budget API](264-per-codec-memory-budget.md) | ✅ (MemoryBudget trait + per-codec overrides for Brotli/ZSTD/LZMA/LZ4 landed in 0.16.27) |
| 265 | [Workspace bench CLI](265-workspace-bench-cli.md) | 🔄 (omnizip-bench CLI exists with --codec/--corpus/--format; --diff subcommand landed in 0.16.26) |
| 266 | [Multi-codec ensemble auto-selection](266-multi-codec-ensemble-autoselect.md) | ✅ (Ensemble + Goal + heuristic picker landed in 0.16.27) |
| 267 | [WebAssembly build target](267-wasm-build-target.md) | 🔄 (P3 — wasm.yml workflow landed in 0.16.28; per-codec fixes pending) |
| 268 | [Compression ratio predictor](268-compression-ratio-predictor.md) | ⏳ (P3 — UX) |
| 269 | [Per-codec README files](269-per-codec-readmes.md) | 🔄 (P3 — brotli README updated; others pending) |
| 270 | [Profile-Guided Optimization (PGO)](270-profile-guided-optimization.md) | 🔄 (P3 — scripts/build-pgo.sh landed in 0.16.28) |
| 271 | [Codec conformance test suite](271-codec-conformance-suite.md) | 🔄 (P1 — tests/conformance/ scaffold landed in 0.16.28; corpora pending) |
| 272 | [Brotli Q11 tuning (4-iteration parser)](272-brotli-q11-tuning.md) | 🔄 (P2 — Q11 now uses 4 iterations landed in 0.16.28) |
| 273 | [LimniFS workload integration tests](273-limnifs-workload-integration-tests.md) | ⏳ (P1 — real-world validation) |
| 274 | [Brotli static dictionary decode audit](274-brotli-static-dict-decode-audit.md) | ⏳ (P1 — decoder rejects vendored output) |
| 275 | [Wire-format property tests with reference decoders](275-wire-format-property-tests.md) | ⏳ (P1 — cross-decoder proptest) |
| 276 | [Codec determinism audit across platforms](276-cross-platform-determinism-audit.md) | ⏳ (P1 — LimniFS requirement) |

## How to claim a TODO

1. Pick an item from the tables above.
2. Move its entry to 🔄 status.
3. Create a feature branch `feat/{slug}`.
4. Implement per the acceptance criteria in the TODO file.
5. Open a PR; merge to main; mark ✅ here.

