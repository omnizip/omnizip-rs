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
| 87 | [Differential parity harness vs C/Ruby refs](87-differential-harness.md) | ⏳ |

## Medium priority

| # | Title | Status |
|---|-------|--------|
| 80 | [ZPAQ: more context-mixing sub-models](80-zpaq-more-models.md) | 🔄 (run-length landed; order-3, word pending) |
| 83 | [SIMD Huffman decode](83-simd-huffman-decode.md) | ⏳ |
| 84 | [Multi-byte FSE decoder](84-multibyte-fse.md) | ⏳ |
| 88 | [Architecture audit (OCP/MECE/DRY)](88-architecture-audit.md) | 🔄 (partial — arith + hash extracted; PpmdCore pending) |
| 89 | [Spec coverage analysis](89-spec-coverage.md) | ⏳ |
| 90 | [Add AIT 2026 corpus to benchmark](90-ait-2026-corpus.md) | ⏳ |
| 91 | [Add LLM-generated text corpus to benchmark](91-llm-generated-corpus.md) | ⏳ |

## Low priority

| # | Title | Status |
|---|-------|--------|
| 85 | [Document convergent-encryption boundary](85-convergent-encryption-note.md) | ✅ |
| 92 | [Track DCC 2026 proceedings quarterly](92-dcc-2026-tracking.md) | process |
| 93 | [Track ZipServ GPU insights for future SIMD work](93-zipserv-insights.md) | process |

## How to claim a TODO

1. Pick an item from the tables above.
2. Move its entry to 🔄 status.
3. Create a feature branch `feat/{slug}`.
4. Implement per the acceptance criteria in the TODO file.
5. Open a PR; merge to main; mark ✅ here.

