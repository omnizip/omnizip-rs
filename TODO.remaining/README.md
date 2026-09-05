# Remaining Tasks

MECE decomposition of all remaining work. Each task is self-contained.

## Status

| # | Task | Status | Priority |
|---|---|---|---|
| 01 | LZMA broad-corpus sweep | done 2026-08-29 | HIGH |
| 02 | Remaining codec broad-corpus sweep (bzip2/deflate/lz4/snappy) | done 2026-08-29 | HIGH |
| 03 | Brotli q9 code-text gap diagnosis (rustsrc 1.071x) | done 2026-08-30 | MEDIUM |
| 04 | Brotli q11 binary literal-tree gap (~1.08x) | done 2026-08-30 | MEDIUM |
| 05 | Container format validation with latest codecs | done 2026-08-29 | MEDIUM |
| 06 | Downstream LimniFS re-validation at 0.21.23 | done 2026-08-29 | MEDIUM |
| 07 | CONTEXT.md domain glossary creation | done 2026-08-29 | LOW |
| 08 | Architecture: emission module extraction (from_spec_encoder.rs 8855 lines) | done 2026-08-30 | LOW |
| 09 | Zstd L1/L2 fast-tier residual cells (documented, not blocking) | deferred | LOW |
| 10 | Zstd L18-22 periodic-CSV cells (1.15-1.19x) | closed 2026-09-04 — stale-generator artifact | LOW |
| 11 | Deflate level tiers (zlib configuration_table parity) | done 2026-08-29 | HIGH |
| 12 | LZ4 fast-tier ratio on low-redundancy data | done 2026-08-29 | HIGH |
| 13 | Encode speed — fresh measurement + deterministic MT | closed 2026-09-04 | HIGH |
| 14 | RAR4 archive format — verification + closure (already shipped; stale-index fix) | done 2026-09-04 | MEDIUM |
| 15 | Real-world content-class conformance pass | done 2026-09-04 | MEDIUM |
| 16 | LZMA residual cells — re-measure + close | done 2026-09-04 | MEDIUM |
| 17 | Fuzzing depth (TODO.omnizip-rs 31) | done 2026-09-04 | MEDIUM |
| 18 | q11 dict candidates via dict-aware DP | done 2026-09-05 — env-gated OFF (measured net ~0.01%) | LOW |
| 23 | Deflate64 interop probe | done 2026-09-05 — NOT interoperable (7zz oracle) | HIGH |
| 24 | Deflate64 wire-true port | done 2026-09-05 — bidirectional 7zz interop | MEDIUM |
| 25 | zstd fast-tier real-corpus cells (task 09 re-open) | FIXED 2026-09-05 — cparams size tiering ported | LOW |
| 19 | zstd multi-threaded compress (`compress_mt`) | done 2026-09-04 | HIGH |
| 20 | brotli q4-9 bank-tier multi-threading | closed — infeasible byte-identical | MEDIUM |
| 21 | brotli q9-text single-thread gap (28-56x) | mitigated 2026-09-04 | MEDIUM |
| 22 | Extend fuzz coverage to the remaining decoders | done 2026-09-04 | MEDIUM |

## Principles

- **MECE**: each task addresses a distinct gap; together they cover all known remaining work
- **SSOT**: these files are the single source of truth for remaining work; memory files are for process/lessons
- **OCP**: new tasks are added as new files, not by modifying existing ones
- **Deletion test**: done-task files are kept as the evidence record; the README table is the status index
