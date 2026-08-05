# TODO 113: ricepp SIMD via wide

## Problem

`omnizip-ricepp/src/lib.rs::encode_block` runs a serial delta +
zigzag + sum loop per pixel:

```rust
for (i, &pixel) in block.iter().enumerate() {
    let diff = pixel.wrapping_sub(last);
    let d = zigzag_encode(diff, pixel_msb, pixel_bits);
    delta[i] = d;
    sum += d;
    last = pixel;
}
```

The dependency on `last` (sequential prefix) makes naive SIMD hard,
but the per-pixel inner work — `wrapping_sub`, `zigzag_encode`, sum
reduction — vectorises cleanly.

With block size = 16 (default) the per-block overhead also dominates:
function-call cost × N pixels × 32 K blocks/MiB = significant.

## Proposed fix

Two complementary changes:

### 1. Per-block SIMD inner loop

Vectorise the zigzag + sum accumulation using `wide::u64x4`. The
delta is still sequential (carry from one chunk to the next), but the
zigzag and sum reduction process 4 pixels per cycle.

```rust
let mut acc = u64x4::splat(0);
let mut last_v = u64x4::splat(last);
for chunk in delta.chunks_exact(4) {
    let pixels = u64x4::from_array(chunk);
    let diff = pixels.wrapping_sub(last_v);
    let zz = zigzag_encode_v(diff, msb_v);
    delta_v[i] = zz;
    acc += zz;
    last_v = pixels;  // shift-splat for next iteration
}
let sum = acc.element_sum();
```

### 2. Larger block size

Lift `DEFAULT_BLOCK_SIZE` from 16 to 64 or 128. Each block pays a
4-bit `fs` field overhead; bigger blocks amortise it.

Caveat: changing the block size affects wire format and ratio
slightly. Must be opt-in via `CodecConfig`.

## Acceptance criteria

- [ ] Bit-exact output vs scalar on 8/16/32-bit pixel inputs.
- [ ] SIMD path active under `simd-delta` cargo feature.
- [ ] Bench: 2-3× speed-up on 1 MiB of 16-bit pixel data.
- [ ] Configurable block size with `block_size: 64` and `128` options.

## Priority

P0 — closes half of the 6× ricepp gap.
