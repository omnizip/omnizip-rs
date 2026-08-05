# TODO 121: ZSTD L12+ regression on highly-repetitive inputs

## Status

**FIXED.** Root cause found and fixed.

## Original symptom

`omnizip_zstd::compress` at L12 (Better) and above produced
500-700× larger output than L1 and took 70,000× longer on highly
repetitive inputs like `b"The quick brown fox jumps over the lazy
dog. ".repeat(2000)`.

```
Level       Output bytes    Time
────────────────────────────────────
Fastest     74              176 µs
Fast        72              194 µs
Default     50,842          13.94 s     ← regression
Better      50,842          95.74 s     ← regression
Best        (killed after 2 minutes)
```

Filed by LimniFS in `docs/omnizip-proposals/zstd-default-broken.md`.

## Root cause

`find_best_match_chain` and `probe_match` used **inconsistent**
match-length caps:

```rust
// find_best_match_chain — TIGHTER cap
let max_extend = limit.saturating_sub(ip).min(BLOCK_MAX_SIZE);
// m_len ≤ MIN_MATCH + (limit - ip - MIN_MATCH) = limit - ip

// probe_match — LOOSER cap
m_len += count_match(src, ip + m_len, src, candidate + m_len,
                     limit + MIN_MATCH - ip - m_len);
// m_len ≤ MIN_MATCH + (limit + MIN_MATCH - ip - MIN_MATCH) = limit + MIN_MATCH - ip
```

For long matches on periodic data, `probe_match` (used by lazy / lazy2
look-ahead at `ip+1` and `ip+2`) returned lengths **3 bytes longer**
than `find_best_match_chain` returned at `ip`. The lazy2 deferral
check `m2.len > m1.len + 1` then spuriously fired at every position,
causing the parser to advance one byte at a time despite finding
huge matches.

Result: lazy2 walked the entire input byte-by-byte, each position
finding a huge match that was always "deferred". After 90,000 single-
byte advances the parser finally accepted a match, but by then the
seq_store had accumulated so many literal runs that the compressed
output exceeded the raw block size, triggering fallback.

## Fix

Align the chain-walking length cap with the probe length cap:

```rust
// Was: limit.saturating_sub(ip).min(BLOCK_MAX_SIZE)
// Now: (limit + MIN_MATCH).saturating_sub(ip).min(BLOCK_MAX_SIZE)
let max_extend = (limit + MIN_MATCH).saturating_sub(ip).min(BLOCK_MAX_SIZE);
```

Now both paths use identical length caps, the lazy2 deferral check
behaves correctly, and the parser accepts long matches on first sight.

## Verified

After fix on the LimniFS repro input:

```
L1:   74 bytes   213 µs
L3:   72 bytes   223 µs
L6:   72 bytes   168 µs
L9:   72 bytes   166 µs
L12:  73 bytes   314 µs   ← was 50842 bytes / 13.5 s
L19:  73 bytes   354 µs   ← was 50842 bytes / 95 s
L22:  73 bytes   369 µs   ← was killed at 2 min
```

All 174 existing ZSTD tests still pass. Workspace test time drops
from ~50 s to ~2 s for the ZSTD suite.
