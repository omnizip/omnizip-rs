# Brotli Performance Optimization Analysis

## Current state (v0.16.31)

20 MiB CSV benchmark on synthetic data:

| Level | Time  | Ratio | MB/s  |
|-------|-------|-------|-------|
| Q2    | 0.7s  | 5.96% | 29    |
| Q5    | 2.5s  | 1.55% | 8     |
| Q8    | 5.0s  | 1.49% | 4     |
| Q11   | 7.9s  | 1.50% | 2.7   |

vs 0.14.20 (vendored C): 0.37s, 3.6%.
vs 0.16.22 (broken): 17.4s, 18.4%.

## Where time is spent (per 1 MiB chunk at Q5)

| Component | Time | % | Notes |
|-----------|------|---|-------|
| Match finder (hash-chain walk + match_length) | ~80ms | 47% | max_chain=8, nice_match=24 |
| Optimal parser DP (45 boundaries × N positions) | ~60ms | 35% | O(45 * N) |
| Symbol stream + Huffman + bit writing | ~30ms | 18% | |

## Attempted optimizations (results)

### Implemented and shipped

| Change | Effect | Version |
|--------|--------|---------|
| Lower max_chain Q4-Q7 (48→16→8) | 5x faster Q5, better ratio | 0.16.29-0.16.30 |
| Cap iterative parser to ≤256 KiB | 3.3x faster Q8, 5x faster Q11 | 0.16.29 |
| Drop Q11 4-iter back to 2 | 5x faster Q11, no ratio loss | 0.16.29 |
| u128 fast-reject in match_length | 1.4x faster Q5 | 0.16.31 |

### Tested and rejected

| Change | Result | Why rejected |
|--------|--------|--------------|
| `safe_max` pre-compute in match_length | 1.5-2x SLOWER | LLVM already optimizes 3-check pattern better |
| `#[inline]` on match_length | 3-5x SLOWER | Code bloat → register pressure → worse codegen |
| Pure u128 loop (no u64) | SLOWER at Q8+ | Explicit array literal → suboptimal codegen without unsafe |
| COPY_BOUNDARIES 45→16 | 2+pp ratio loss | Finer granularity finds better alignments |
| 4 MiB chunks (instead of 1 MiB) | 4x ratio loss | Lazy parser much worse than optimal_parse |
| `try_into()` for array conversion | 3x SLOWER | Runtime length check not elided |

### Not yet attempted (future work)

## Architecture-level optimizations

### 1. Cross-chunk match finder reuse (HIGH IMPACT)

**Problem**: 20 MiB input split into 20 × 1 MiB chunks. Each chunk
creates a fresh HashChainMatchFinder. Matches can only reference data
within the same 1 MiB chunk.

**Fix**: Create ONE match finder over the full input. Thread it
through all chunks. Chunk N+1 sees hash entries from chunks 0..N.

**Expected win**:
- Ratio: 10-30% improvement (longer match distances)
- Speed: 5-10% (no per-chunk hash table setup)

**Complexity**: Moderate refactor (3-5 hours).

### 2. Skip incompressible sections (MEDIUM IMPACT)

**Problem**: Every position is probed via hash + chain walk, even for
sections that clearly have no matches (random binary data).

**Fix**: Detect incompressible runs (4+ consecutive non-matching
positions) and skip `skip_len` positions ahead without probing.

**Expected win**:
- Speed: 2-3x on binary/random data
- Ratio: neutral

**Complexity**: Low (1 hour). Already implemented in the lazy parser
path but not in optimal_parse.

### 3. Larger metablock size (MEDIUM IMPACT)

**Problem**: 1 MiB metablocks cause per-metablock Huffman table overhead
(~200 bytes per table × 3 tables = 600 bytes per chunk). For 20 chunks,
that's 12 KB of overhead.

**Fix**: Use 4 MiB metablocks with optimal_parse (need to raise the
DP threshold from 1 MiB to 4 MiB). The DP at 4 MiB is ~4s per chunk
but we'd only have 5 chunks instead of 20.

**Expected win**:
- Ratio: 5-10% (fewer Huffman tables, better tree fit)
- Speed: neutral or slightly worse

**Complexity**: Low (change threshold + chunk_size).

**Caveat**: Tested previously — 4 MiB chunks with lazy parser hurt
ratio badly. 4 MiB chunks with optimal_parse is slow per chunk but
fewer total chunks.

### 4. SIMD via `wide` crate (HIGH IMPACT)

**Problem**: match_length uses u64/u128 scalar comparisons. The C
reference uses SSE2/AVX2 SIMD intrinsics for 16-32 byte comparison.

**Fix**: Add `wide` crate dep to omnizip-codecs. Use `wide::u8x32`
for 32-byte comparison in match_length.

**Expected win**:
- Speed: 1.5-2x on match_length calls
- Overall: ~1.2x (match_length is ~50% of Q5 time)

**Complexity**: Low (1-2 hours). `wide` is already used in omnizip-flac.

### 5. Better hash function (LOW IMPACT)

**Problem**: Current 4-byte multiplicative hash has moderate collision
rate at hash_log=17 (128K buckets).

**Fix**: Use a 5-byte or 8-byte hash. Fewer collisions → fewer chain
walks → faster.

**Expected win**:
- Speed: 1.1-1.2x
- Ratio: neutral

**Complexity**: Low (30 min).

### 6. Skip dictionary lookup when hash match exists (LOW IMPACT)

**Problem**: Currently dict_hash::find_match is called at every position
WITHOUT a hash match. For highly repetitive data, hash matches dominate
and dict is rarely called. For less repetitive data, dict is called
more often but rarely finds a match.

**Fix**: Already implemented (dict only called when hash match is None).

**Expected win**: Already captured.

## Recommended priority

1. **Cross-chunk MF reuse** — biggest ratio win, moderate effort
2. **`wide` crate SIMD** — biggest speed win, low effort
3. **Skip incompressible sections** — big speed win on binary data
4. **4 MiB metablocks with optimal_parse** — ratio win, needs perf testing

## What the C reference does that we don't

| Feature | C Reference | Us | Impact |
|---------|-------------|-----|--------|
| SIMD match_length | SSE2/AVX2 | u128 fast-reject | 1.5x speed |
| PGO | profile-guided | no | 1.3x speed |
| Block-type switching | multi-table | disabled (decoder bug) | 2-5% ratio |
| Smart context clustering | adaptive | fixed split (decoder bug) | 3-8% ratio |
| Multi-probe hash | 4+8 byte | 4-byte only | 1.1x speed |
| Cross-chunk matching | full window | 1 MiB chunks | 10-30% ratio |
| Long-distance matching (LDM) | yes | no | 5-15% ratio on large input |

The cross-chunk matching gap is the biggest ratio difference. The SIMD
gap is the biggest speed difference.
