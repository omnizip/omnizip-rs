# Spec — normative format specifications

Bit-level, normative specifications of every compression format implemented
in omnizip-rs. These documents define the wire format — every field, every
bit, every state transition. Code that disagrees with these specs is a bug.

A format change updates these specs FIRST; code follows spec, never the
reverse. (The historical task/requirements/reference boards that drove the
ports live in git history; open work is tracked in GitHub issues.)

## LZMA format specification

| # | File | Topic |
|---|---|---|
| 01 | [01-lzma-overview.md](01-lzma-overview.md) | LZMA1 / LZMA2 / XZ relationship; container hierarchy |
| 02 | [02-lzma-range-coder.md](02-lzma-range-coder.md) | Range coder: probability model, bit encoding/decoding |
| 03 | [03-lzma-state-machine.md](03-lzma-state-machine.md) | 12-state machine tracking match/literal history |
| 04 | `04-lzma-literal-coder.md` | Context-coded literal encoding (lc, lp parameters) — *(not yet written)* |
| 05 | `05-lzma-match-coder.md` | Length + distance coding; rep-match handling — *(not yet written)* |
| 06 | `06-lzma-match-finder.md` | Hash chain (HC3/HC4) and binary tree (BT2/BT4) — *(not yet written)* |
| 07 | `07-lzma-optimal-parser.md` | DP-based optimal parsing (levels 4–9) — *(not yet written)* |
| 08 | [08-lzma2-container.md](08-lzma2-container.md) | LZMA2 chunk format: control byte, chunk types  *(not yet written)* |
| 09 | `09-xz-container.md` | XZ stream: magic, flags, blocks, index, CRC64 — *(not yet written)* |

## ZSTD format specification

| # | File | Topic |
|---|---|---|
| 10 | [10-zstd-frame.md](10-zstd-frame.md) | Frame header, frame content size, window size |
| 11 | `11-zstd-blocks.md` | Block header, raw/RLE/compressed blocks — *(not yet written)* |
| 12 | `12-zstd-literals.md` | Literals section: raw/RLE/compressed/treeless — *(not yet written)* |
| 13 | `13-zstd-sequences.md` | Sequence execution: literal copy + match copy — *(not yet written)* |
| 14 | [14-zstd-fse.md](14-zstd-fse.md) | Finite State Entropy: table, bitstream, decode |
| 15 | `15-zstd-huffman.md` | Huffman coding: header, tree, stream — *(not yet written)* |

## Notation conventions

All multi-byte integers are **little-endian** unless stated otherwise. Bit
diagrams show bit 0 (LSB) on the right. Field widths are in bits unless
suffixed with `B` (bytes).

```text
  Byte 0              Byte 1
  7  6  5  4  3  2  1  0  7  6  5  4  3  2  1  0
 ┌──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┬──┐
 │     field A      │  B  │           field C          │
 └──────────────────┴─────┴────────────────────────────┘
  ←──── 5 bits ────→ ← 2 → ←──────── 9 bits ────────→
```

Range-coder probability values are unsigned 11-bit (0–2047). "Adapt" means
"move the probability toward the observed outcome by a small step."
