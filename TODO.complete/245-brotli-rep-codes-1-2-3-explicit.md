# 245 — Brotli Rep Codes 1/2/3 via Explicit Distance Codes

- **Status:** DONE (RepBuffer struct mirrors decoder ring buffer exactly;
  every LZ77 back-reference whose distance matches rep0/1/2/3 emits
  explicit distance code 0/1/2/3 with 0 extra bits)
- **Priority:** P2 (moderate ratio win on structured data)
- **Crate:** `omnizip-brotli`
- **Depends on:** none (builds on TODO 239 which added rep0 implicit)
- **Estimated effort:** 2 days

## Problem

The encoder currently uses only `rep0` via the implicit command
code path (kCmdLut entries with `distance_code == 0`). This saves
the entire distance Huffman code (~5 bits) when consecutive matches
share the same distance.

RFC 7932 §10.4 also defines explicit distance symbols 1, 2, 3 for
`rep1`, `rep2`, `rep3`. Each costs the distance Huffman code (~5
bits, same as any short symbol) but skips the distance extra bits
(0-24 bits, typically 5-15 bits for medium distances).

For CSV data with alternating column patterns (e.g., two columns
repeating with different distances), `rep1` could save 5-15 bits
per match versus the long-form distance code. Currently unused.

## Design

### Ring buffer state tracking

`build_symbol_stream` currently tracks only `rep0`. Extend it to
maintain the full 4-slot ring buffer matching the decoder's
`dist_rb: [u32; 4]` + `dist_rb_idx: i32` state.

```rust
struct RepBuffer {
    dist_rb: [u32; 4],
    idx: i32,
}

impl RepBuffer {
    fn initial() -> Self {
        // Matches decoder_full.rs:495 initialization.
        Self { dist_rb: [16, 15, 11, 4], idx: 0 }
    }

    fn rep_at(&self, code: u32) -> u32 {
        // code 0 → most recent (rep0)
        // code 1,2,3 → rep1,2,3
        let offset = (code as i32 - 3).rem_euclid(4);
        let idx = (self.idx - offset) & 3;
        self.dist_rb[idx as usize]
    }

    fn push(&mut self, distance: u32) {
        self.dist_rb[(self.idx & 3) as usize] = distance;
        self.idx = self.idx.wrapping_add(1);
    }
}
```

### Encoding decision tree

For each command with `copy_len > 0`:

1. If distance == rep0 AND (insert_len, copy_len) in implicit range
   AND prev wasn't implicit: use implicit command (saves all distance
   bits — same as today).
2. Else if distance == rep0: emit explicit distance code 0.
3. Else if distance == rep1: emit explicit distance code 1.
4. Else if distance == rep2: emit explicit distance code 2.
5. Else if distance == rep3: emit explicit distance code 3.
6. Else: emit long-form distance code (existing `encode_distance`).

### Distance symbol emission

`encode_distance` currently returns the long-form code. Add a new
helper that returns 0/1/2/3 for rep codes:

```rust
fn encode_distance_with_rep(
    distance: u32,
    cfg: &DistanceConfig,
    rep: &RepBuffer,
) -> (u32 /* sym */, u32 /* extra */) {
    for code in 0..4 {
        if distance == rep.rep_at(code) {
            return (code, 0);
        }
    }
    encode_distance(distance, cfg)
}
```

### Ring buffer update rules

Match the decoder (decoder_full.rs:620-636):

- Implicit command (distance_code == 0 in kCmdLut): `idx -= 1` (the
  decoder compensates after the dictionary check).
- Explicit short symbol (0-15 in distance alphabet): no ring buffer
  write; the decoder's `take_distance_from_ring_buffer` rotates idx
  internally.
- Explicit long symbol (16+): `push(distance)`.
- Dictionary reference (distance > output.len()): no ring buffer
  write; if implicit, `idx += 1` to undo the early decrement.

## Acceptance criteria

- [ ] `RepBuffer` matches decoder state exactly (verified by a unit
      test that runs the same command stream through both).
- [ ] CSV 100KB Q5 ratio improves by 0.5-2% (estimate based on the
      prevalence of alternating-distance patterns in CSV).
- [ ] All 86 brotli tests pass.
- [ ] No regression on binary or text benchmarks.

## Why this matters

For text inputs with column-aligned matches, distances often repeat
in patterns (col1_dist, col2_dist, col1_dist, ...). Currently only
col1_dist reuse is captured. This TODO captures the other half.
