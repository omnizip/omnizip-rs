# Task 11: deflate level tiers (omnizip-libdeflate)

## Status: done (2026-08-29) — plus two data-integrity bugs fixed

## Tiers

`Lz77Params` mirrors zlib's `configuration_table` exactly; levels 1-3
run `deflate_fast` (greedy), 4-9 run `deflate_slow` (lazy), level 0 is
stored-only. `collect_tokens_with(input, &params)` is the single LZ77
parse (the fixed-Huffman writer's duplicated inline loop was removed
and now consumes tokens too). The dynamic/fixed/stored emission
contest stays as the final step at each level.

## Bugs found on the way

1. **Dynamic-Huffman header corruption (pre-existing, released)**: the
   local length limiter (zlib-CPI-style repair) could leave a 250+-
   symbol table over-subscribed by one 2^-15 unit on skewed
   distributions — the reference decoder rejects the block header
   ("invalid literal/lengths set"). Reproduced on main (0.21.22):
   arial decoded to 23,403,412 bytes instead of 23,278,008. Fixed by
   delegating to the shared package-merge builder in
   `omnizip-codecs::huffman` (Kraft-exact); regression test
   `huffman_lengths_are_kraft_complete`.
2. **Chain self-loop in the lazy parse**: re-inserting the current
   position after a match emission chained it to itself
   (`chain[i] == i`), so every subsequent walk through that bucket
   burned its whole budget on one candidate. This silently halved
   match quality (2x as many 3-5 byte matches as zlib). Fixed by
   starting the covered-position insert loop at i+1.

## Results (ours vs zlib at the same level, 10-corpus sweep)

| level | range |
|---|---|
| 1 | 0.9889-0.9998 (beats zlib -1 everywhere) |
| 6 | 0.9994-1.0003 |
| 9 | 0.9992-1.0004 |

Every output byte-verified through the reference zlib decompressor.

## Acceptance

- [x] Levels 1/6/9 produce distinct outputs with monotone sizes
      (`level_tiers_are_distinct_monotone_and_round_trip`)
- [x] lv9 within 1.02x of zlib -9 on the broad corpus (worst 1.0004)
- [x] 27 crate tests green; determinism recording regenerated
