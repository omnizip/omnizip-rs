# TODO 167: ricepp remainder — unary emission + Rice code selection

## Problem

TODO 113 closed half the ricepp 6× gap (SIMD delta+zigzag+sum).
Still ~3× slower than DwarFS C++ ricepp.

Remaining gap is the **unary-bit emission** and **Rice code
selection** loops:

```rust
for &d in delta.iter() {
    let top = d >> fs;
    if top > 0 {
        writer.write_bit_repeated(false, top as u32);  // unary zeros
    }
    writer.write_bits(1, 1);  // unary terminator
    writer.write_bits(d, fs); // low bits
}
```

Three issues:
1. Each pixel makes 2-3 method calls into the bit writer.
2. Unary zeros are written one pixel at a time instead of batched.
3. The `compute_best_split` cost-evaluation is O(N) per fs candidate.

## Scope

1. **Batch unary emission**: precompute total unary zeros per block,
   emit in one `write_bit_repeated` call.
2. **Inline bit writer**: hot loop should write directly to a u64
   accumulator, not through method calls.
3. **Vectorise Rice code selection**: use SIMD to evaluate all fs
   candidates simultaneously.

## Implementation plan

1. Profile to confirm the bottleneck is unary emission (not bit
   packing).
2. Pre-compute per-pixel unary counts and low bits.
3. Single-pass emit: total unary + terminator + low-bits in one go.

## Acceptance criteria

- [ ] Bench shows ≥ 2× additional speedup vs current SIMD path.
- [ ] Output stays byte-identical.
- [ ] Approaches DwarFS C++ ricepp throughput.

## Priority

P2 — LimniFS explicitly said "not blocking us; just means ricepp
path on fits-synthetic is slower than it could be".
