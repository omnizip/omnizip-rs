# 14 — ZSTD Finite State Entropy (FSE)

FSE is ZSTD's entropy coder — a table-based variant of asymmetric numeral
systems (ANS), designed by Jarek Duda and adapted for ZSTD by Facebook.
It is used to encode literal lengths codes, match length codes, and offset
codes in the Sequences section.

## Overview

An FSE table maps each symbol to a number of states proportional to its
frequency. The decoder reads bits to transition between states and emit
symbols. The result is near-arithmetic-coding compression ratios at
near-Huffman-coding speed.

## FSE table format (in the bitstream)

### FSE_table_description

```text
  Accuracy_Log (4 bits + 6, range 6–9):
    Value 0..15 → Accuracy_Log = 6 + value

  For each symbol (until total probability = 1 << Accuracy_Log):
    If remaining probability is large:
      Probability (2 bits prefix + value):
        00  → 0 bits follow → probability = 1 << (Accuracy_Log - 5)
        01  → 1 bit follows → probability = 2 << ...
        10  → value encoded via bit-by-bit low vs high comparison
        11  → value encoded via interleaved FSE (rare)
    Else (remaining is small):
      Use smaller encoding
```

The symbol set is determined by context: for literal lengths it's
0–35, for match lengths 0–52, for offsets 0–31. Symbols not listed get
probability 0 (never emitted).

### Normalised distribution

After parsing, the decoder builds a normalised distribution table:
`distribution[symbol] = number of states allocated to that symbol`.
The total MUST equal `1 << Accuracy_Log` (typically 64–512).

## State table construction

From the normalised distribution, build two arrays:

- `stateTable[]`: maps old state → new state (after reading the
  symbol's extra bits).
- `symbolTable[]`: maps state → symbol emitted at that state.

Construction algorithm (Vose's alias method simplified for FSE):

```text
 1. For each symbol s with probability p > 0:
    - Assign p positions in the state table.
 2. Distribute positions using a "high-low" interleaving:
    - high[s] = symbols with probability >= threshold
    - low[s]  = symbols with probability < threshold
    - For each state position i (from 0):
      Take from a high symbol if available, else from a low symbol.
 3. The state table is: stateTable[i] = (next_state_for_symbol)
 4. symbolTable[i] = the symbol that occupies state i
```

## Decode algorithm

### Initialisation

Read `Accuracy_Log` bits from the bitstream to get the initial state
value (the initial state is simply the first bits read, masked to
`Accuracy_Log` width).

### Decode one symbol

```text
1. symbol = symbolTable[state]
2. Emit symbol
3. Read the number of extra bits for this symbol (from a lookup table
   that depends on Accuracy_Log and the symbol's baseline)
4. Read `numBits` extra bits → `extraBits`
5. newStateBase = stateTable[state]    // baseline for the next state
6. state = newStateBase + extraBits     // actual next state
```

The decoder alternates between reading the FSE table description
(once per block, if present) and decoding symbols using that table.

## Bitstream reader

ZSTD bitstreams are read in **reverse** (from the last byte to the
first, and within each byte from MSB to LSB). This is different from
most bitstream formats and is a common source of porting bugs.

```text
  // Initialisation
  bitContainer = read 4 bytes from END of bitstream (reversed)
  bitPos = 0  // number of bits consumed from bitContainer
  // ...
  // Read n bits:
  bits = (bitContainer >> bitPos) & ((1 << n) - 1)
  bitPos += n
  if bitPos >= 25:
    // refill: shift bitContainer and read next byte from the
    // reversed input
    bitContainer >>= (bitPos & 7)
    bitPos = bitPos & 7
    if more_bytes_available:
      bitContainer |= (u64::from(next_byte_reversed) << (24 + bitPos))
```

The `>= 25` threshold ensures there are always at least 7 bits
available before the next read (maximum single-read is 6 bits for
offset extra bits).

## Determinism

FSE is fully deterministic: same distribution table + same symbol
sequence ⇒ byte-identical bitstream. The table construction (Vose's
method) is also deterministic: same input distribution ⇒ same table,
byte-for-byte.

## Cross-references

- Ruby: `omnizip/lib/omnizip/algorithms/zstandard/fse/bitstream.rb`
- Ruby: `omnizip/lib/omnizip/algorithms/zstandard/fse/table.rb`
- Ruby: `omnizip/lib/omnizip/algorithms/zstandard/fse/encoder.rb`
- Spec: RFC 8878 §4.1.1 (Sequences_Section)
- C: `zstd/lib/common/fse.h`, `zstd/lib/common/entropy_common.c`
- Paper: Jarek Duda, "Asymmetric Numeral Systems" (arXiv:0902.0271)
- Rust port: `omnizip-zstd/src/fse/` (pending)
