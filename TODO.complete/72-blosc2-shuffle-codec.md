# 72 — omnizip-blosc: BLOSC2 multi-codec container for scientific data

## Source
- Proposal: `../../limnifs/limnifs/docs/omnizip-blosc2-proposal.md`
- Spec: c-blosc2 (BSD-3) — container format reference only, no code copying
- Algorithm: byte/bit shuffle + LZ4/ZSTD inner codec

## Why
Scientific float32 data (FITS images, sensor arrays, NumPy) compresses
poorly with general-purpose codecs because the bytes of each float are
uncorrelated. BLOSC2's shuffle reorders bytes so that all byte-0s of
all floats are adjacent, then all byte-1s, etc. — exposing redundancy
to the inner LZ4/ZSTD codec. Expected: 80% → ~40% on smooth floats.

## Two implementation options

### Option A — Full `omnizip-blosc` crate (~1300 LOC)

```
omnizip-blosc/
  src/
    lib.rs              — Codec trait impl, public API
    shuffle.rs          — byte shuffle + bit shuffle (400 LOC)
    container.rs        — 32-byte header + per-chunk framing (300 LOC)
    inner_codec.rs      — LZ4/ZSTD wrapper (200 LOC)
    tests/
      round_trip.rs     — shuffle + codec round-trip
      differential.rs   — Python blosc2 byte-identical shuffle output
```

### Option B — Shuffle-only in `omnizip-filters` (~200 LOC)

Add `ByteShuffle` and `BitShuffle` filters to the existing
`omnizip-filters` crate. Compose with existing LZ4/ZSTD. No new crate
needed. Captures most of the ratio benefit at minimal effort.

**Recommendation**: start with Option B (quick win), escalate to
Option A if LimniFS needs the full container format.

## Wire format (Option A)

```text
Header (32 bytes):
  magic "BLOSC2\0" (8 bytes)
  version (1 byte)
  item_size (1 byte: 1/2/4/8)
  shuffle_mode (1 byte: 0=none, 1=byte, 2=bit)
  inner_codec (1 byte: 1=LZ4, 2=ZSTD)
  uncompressed_size (8 bytes LE)
  compressed_size (8 bytes LE)
  chunk_count (4 bytes)

Chunks:
  for each chunk:
    chunk_header (8 bytes: compressed_size + uncompressed_size)
    shuffled_data (if uncompressed)
    inner_compressed_data
```

## Acceptance criteria

1. `compress(float32, item_size=4, shuffle=Byte, codec=Lz4)` output ≤ 50% of raw LZ4.
2. Round-trip integrity for all item sizes (1/2/4/8).
3. Bit-shuffle beats byte-shuffle on float32.
4. Differential: Python `blosc2` produces byte-identical shuffle output.

## Codec ID
`0x0A` (assigned by LimniFS).
