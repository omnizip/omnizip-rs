# 20 — Snappy

- **Priority:** P2 (data-lake / OLAP interop)
- **Depends on:** [01](01-codec-trait-registry.md)
- **Estimated effort:** 1 week
- **Crate:** `omnizip-snappy`

## Why

Snappy (Google 2011) is the codec for Parquet, ORC, Avro, and SQLite WAL
files. LimniFS users storing these formats need Snappy for interop.
Pure-Rust `snap` crate (v1+) is production-quality.

## Approach

Wrap the [`snap`](https://crates.io/crates/snap) crate (MIT/Apache) as an
`omnizip-snappy` codec. No porting required — `snap` is the standard
implementation and is byte-identical with the C++ reference.

If we later want a self-contained implementation (to remove the snap dep),
port from the C++ reference at `google/snappy`. Not in scope for this task.

## Acceptance

- `omnizip-snappy` wraps `snap::Encoder` and `snap::Decoder`.
- Round-trips on every corpus fixture.
- Output byte-identical to `snappy -c` (C++ reference) on the same input.
- Decode throughput ≥ 500 MB/s; encode throughput ≥ 500 MB/s (Snappy is
  designed for speed; pure-Rust `snap` matches).
- Clippy clean, no `unsafe`, deterministic.
