# LZMA — single-stream `.lzma` only

## Status

**Open.** Only the `.lzma` (LZMA-Alone) container is decoded. The
`.xz` container and `.lz` (lzip) container, and LZMA2 multi-chunk
streams, are not yet ported.

## Affected code

`omnizip-lzma/src/decoder/{mod,alone,lzma1}.rs`.

## What the LZMA spec / C reference says

The `tukaani-project/xz` C reference implements three LZMA container
formats:

1. **`.lzma` (LZMA-Alone)** — legacy LZMA Utils format. 13-byte
   header (1 property byte + 4 dict-size bytes + 8 uncompressed-size
   bytes) followed by a raw LZMA1 stream. Trivial container; the
   interesting work is the LZMA1 packet decoder.
2. **`.xz`** — modern XZ Utils format. Stream header (12 bytes:
   magic + stream flags + CRC32), one or more blocks (each with a
   block header + compressed payload + padding + check), an index,
   and a stream footer (12 bytes). The compressed payload is LZMA2.
3. **`.lz` (lzip)** — lzip format. Magic bytes (`LZIP`) + version +
   dictionary size, then one or more LZMA1 members each with a
   trailing CRC32.

LZMA2 is a chunked wrapper around LZMA1: each chunk has a control
byte that signals chunk type (end / uncompressed / compressed) and
reset state, plus the LZMA1 parameters (lc, lp, pb, dict_size) for
that chunk. State (probability models, rep distances, dictionary)
can persist across chunks per the control byte.

## What the Rust port does

The Rust port implements:

- `decoder/lzma1.rs` — the LZMA1 packet decode engine. This is the
  core of every LZMA decoder; it handles literals, matches, rep
  matches, short rep, and EOPM.
- `decoder/alone.rs` — the `.lzma` 13-byte header parser. Calls
  `Lzma1Decoder::decode` with `allow_eopm = true`.

It does NOT implement:
- `.xz` stream container (no block header parser, no CRC32, no
  CRC64, no index, no stream footer).
- LZMA2 chunk manager (no control-byte parser, no chunk-state
  preservation, no dictionary reset logic).
- `.lz` (lzip) container.

## What the Ruby port does

The Ruby port's `xz_utils_decoder.rb` (1,311 LOC) implements the
LZMA1 engine plus LZMA2 multi-chunk state preservation. The
`lzma_alone_decoder.rb`, `lzip_decoder.rb`, and the XZ container
decoders all delegate to it.

## Why the divergence exists

The LZMA2 multi-chunk logic is tightly interwoven with the LZMA1
engine in the Ruby (shared state for probability models, rep
distances, dictionary, range decoder). Porting it requires the
`prepare_state_reset` / `finish_state_reset` /
`set_uncompressed_size` API surface, plus the LZMA2 chunk manager
that drives it. This is substantial (estimated 2,000+ LOC of
careful state-machine porting) and was deferred in favour of
shipping the simpler single-stream path first.

## Impact

- `.xz` files (the most common modern LZMA container) cannot be
  decoded. Differential parity tests for `.xz` fixtures are skipped.
- `.lz` files cannot be decoded.
- LZMA2 chunks within `.xz` cannot be decoded.

The `.lzma` path works end-to-end: 3 fixtures under
`tests/fixtures/lzma/good-*.lzma` decode byte-identically to
`xz -d`.

## Reconciliation plan

1. Port `omnizip/lib/omnizip/algorithms/lzma2/{constants,properties,chunk,chunk_manager}.rb`
   to `omnizip-lzma/src/lzma2/`. This adds the LZMA2 control-byte
   parser and chunk-state machine.
2. Extend `Lzma1Decoder` with `prepare_state_reset`,
   `finish_state_reset`, `set_uncompressed_size`, `set_input`,
   `add_to_dictionary` methods — matching the Ruby API.
3. Port `omnizip/lib/omnizip/algorithms/lzma/xz_utils_decoder.rb`
   multi-chunk paths (the parts currently stubbed).
4. Port the XZ container: stream header (CRC32 of flags), block
   header (CRC32 + filter flags), index (CRC32 + records), stream
   footer (CRC32 + backward size).
5. Implement CRC32 and CRC64 (small, ~50 LOC each).

Estimated effort: 2 weeks of focused work.
