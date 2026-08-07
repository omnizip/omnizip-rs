# 186: Shared Bitstream Adoption

## Priority: P2 (DRY)

## Status: partial — shared module exists, no codecs adopted yet

## Context

PR #173 added `omnizip_codecs::bitstream` with `BitReaderBE`,
`BitReaderLE`, `BitWriterBE`, `BitWriterLE` (14 tests, both bit
orders).

Five codecs still have their own BitReader implementations:
- FLAC (`omnizip-flac/src/bitreader.rs`) — MSB-first + unary/Rice
- Brotli (`omnizip-brotli/src/decoder.rs`) — MSB-first inline
- ZSTD FSE (`omnizip-zstd/src/fse/bitstream.rs`) — LSB-first
- libdeflate (`omnizip-libdeflate/src/inflate.rs`) — LSB-first
- GLZA (`omnizip-glza/src/entropy.rs`) — MSB-first

## Adoption plan (one codec per PR)

### PR 1: FLAC

Attempted in this session. Failed because the shared reader's
u64-accumulator pre-fill conflicts with FLAC's `peek_byte` semantics
(which needs to see the next whole byte without consuming the partial
byte in the accumulator).

**Fix needed**: Add a `bit_count()` accessor to the shared reader so
adopters can compute the true byte position. Or add a `peek_byte()`
method to the shared reader that handles partial-byte state.

### PR 2: ZSTD FSE

The ZSTD FSE bitstream is LSB-first and uses a reverse-read pattern
(reads from the end of the block). The shared `BitReaderLE` is
forward-only. Need either a reverse variant or keep ZSTD's local
implementation.

### PR 3: Brotli

Brotli's BitReader is inline in decoder.rs. Extracting it to use the
shared module is straightforward (MSB-first, no special methods).

### PR 4: libdeflate

Similar to Brotli — LSB-first, no special methods.

### PR 5: GLZA

MSB-first with arithmetic coding on top. Thin wrapper.

## Acceptance criteria

- [ ] At least 3 codecs use the shared bitstream module
- [ ] No behavior change (all existing tests pass)
- [ ] LOC reduction ≥200 across adopted codecs
