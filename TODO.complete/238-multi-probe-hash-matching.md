# 238 — Multi-Probe Hash Matching for Brotli

- **Status:** SUPERSEDED — subsumed by the cost-aware optimal parser
  (TODO 240). The DP at each position considers all match candidates
  from the hash chain; probing multiple hashes would only find
  subsets of the same candidates the DP already evaluates. No
  additional ratio win available beyond what `optimal_parse` delivers.
- **Priority:** P1 (biggest single ratio win)
- **Crate:** `omnizip-brotli`, `omnizip-codecs`
- **Depends on:** none
- **Estimated effort:** 3 days

## Problem

The from_spec encoder uses a single 4-byte hash probe per position.
This misses matches that start at byte offsets where the 4-byte
prefix doesn't hash to the same bucket as a previous occurrence.

The C reference uses multi-probe matching: at each position, it
hashes multiple byte combinations (e.g., 4-byte, 5-byte, 8-byte)
to find more match candidates. This finds 2-5x more matches on
text data with repetitive structure.

## Example

CSV data: rows like "12345,user_12345,city_123,..."
At position 5 of row 2 (",user"), the 4-byte hash is of ",use".
At position 5 of row 1, the same 4 bytes appear. Match found!

But at position 0 of row 2 ("1235..."), the hash differs from
row 1's position 0 ("1234..."). No match found via 4-byte hash.
An 8-byte hash of "1235,use" vs "1234,use" also differs.

However, a 5-byte hash at position 1 ("2345,") matches row 1's
position 1 ("2345,"). This match is only found by probing at
multiple positions, not just position 0.

## Design

Extend `HashChainMatchFinder` to support multi-probe matching:

```rust
pub struct HashChainConfig {
    ...
    pub num_probes: u32,  // Number of hash probes per position
}
```

At each position, probe `num_probes` hash functions:
1. hash4(data, pos) — standard 4-byte hash
2. hash4(data, pos+1) — probe next position (look-ahead)
3. hash4(data, pos+2) — probe further

For each probe, walk the hash chain and collect candidates.
Pick the longest match across all probes.

## Acceptance criteria

- [ ] Multi-probe matching implemented in HashChainMatchFinder
- [ ] num_probes=3 at quality 5+ for text input
- [ ] CSV ratio improvement >= 30% (from ~20% to ~14%)
- [ ] Encoding time increase <= 50%
