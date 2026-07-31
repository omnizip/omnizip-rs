# 01 — LZMA format overview

## Container hierarchy

```text
XZ stream  (`.xz` file)
 └─ Stream header (magic + flags + CRC32)
 ├─ Block 0
 │   ├─ Block header (header size + flags + filters + CRC32)
 │   ├─ Compressed data (LZMA2 payload)
 │   └─ Block padding + CRC64
 ├─ Block 1
 │   └─ ...
 ├─ Index (record per block + CRC64)
 └─ Stream footer (CRC32 + backward size + magic)

LZMA2 stream (inside an XZ block, or standalone `.lzma2`)
 └─ Chunk 0 (control byte + optional dict reset + data)
 ├─ Chunk 1
 └─ End marker (control byte = 0x00)

LZMA1 raw stream (`.lzma` alone file)
 └─ Properties byte (lc/lp/pb)
 ├─ Dictionary size (4 bytes LE)
 ├─ Uncompressed size (8 bytes LE; 0xFFFF_FFFF_FFFF_FFFF = unknown)
 ├─ Range-coded data
 └─ End-of-stream marker (optional)
```

## What each layer does

| Layer | Owns | Does NOT own |
|---|---|---|
| **LZMA1** | range coder, literal/match encoding, probability models | chunking, dictionary resets, integrity checking |
| **LZMA2** | chunk boundaries, dictionary resets, compressed/uncompressed chunk type | entropy coding, match finding |
| **XZ** | stream framing, block headers, CRC32/CRC64 integrity, multi-block, index | compression algorithm (pluggable via filter chain) |

An XZ file uses LZMA2 as its compression filter. LZMA2 wraps LZMA1 data
into chunks with explicit dictionary resets. LZMA1 is the raw range-coded
bitstream.

## Why three layers?

1. **LZMA1** is the core algorithm — range coding + probability models +
   match/literal decisions. It cannot self-delimit (no chunk boundaries).
2. **LZMA2** adds chunking: the encoder decides where to cut the LZMA1
   stream into chunks, optionally resetting the dictionary. This allows
   random access (seek to chunk N) and multi-threaded encode.
3. **XZ** adds integrity (CRC32/CRC64), multi-filter chaining (BCJ →
   LZMA2), and stream framing (magic numbers, index for random access).

## Properties byte

The LZMA1 properties byte encodes the three LZMA tuning parameters:

```text
 properties = pb * 45 + lp * 9 + lc
```

| Parameter | Range | Default (xz -6) | Effect |
|---|---|---|---|
| `lc` (literal context bits) | 0–8 | 3 | How many previous bytes' high bits select the literal probability context |
| `lp` (literal position bits) | 0–4 | 0 | How many low bits of position select the literal probability context |
| `pb` (position bits) | 0–4 | 2 | How many low bits of position select the match probability context |

Invalid properties (e.g. lc + lp > 4) MUST be rejected on decode.

## Dictionary

The sliding window for match references. Size is configurable (4 KiB to
1 GiB). On LZMA2 chunk boundaries, the encoder may reset the dictionary
(empty the match history).

## Levels (xz presets)

| Level | Dict size | lc | lp | pb | nice_len | Depth | Match finder | Mode |
|---|---|---|---|---|---|---|---|---|
| 0 | 4 MiB | 4 | 0 | 0 | 128 | 2 | HC4 | fast |
| 1 | 1 MiB | 3 | 0 | 2 | 128 | 4 | HC4 | fast |
| 2 | 2 MiB | 3 | 0 | 2 | 128 | 8 | HC4 | fast |
| 3 | 4 MiB | 3 | 0 | 2 | 128 | 16 | HC4 | fast |
| 4 | 4 MiB | 3 | 0 | 2 | 273 | 16 | HC4 | normal |
| 5 | 8 MiB | 3 | 0 | 2 | 273 | 32 | BT4 | normal |
| 6 | 8 MiB | 3 | 0 | 2 | 273 | 48 | BT4 | normal (default) |
| 7 | 16 MiB | 3 | 0 | 2 | 273 | 96 | BT4 | normal |
| 8 | 32 MiB | 3 | 0 | 2 | 273 | 192 | BT4 | normal |
| 9 | 64 MiB | 3 | 0 | 2 | 273 | 384 | BT4 | normal |

"fast" mode = greedy/lazy parsing. "normal" mode = optimal parsing (DP).
Source: `xz/liblzma/lzma/lzma_encoder_presets.c`.

## Cross-references

- Ruby reference: `omnizip/lib/omnizip/algorithms/lzma/constants.rb`
- C reference: `xz/src/liblzma/lzma/lzma2_encoder.c`
- Spec doc: LZMA SDK `lzma-specification.txt` (Igor Pavlov)
- See: [02-lzma-range-coder.md](02-lzma-range-coder.md)
