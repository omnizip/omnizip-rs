# TODO 134: BLOSC pure-Rust port (replace `lz4_flex` wrapper)

## Problem

`omnizip-blosc` (821 LOC) wraps `lz4_flex` for the inner
compression layer. Blosc is a meta-codec that delegates to LZ4 /
ZSTD / others for actual compression.

## Proposed fix

Once TODO 132 (LZ4 from spec) lands, swap `lz4_flex` for the
in-house LZ4. If Blosc also uses ZSTD internally, swap that to the
in-house ZSTD too.

## Acceptance criteria

- [ ] No `lz4_flex` dependency in `omnizip-blosc`.
- [ ] Round-trip parity with the C `blosc` tool.
- [ ] Throughput ≥ 500 MB/s on numeric arrays.

## Priority

P2 — depends on TODO 132.

## Dependencies

- TODO 132 (LZ4 from spec).
