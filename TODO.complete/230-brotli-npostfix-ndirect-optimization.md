# 230 — Brotli NPOSTFIX/NDIRECT Distance Code Optimization

- **Priority:** P2 (ratio win on inputs with clustered short distances)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 day

## Goal

Optimize the NPOSTFIX/NDIRECT distance code configuration per metablock.
The current `DistanceConfig::choose` uses a simple heuristic (check if
>=20% of distances are <=15). The C reference evaluates multiple
configurations and picks the best.

## Background

RFC 7932 §10.4 distance coding:
- NPOSTFIX: 0-3 (adds postfix bits to distance codes)
- NDIRECT: 0-120 (number of "direct" distance codes with zero extra bits)
- The combination affects the distance symbol alphabet size and encoding cost

Different NPOSTFIX/NDIRECT values favor different distance distributions:
- NPOSTFIX=0, NDIRECT=0: default, good for uniform distances
- NPOSTFIX=0, NDIRECT=16: good for many short distances (1-16 bytes back)
- NPOSTFIX=2, NDIRECT=0: good for distances with regular stride patterns

## Current state

- `DistanceConfig::choose` checks if >=20% of distances are <=15
- If yes: NPOSTFIX=0, NDIRECT=16 (NUM_SHORT direct codes)
- Otherwise: NPOSTFIX=0, NDIRECT=0

## Plan

1. For each metablock, compute distance frequency histogram
2. Evaluate top 3-4 NPOSTFIX/NDIRECT configurations by estimated cost
3. Pick the configuration with minimum estimated encoded size
4. The cost model: Shannon entropy over distance symbols + extra bits

## Acceptance criteria

- [ ] Multiple NPOSTFIX/NDIRECT configurations evaluated per metablock
- [ ] Best configuration selected by cost model
- [ ] Ratio improvement >= 0.5% on inputs with clustered distances
- [ ] No regression on inputs with uniform distance distributions
