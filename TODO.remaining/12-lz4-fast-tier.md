# Task 12: lz4 fast-tier ratio on low-redundancy data

## Status: done (2026-08-29)

## Root cause (not tuning — a bug)

Tokenizing our arial.ttf output against the reference showed ours
contained **zero matches**: the entire 23 MB file was literals. The
old in-house `compress_block` had an "incompressibility detector"
that probed the first 256 positions and bailed to literal-only mode
for the WHOLE file when it saw < 1 match there. arial.ttf starts with
a TTF table directory (pseudo-random checksums/offsets), so the
detector fired on exactly the file it was meant to spare. The
reference found 979,723 matches / 31.5% coverage on the same bytes.

## Fix: port the C fast loop

`omnizip-lz4/src/block.rs::compress_block` is now a line-by-line
port of `LZ4_compress_generic` (lz4.c, byU32, noDict, notLimited,
acceleration = 1 — what `lz4 -1` runs):

- 5-byte hash (`LZ4_hash5`, LE variant pinned for cross-machine
  determinism) into a 4096-entry u32 table
- skip-stride: +1 stride per 64 consecutive misses, reset per
  sequence (replaces the detector — 4 MB of rand.bin compresses in
  6 ms)
- catch-up backward match extension into the literal run
- "test next position" retry: consecutive matches with zero-literal
  tokens (`_next_match`, token = 0)
- mflimit/matchlimit end conditions (last 12/5 bytes), ip-2 table
  fill, exact upstream length-extension encoding

Also fixed (found by the new interop gate): the LZ4 **frame** layer
omitted the mandatory header checksum (HC). `compress_frame` now
emits `(XXH32(descriptor) >> 8) & 0xFF` and `decompress_frame`
verifies it. Before this, `lz4 -d` rejected every frame we wrote.

## Results (ours vs `lz4 -1`, /tmp/sweep 10-corpus)

arial 0.9999, bin1 1.0000, bin2 0.9999, csv2m 1.0000,
dbdump 0.9999, fits4m 1.0030, rand 1.0039, rfc 0.9999,
rustsrc 1.0000, words 1.0000.

The ≤1.0000 cells are exact algorithm parity minus the reference's
~19-byte frame overhead. fits4m/rand sit 0.3-0.4% over because the
CLI frame stores incompressible blocks raw; our block stream expands
literals ~0.4% by format. Both far inside acceptance.

## Acceptance

- [x] arial.ttf and rfc.txt within 1.02x of `lz4 -1` (0.9999 both)
- [x] All 10 corpora ≤ 1.004x
- [x] Round-trip suite green (37 tests), incl. new regression
      `incompressible_prefix_does_not_disable_matching`
- [x] New interop gate `interop_with_system_lz4` (frame decodes via
      reference CLI; skips when the binary is absent)
- [x] Determinism recording regenerated
