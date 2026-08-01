# ZSTD FSE decode-table builder — does not handle `-1` low-probability sentinels

## Status

**Open.** This is the current blocker for ZSTD compressed-block decode.

## Affected code

`omnizip-zstd/src/fse/table.rs` — `Table::build`, `allocate_cells`,
`calculate_state_values`.

## What RFC 8878 / the C reference says

RFC 8878 §4.1.2 describes the FSE table-construction algorithm as a
"spread" pattern: symbols are distributed across the table at fixed
step intervals. The C reference (`lib/common/entropy_common.c`,
`FSE_buildDTable_internal`) implements this with a crucial extension:

```c
for (symbol=0; symbol<=maxSymbolValue; symbol++) {
    int const freq = normalizedCounter[symbol];
    if (freq == 0) continue;
    if (freq == -1) {
        // Low-probability symbol: place at highThreshold (top of table).
        tableSymbol[highThreshold--] = (U16)symbol;
        continue;
    }
    // Positive frequency: spread from bottom.
    for (nbOccurrences=0; nbOccurrences<freq; nbOccurrences++) {
        tableSymbol[position] = (U16)symbol;
        position = (position + step) & mask;
        while (tableSymbol[position]) position = (position + step) & mask;
    }
}
```

Symbols with `-1` frequency are "low-probability" markers. They each
get exactly one cell, placed from the top of the table downward. The
positive-frequency symbols fill from the bottom upward. The two
regions meet in the middle.

The C source's predefined offset distribution uses `-1` sentinels:

```c
static const S16 OF_defaultNorm[32] = {
     1, 1, 1, 1, 1, 1, 2, 2,
     2, 1, 1, 1, 1, 1, 1, 1,
     1, 1, 1, 1, 1, 1, 1, 1,
    -1,-1,-1,-1,-1,-1,-1,-1
};
```

Sum of positive entries: 27. Number of `-1` entries: 8. Total cells:
27 + 8 = 35, but `table_size = 32`. The spread algorithm's
collision-detection `while (tableSymbol[position])` skips the
high-threshold cells, so the positive entries actually occupy fewer
cells than their raw count.

## What the Rust port does

`omnizip-zstd/src/fse/table.rs` uses `&[u8]` for the distribution
type, which cannot represent `-1`. The current implementation:

1. Treats `0` as "symbol absent" (skips the symbol entirely).
2. Treats any positive value as the cell count for that symbol.
3. Leaves any unfilled cell as `None`, which becomes a default
   `FseState { symbol: 0, num_bits: 0, baseline: 0 }`.

The current `PREDEFINED_OFFSET_DISTRIBUTION` is:

```rust
pub const PREDEFINED_OFFSET_DISTRIBUTION: [u8; 32] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
```

This sums to 18 (not 32) and has no `-1` sentinels, so 14 cells
remain empty. Those empty cells all decode to symbol 0.

## Why the divergence exists

The Rust port was written before the `-1` sentinel semantics were
understood. The initial implementation matched the Ruby port, which
also uses `[u8]` and has the same limitation (see
`../omnizip/BUGREPORT.09-predefined-distributions-wrong-sum.md`).

## Impact

Every ZSTD frame that uses `MODE_PREDEFINED` for any of the three
sequence streams (literal-length, match-length, offset) decodes wrong
symbols from the FSE bitstream. The wrong symbols cascade into wrong
literal lengths, match lengths, and offsets, producing silently
corrupt output.

The `test-aaaa.zst` differential fixture fails with this root cause.

## Reconciliation plan

1. Change the distribution type from `&[u8]` to `&[i16]` across
   `Table::build`, `Table::build_predefined`, `Table::build_rle`,
   `allocate_cells`, and `calculate_state_values`.
2. Port the C reference's `highThreshold` algorithm:
   - For each symbol in order: if `freq == -1`, place at
     `tableSymbol[highThreshold--]`. Otherwise spread from `position`
     using the step, skipping occupied cells.
3. Update `calculate_state_values` to handle the low-probability
   cells:
   - `nbBits = tableLog` (read the full table index for the next state).
   - `baseline = 0`.
4. Update the three predefined distributions to use the correct C
   source values with `-1` sentinels:
   ```rust
   pub const PREDEFINED_OFFSET_DISTRIBUTION: [i16; 32] = [
        1, 1, 1, 1, 1, 1, 2, 2,
        2, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1,
       -1,-1,-1,-1,-1,-1,-1,-1,
   ];
   ```
5. Verify the differential fixture `test-aaaa.zst` decodes correctly.

Estimated effort: half a day of careful work.

## Workaround

None. ZSTD compressed-block decode is non-functional until this is
fixed. Raw and RLE block decode work correctly.
