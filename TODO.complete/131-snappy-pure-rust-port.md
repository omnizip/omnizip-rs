# TODO 131: Snappy pure-Rust port (replace `snap` wrapper)

## Problem

`omnizip-snappy` (107 LOC) wraps the `snap` crate entirely. The
workspace convention is every codec implemented from spec. Snappy
is the simplest format we wrap — RFC-style spec at
<https://github.com/google/snappy/blob/main/format_description.txt>.

## Scope

Snappy is a basic LZ77 variant:
- No entropy coding (raw literals + raw matches).
- 1-byte tag encoding distinguishes literal vs match.
- Variable-length integer encoding (varint) for offsets/lengths.
- Sliding window: 64 KiB.

## Implementation plan

1. Decoder (`omnizip-snappy/src/decoder.rs`): ~200 LOC.
   - Preamble: varint-encoded uncompressed length.
   - Tag loop: 2-bit tag distinguishes 4 match encodings + literal.
2. Encoder (`omnizip-snappy/src/encoder.rs`): ~250 LOC.
   - Hash-table match finder (single probe, no chain).
   - Match emission via the 4 tag formats.
3. Replace wrapper in `lib.rs`.

## Acceptance criteria

- [ ] No `snap` dependency in `Cargo.toml`.
- [ ] Round-trip parity with `snap` on the Snappy test corpus.
- [ ] Throughput ≥ 500 MB/s decode, ≥ 200 MB/s encode on commodity HW.
- [ ] 50+ unit tests covering tag variants, varint edge cases, etc.

## Priority

P1 — Snappy is the easiest pure-Rust port; good first one to validate
the workflow before tackling LZ4 / DEFLATE.
