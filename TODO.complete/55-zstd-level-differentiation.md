# Task 55: ZSTD level differentiation

## Status: completed (partial)
## Priority: P0

## What was done

- Created `encoder/cparams.rs` with full ZSTD_defaultCParameters[0] table.
- Wired level through `encode_frame_compressed` → `write_block` → match finder.
- `hash_log` varies by level (14 at L1, 19 at L6, 25 at L22).
- `min_match` varies by level (7 at L1, 5 at L6, 3 at L22).
- Acceptance check added to skip matches below min_match threshold.

## Results

50K mixed text input:
- L1: 14.4% (hash_log=14, min_match=7)
- L3: 12.8% (hash_log=17, min_match=5)
- L6+: 12.8% (same hash table size is sufficient for this input)

## Remaining work

- Add lazy parser for L6+ (look-ahead-1).
- Add cost model to reject unprofitable short matches.
- Cross-block match finding (don't reset hash table between blocks).
