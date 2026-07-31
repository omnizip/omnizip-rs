# 23 — ZSTD dictionaries

- **Priority:** P2 (critical for small-file ratio)
- **Depends on:** [14](14-zstd-phase-b-encoder.md)
- **Estimated effort:** 2 weeks
- **Crate:** `omnizip-zstd` (extended)

## Why

ZSTD dictionaries (Facebook 2018+) pre-train an entropy model on a
representative corpus. For small files (< 100 KB), dictionary compression
gives 2–4x better ratio than dictionary-less. Critical for LimniFS when
storing many small files (e.g., a node_modules tree or a kernel source
tree).

Without dictionaries, ZSTD on a 4 KB file produces ~3 KB output. With a
trained dictionary, ~1 KB. For a content-addressed store with millions of
small drops, this is the difference between viable and not.

## Approach

1. **Dictionary format** (1 week): port the ZSTD dictionary format (magic
   number, dictionary ID, entropy tables, trained content). Decode-side
   first (the format is documented in RFC 8878 §5).
2. **Dictionary training** (1 week): implement FASTCOVER and ECov training
   algorithms. The C reference at `lib/dictCover.c` (BSD-3) is the spec;
   port to Rust.

## Acceptance

- A trained dictionary on a 1000-file corpus of 4 KB JSON files achieves
  ≥ 2x ratio improvement vs dictionary-less ZSTD-6.
- Dictionary encode + decode round-trips byte-identically.
- Dictionary ID is deterministic (same corpus ⇒ same dictionary).
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- The dictionary ID must be recorded in the codec metadata so the decoder
  knows which dictionary to use. This requires extending LimniFS's drop
  record format with a dictionary-id field — coordinate with limnifs.
- Dictionary training is slow (seconds per MB of corpus). Cache trained
  dictionaries; don't re-train per encode.
- FASTCOVER is the modern training algorithm (2018+); ECov is the simpler
  older one. Port FASTCOVER; ECov is the fallback if FASTCOVER is too slow.
