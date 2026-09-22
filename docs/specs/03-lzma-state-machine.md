# 03 — LZMA state machine

The LZMA state machine tracks recent encoding history to select the correct
probability context for the next decision. It has 12 states (0–11),
organised by how recently a match or literal was emitted and whether the
last match was a rep-match (repeated distance).

## State diagram

```text
                    ┌─────────────────────────────────────┐
                    │           Literal path               │
                    │  (MaxStateLiteral = 7)               │
                    v                                     │
 ┌──────┐  lit   ┌──────┐  lit   ┌──────┐  lit   ┌──────┐ │
 │  S0  │ ─────► │  S1  │ ─────► │  S2  │ ─────► │  S3  │ │
 └──┬───┘        └──┬───┘        └──┬───┘        └──┬───┘ │
    │ match         │ match         │ match         │     │
    │               │               │               │     │
    ▼               ▼               ▼               ▼     │
 ┌──┬──┐         ┌──┬──┐         ┌──┬──┐         ┌──┬──┐  │
 │S4│S5│ lit     │S4│S5│ lit     │S4│S5│         │S6│S7│──┘
 └──┴──┘◄─────── └──┴──┘◄─────── └──┴──┘         └──┴──┘
    ▲  ▲            ▲  ▲            ▲  ▲            ▲  ▲
    │  │ rep0=1     │  │            │  │            │  │
    │  │ (short)    │  │            │  │            │  │
    │  └────────────┘  │            │  │            │  │
    │     match        │            │  │            │  │
    └──────────────────┘────────────┘──┘────────────┘──┘

 Rep states: S8–S11 (entered after a rep-match)
 ┌──────┐  lit   ┌──────┐  lit   ┌──────┐
 │  S8  │ ─────► │  S9  │ ─────► │ S10  │ ──lit──► S11(lit) ──lit──► S4
 └──┬───┘        └──┬───┘        └──┬───┘
    │ match         │ match         │ match
    └───────────────┴───────────────┘
              (stays in rep states)
```

## State transitions

| State | After literal | After match | After rep |
|---|---|---|---|
| 0 | 0 | 7 | 8 |
| 1 | 0 | 7 | 8 |
| 2 | 0 | 7 | 8 |
| 3 | 0 | 7 | 8 |
| 4 | 1 | 8 | 8 |
| 5 | 2 | 8 | 8 |
| 6 | 3 | 8 | 8 |
| 7 | 3 | 8 | 8 |
| 8 | 9 | 11 | 11 |
| 9 | 9 | 11 | 11 |
| 10 | 9 | 11 | 11 |
| 11 | 11 | 11 | 11 |

## How the state is used

The state selects different probability arrays:

- **State < 7**: the encoder is "fresh" (recent literals, not deep in a
  match sequence). Uses `IsMatch[state][pos_state]` to decide
  literal vs match.
- **State >= 7**: a match was recently emitted. The next literal might
  be a "matched literal" (the byte predicted by the last match
  distance), which has a special fast path.
- **State >= 8**: a rep-match was the last operation. `IsRep[state]`
  decides whether the next match is a rep or a new distance.
- **State >= 11**: at least 2 consecutive rep-matches. `IsRep0Long[state]`
  decides whether the rep-match has length ≥ 2 (a full match) or
  length 1 (effectively a literal at the rep distance).

## Rust representation

```rust
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
struct LzmaState(u8);

impl LzmaState {
    const NUM_STATES: usize = 12;

    fn on_literal(&mut self) {
        self.0 = match self.0 {
            0..=3 => 0,
            4 => 1,
            5 => 2,
            6..=7 => 3,
            8..=10 => 9,
            _ => 11,
        };
    }

    fn on_match(&mut self) {
        self.0 = if self.0 < 7 { 7 } else { 11 };
    }

    fn on_rep(&mut self) {
        self.0 = if self.0 < 7 { 8 } else { 11 };
    }

    fn is_literal_context(&self) -> bool {
        self.0 < 7
    }

    fn is_rep_context(&self) -> bool {
        self.0 >= 7
    }
}
```

## Why 12 states?

Empirically tuned by the 7-Zip authors. Fewer states lose ratio because
the probability model can't distinguish between contexts; more states
waste memory and slow adaptation without measurable ratio gain. The 12-
state partition is the sweet spot found by Igor Pavlov.

## Cross-references

- Ruby: `omnizip/lib/omnizip/algorithms/lzma/state.rb`
- C: `xz/src/liblzma/lzma/lzma_decoder.c` (inline state transitions)
- Spec: `lzma-specification.txt` §3
- Rust port: `omnizip-lzma/src/state.rs` (pending)
