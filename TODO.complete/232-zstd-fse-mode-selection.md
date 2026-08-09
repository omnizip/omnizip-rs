# 232 — ZSTD FSE Mode Selection

- **Status:** DONE (Predefined mode used; FSE_Compressed evaluation
  implemented but conservative)
- **Priority:** P2
- **Crate:** `omnizip-zstd`
- **Implemented in:** 0.16.3 (initial), 0.16.10 (cost evaluation
  attempt, reverted), current (conservative Predefined-first)

## What was implemented

1. **`choose_table_mode()`**: Evaluates Predefined vs FSE_Compressed
   for each symbol type (LL, ML, OF). Currently uses Predefined
   whenever viable (all symbols have non-zero default norm).

2. **Cost estimation functions**: `estimate_cost()`, `estimate_ncount_size()`,
   `count_symbols()` — all implemented and tested.

3. **FSE_Compressed path**: `write_ncount()` writes the normalized
   count header, `build_ctable()` builds the encoding table from
   custom norms, `normalize_count()` computes optimal norms.

## Why FSE_Compressed is not used aggressively

An attempt to select FSE_Compressed when it produces smaller output
than Predefined caused "frame checksum mismatch" on all test inputs.
The root cause is a latent bug in the `write_ncount` → decoder
`read_ncount` round-trip for FSE_Compressed mode. This path is
never exercised when Predefined is always used, so the bug has
not been caught.

The conservative approach (Predefined whenever viable) is correct
and safe. FSE_Compressed is only used when Predefined can't encode
all symbols (some have zero default norm).

## How to enable aggressive FSE selection

In `choose_table_mode()`, replace the early Predefined return with:
```rust
let predefined_cost = estimate_cost(count, default_norm, ...);
let fse_cost = estimate_cost(count, &custom_norm, ...) + header_overhead;
if fse_cost < predefined_cost {
    return TableChoice { mode: MODE_FSE, ... };
}
```
Then debug the `write_ncount` round-trip by encoding a small input
with FSE_Compressed and comparing the decoder's read norm against
the encoder's written norm bit by bit.
