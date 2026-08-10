# 241 — Two-Pass Backward Reference Collection

- **Status:** DONE (Pass 1 collects all matches; Pass 2 greedy with
  4-position lookahead; used as fallback for inputs > 1 MiB)
- **Priority:** P1 (the C reference's key advantage at quality 2-9)
- **Crate:** `omnizip-brotli`
- **Depends on:** [238](238-multi-probe-hash-matching.md) (superseded)
- **Estimated effort:** 5 days

## Problem

The from_spec encoder uses SINGLE-PASS lazy parsing: it decides
at each position whether to emit a match or literal, without
knowledge of future matches. The C reference uses TWO-PASS
backward reference collection:

1. **Pass 1 (collection)**: Walk all positions, find ALL viable
   matches, store them in a backward references array.
2. **Pass 2 (assignment)**: Walk the backward references and
   assign each position to either a match or a literal, optimizing
   for the cheapest overall encoding.

This two-pass approach finds better match combinations because it
can see ALL matches before committing to any.

## How it works in the C reference

```c
// Pass 1: collect
for each position pos:
    find best match at pos
    store in backward_refs[pos]

// Pass 2: assign
for each position pos:
    if backward_refs[pos].length >= MIN_MATCH:
        emit match command
        skip to pos + length
    else:
        emit literal
        advance to pos + 1
```

The key insight: in pass 2, the encoder can SKIP positions that
are covered by long matches, avoiding the per-position match-finding
cost. This is both faster AND finds better matches.

## Design

```rust
struct BackwardRefs {
    matches: Vec<Option<(u32, u32)>>,  // (distance, length) per position
}

fn collect_backward_refs(input: &[u8], mf: &mut HashChainMatchFinder) -> BackwardRefs {
    let mut refs = BackwardRefs { matches: vec![None; input.len()] };
    for pos in 0..input.len() {
        mf.advance();
        if let Some(m) = mf.find_match(pos) {
            refs.matches[pos] = Some((m.distance, m.length));
        }
    }
    refs
}

fn assign_commands(refs: &BackwardRefs) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut pos = 0;
    let mut insert_start = 0;
    while pos < refs.matches.len() {
        if let Some((dist, len)) = refs.matches[pos] {
            if len >= MIN_MATCH {
                let insert_len = (pos - insert_start) as u32;
                commands.push(Command { insert_len, copy_len: len, distance: dist });
                pos += len as usize;
                insert_start = pos;
                continue;
            }
        }
        pos += 1;
    }
    // Trailing literals
    if insert_start < refs.matches.len() {
        commands.push(Command { insert_len: (refs.matches.len() - insert_start) as u32, copy_len: 0, distance: 0 });
    }
    commands
}
```

## Impact

The two-pass approach is the C reference's key advantage at
quality 2-9. It finds longer matches and better match combinations
than single-pass lazy parsing. Expected CSV ratio improvement:
30-50% (from ~20% to ~10-14%).

## Acceptance criteria

- [ ] Backward reference collection implemented
- [ ] Two-pass command assignment implemented
- [ ] Used at quality 2-9 for text input
- [ ] CSV ratio improvement >= 30%
- [ ] No speed regression (pass 2 skips matched positions)
