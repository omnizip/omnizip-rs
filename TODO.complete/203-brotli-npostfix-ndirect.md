# 203 — Brotli NPOSTFIX/NDIRECT Distance Tuning

- **Priority:** P1 (3% ratio win, low complexity)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 2 days

## Goal

Enable NPOSTFIX and NDMOEM (NDIRECT) configuration in the metablock
header. Currently hardcoded to NPOSTFIX=0, NDMOEM=0, which forces all
non-short distances through long-form encoding. Adding direct distance
codes gives cheaper encoding for short distances.

## Background

RFC 7932 §9.4 defines the distance code alphabet:

```
alphabet_size = NUM_SHORT(16) + NDIRECT + (48 << NPOSTFIX)
NDIRECT = NDMOEM << NPOSTFIX
```

- **NUM_SHORT (16)**: ring-buffer distance codes (always present)
- **NDIRECT**: direct distance codes for distances 1..NDIRECT (no extra
  bits — cheaper than long-form)
- **48 << NPOSTFIX**: long-form distance codes with postfix bits for
  finer granularity

Current NPOSTFIX=0, NDMOEM=0: NDIRECT=0, all distances use long-form.
This wastes bits on short distances.

Optimal settings depend on input:
- Small inputs: NPOSTFIX=0, NDMOEM=0 (few distances)
- Medium text: NPOSTFIX=0, NDMOEM=8–16 (many short-distance matches)
- Large files: NPOSTFIX=1, NDMOEM=8 (fine-grained long distances)

## Scope

1. **Parameter selection** (1 day): heuristic to choose NPOSTFIX and
   NDMOEM based on input size and distance distribution.

2. **Distance encoding update** (1 day): modify `encode_distance` to
   handle direct codes and postfix bits.

## Acceptance criteria

- [ ] NPOSTFIX and NDMOEM are no longer hardcoded to 0
- [ ] Distance encoding uses direct codes for distances ≤ NDIRECT
- [ ] Round-trip correctness preserved
- [ ] Ratio improvement ≥ 2% on inputs with many short-distance matches
- [ ] `brotli -d` accepts output

## Implementation plan

### Modified: `encode_huffman_chunk_into`

```rust
let (npostfix, ndmoem) = choose_distance_params(input.len(), &commands);
bw.write_bits(npostfix as u32, 2);   // NPOSTFIX
bw.write_bits(ndmoem as u32, 4);     // NDMOEM
let ndirect = (ndmoem << npostfix) as usize;
```

### New function: `choose_distance_params`

```rust
fn choose_distance_params(input_len: usize, commands: &[Command]) -> (usize, usize) {
    // Count distance distribution
    let mut short_dists = 0;  // distances 1..16
    let mut med_dists = 0;    // distances 17..256
    for cmd in commands {
        if cmd.copy_len > 0 {
            if cmd.distance <= 16 { short_dists += 1; }
            else if cmd.distance <= 256 { med_dists += 1; }
        }
    }

    // Heuristic: if many medium-distance matches, add direct codes
    if med_dists > input_len / 100 {
        (0, 12)  // NPOSTFIX=0, NDIRECT=12 << 0 = 12
    } else if input_len > 100_000 {
        (1, 8)   // NPOSTFIX=1 for finer long distances
    } else {
        (0, 0)   // Default: no direct codes
    }
}
```

### Modified: `encode_distance`

```rust
fn encode_distance(distance: u32, ndirect: u32, npostfix: u32) -> (u32, u32) {
    // Short codes 0–15: ring buffer (handled by decoder, not emitted here)
    // Direct codes 16..16+NDIRECT-1: distance = code - 15
    if distance <= ndirect {
        return (16 + distance - 1, 0);  // symbol, no extra bits
    }
    // Long codes: symbol ≥ 16 + NDIRECT
    // ... existing long-form logic with postfix bits ...
}
```

## Test plan

- Unit test: distance round-trip with NPOSTFIX=0, NDMOEM=12
- Unit test: distance round-trip with NPOSTFIX=1, NDMOEM=8
- Integration: ratio improvement on text with many short matches
- Integration: `brotli -d` accepts output

## References

- RFC 7932 §9.4 (NPOSTFIX, NDMOEM), §10.4 (distance decoding)
- Upstream: `brotli/c/enc/encode.c:ChooseDistanceParams`
- Our decoder: `decoder.rs:decode_distance_from_code` (already handles
  NPOSTFIX/NDIRECT)
