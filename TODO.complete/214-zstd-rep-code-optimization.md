# 214 — ZSTD Rep-Code Offset Optimization

- **Priority:** P3 (2% ratio win, well-defined)
- **Crate:** `omnizip-zstd`
- **Depends on:** none
- **Estimated effort:** 3 days

## Goal

Improve rep-code (repeat offset) usage in the sequence encoder. The
C reference uses 3 rep-codes (recent distances) to encode frequently
repeating patterns more cheaply. Our encoder supports rep-codes but
doesn't optimally select when to use them.

## Background

ZSTD uses a 3-entry ring buffer of recent match distances:
```
rep[0], rep[1], rep[2]
```

When a new match has the same distance as one of these, it can be
encoded as a rep-code (0 bits for the distance symbol) instead of a
full offset code.

Offset symbols 0–2 in the FSE table correspond to rep-codes:
- Symbol 0: use rep[0] (and shift the ring)
- Symbol 1: use rep[1] (and promote to rep[0])
- Symbol 2: use rep[2] (and promote to rep[0])
- Symbol 3: rep[0] - 1 (adjusted repeat)
- Symbol 4+: explicit distances

Current state: rep-codes are supported in the decoder but the encoder
uses explicit distances for most matches.

## Scope

1. **Rep-code preference** (2 days): when a match distance equals one
   of the 3 recent distances, use the rep-code instead of an explicit
   offset.

2. **Rep-code update** (1 day): properly update the ring buffer when
   rep-codes are used (shift semantics differ from explicit offsets).

## Acceptance criteria

- [ ] Matches with repeat distances use rep-codes
- [ ] Ratio improvement ≥ 1% on inputs with repeated patterns
- [ ] `zstd -d` accepts output
- [ ] No round-trip regression

## Implementation plan

### Modified: `encoder/match_finder.rs:compress_block_lazy*`

When a match is found, check if its distance matches any rep-code:

```rust
let rep_idx = if distance == rep_offsets[0] {
    Some(0)
} else if distance == rep_offsets[1] {
    Some(1)
} else if distance == rep_offsets[2] {
    Some(2)
} else {
    None
};

if let Some(idx) = rep_idx {
    // Emit rep-code offset symbol
    sequences.push(Sequence {
        literal_length, match_length,
        offset: idx as u32,  // rep-code symbol
    });
    // Update ring buffer
    update_rep_offsets(&mut rep_offsets, idx);
} else {
    // Emit explicit offset
    sequences.push(Sequence {
        literal_length, match_length,
        offset: distance + 3,  // shift for rep-code symbols
    });
    // Promote to rep[0]
    rep_offsets[2] = rep_offsets[1];
    rep_offsets[1] = rep_offsets[0];
    rep_offsets[0] = distance;
}
```

## Test plan

- Unit test: rep-code ring buffer updates correctly
- Unit test: repeated-distance matches use rep-codes
- Integration: ratio improvement on structured data (CSV, JSON)
- Integration: `zstd -d` accepts output

## References

- RFC 8478 §3.1.1.3.2 (repeat offsets)
- C reference: `zstd/compress/zstd_compress_internal.h:repcode`
- Our encoder: `encoder/match_finder.rs` (has rep_offsets tracking)
