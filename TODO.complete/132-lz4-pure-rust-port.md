# TODO 132: LZ4 pure-Rust port (replace `lz4_flex` wrapper)

## Problem

`omnizip-lz4` (619 LOC) wraps `lz4_flex` for the fast encoder. The
HC encoder is already in-house (`src/hc.rs`). Wire format is
specified at <https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md>.

## Scope

LZ4 is a simple LZ77 variant:
- Block format: token byte (literal length high nibble + match
  length low nibble) + varint literal count + literals + varint
  offset + varint match length extension.
- Frame format: magic + flags + content size + (optional dict id)
  + blocks + (optional checksum) + EOF magic.
- Sliding window: 64 KiB.

## Implementation plan

1. Block decoder (`omnizip-lz4/src/block_decoder.rs`): ~120 LOC.
2. Block encoder (`omnizip-lz4/src/block_encoder.rs`): ~150 LOC.
   Use shared `HashChainMatchFinder` from `omnizip-codecs`.
3. Frame decoder: ~200 LOC.
4. Frame encoder: ~150 LOC.
5. Replace `lz4_flex` calls in `lib.rs`.
6. Keep HC encoder (`hc.rs`) as-is.

## Acceptance criteria

- [ ] No `lz4_flex` dependency in `Cargo.toml`.
- [ ] Round-trip parity with the reference C `lz4` CLI.
- [ ] Throughput ≥ 800 MB/s decode, ≥ 400 MB/s encode.
- [ ] 60+ unit tests covering block + frame formats.

## Priority

P1 — LZ4 is simpler than LZMA/ZSTD; port validates the workflow
for LZ77 codecs.
