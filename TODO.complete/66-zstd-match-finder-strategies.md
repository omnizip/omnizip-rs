# 66 — ZSTD match finder strategies

## Gap

The current ZSTD encoder uses a single hash-chain match finder with
greedy matching. The C reference supports 9 strategies tuned per
compression level:

| Strategy      | Levels | Description |
|--------------|--------|-------------|
| fast         | 1-2    | Single hash probe, no chain |
| double_fast  | 3-5    | Two hash tables (short + long) |
| greedy       | 6      | Hash chain, pick first match |
| lazy         | 7-9    | Hash chain, look-ahead-1 |
| lazy2        | 10     | Look-ahead-2 |
| btlazy2      | 11     | Binary tree, look-ahead-2 |
| btopt        | 12-16  | Binary tree, optimal parse |
| btultra      | 17-19  | Binary tree, ultra optimal |
| btultra2     | 20-22  | Binary tree + statistics |

Currently `cparams.rs` selects parameters per level but the encoder
ignores the strategy field and always uses greedy hash-chain. This
caps compression ratio significantly.

## Implementation plan

1. **Separate `Strategy` enum** in `cparams.rs`.
2. **`match_finder/fast.rs`** — single-probe fast mode (port
   `~/src/external/zstd/lib/compress/zstd_fast.c`).
3. **`match_finder/double_fast.rs`** — two-hash mode (port
   `zstd_double_fast.c`).
4. **`match_finder/lazy.rs`** — look-ahead-1 mode (port
   `zstd_lazy.c`).
5. **Wire** `Strategy` into `block.rs` to dispatch.

## Test strategy

- L1 (fast) must be ≥ 2× faster than L6 (lazy) on enwik8.
- L6 ratio must beat L1 by ≥ 3 percentage points.
- All levels round-trip through own decoder and reference `zstd -d`.
