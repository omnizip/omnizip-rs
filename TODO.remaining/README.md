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
| 10 | Zstd L18-22 periodic-CSV cells (1.15-1.19x) | deferred — re-diagnosed 2026-08-30 | LOW |
| 18 | q11 collect-level dictionary candidates (activation needs dict-aware DP) | pending 2026-09-04 | LOW |

## Principles

- **MECE**: each task addresses a distinct gap; together they cover all known remaining work
- **SSOT**: these files are the single source of truth for remaining work; memory files are for process/lessons
- **OCP**: new tasks are added as new files, not by modifying existing ones
- **Deletion test**: done-task files are kept as the evidence record; the README table is the status index
