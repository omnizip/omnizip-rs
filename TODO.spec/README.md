# Spec — normative format specifications

Bit-level, normative specifications of every compression format implemented
in omnizip-rs. These documents define the wire format — every field, every
bit, every state transition. Code that disagrees with these specs is a bug.

## Relationship to other TODO directories

| Directory | Purpose | Audience |
|---|---|---|
| `TODO.spec/` | **What** the format is (bit-level normative) | implementors, auditors |
| [`TODO.requirements/`](../TODO.requirements/) | **Why** we implement it (acceptance criteria, use cases) | product owners, testers |
| [`TODO.references/`](../TODO.references/) | **Where** to find the source (Ruby files, C files, RFCs) | porters, researchers |
| [`TODO.omnizip-rs/`](../TODO.omnizip-rs/) | **How** to implement it (task files, phased plans) | developers |

A format change updates `TODO.spec/` FIRST, then `TODO.requirements/`, then
`TODO.omnizip-rs/` task files. Code follows spec, never the reverse.

## LZMA format specification

| # | File | Topic |
|---|---|---|
| 01 | [01-lzma-overview.md](01-lzma-overview.md) | LZMA1 / LZMA2 / XZ relationship; container hierarchy |
| 02 | [02-lzma-range-coder.md](02-lzma-range-coder.md) | Range coder: probability model, bit encoding/decoding |
| 03 | [03-lzma-state-machine.md](03-lzma-state-machine.md) | 12-state machine tracking match/literal history |
| 04 | [04-lzma-literal-coder.md](04-lzma-literal-coder.md) | Context-coded literal encoding (lc, lp parameters) |
| 05 | [05-lzma-match-coder.md](05-lzma-match-coder.md) | Length + distance coding; rep-match handling |
| 06 | [06-lzma-match-finder.md](06-lzma-match-finder.md) | Hash chain (HC3/HC4) and binary tree (BT2/BT4) |
| 07 | [07-lzma-optimal-parser.md](07-lzma-optimal-parser.md) | DP-based optimal parsing (levels 4–9) |
| 08 | [08-lzma2-container.md](08-lzma2-container.md) | LZMA2 chunk format: control byte, chunk types |
| 09 | [09-xz-container.md](09-xz-container.md) | XZ stream: magic, flags, blocks, index, CRC64 |

## ZSTD format specification

| # | File | Topic |
|---|---|---|
| 10 | [10-zstd-frame.md](10-zstd-frame.md) | Frame header, frame content size, window size |
| 11 | [11-zstd-blocks.md](11-zstd-blocks.md) | Block header, raw/RLE/compressed blocks |
| 12 | [12-zstd-literals.md](12-zstd-literals.md) | Literals section: raw/RLE/compressed/treeless |
| 13 | [13-zstd-sequences.md](13-zstd-sequences.md) | Sequence execution: literal copy + match copy |
| 14 | [14-zstd-fse.md](14-zstd-fse.md) | Finite State Entropy: table, bitstream, decode |
| 15 | [15-zstd-huffman.md](15-zstd-huffman.md) | Huffman coding: header, tree, stream |

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
