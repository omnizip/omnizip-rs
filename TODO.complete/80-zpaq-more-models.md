# 80 — ZPAQ: add more context-mixing sub-models

**Priority:** Medium
**Source:** RESEARCH.md §2 (cmix / PAQ lineage)

## Context

`omnizip-zpaq` currently uses 4 sub-models in its MultiModel:
- Order-0 (uniform byte frequency)
- Order-1 (previous byte context)
- Order-2 (two previous bytes context)
- Match model (longest-match prediction)

cmix (SOTA on Hutter Prize) uses 1000+ sub-models. Adding more
should improve ratio significantly on natural-language input.

## Proposed additions

1. **Order-3 model** — three-byte context. Diminishing returns beyond
   order 3 for byte-level prediction, but worth measuring.
2. **Word-level model** — treat ASCII alphanumeric runs as tokens,
   predict next token from previous 1-2 tokens. Big win on English text.
3. **Hash-context model** — sliding 8-byte hash → predict byte. Cheap
   collision-resistant alternative to order-8 contexts.
4. **Run-length model** — if last N bytes were identical, predict the
   same byte with high probability. Big win on RLE-friendly inputs.
5. **Bit-level models** — bit-position-context predictions (8 bit-models
   per byte). Already used in PPMd7; worth porting to ZPAQ.

## Architecture

Following OCP, each new model should be a separate struct implementing
a `Model` trait, registered in `MultiModel::new()`. No edits to the
mixer or encoder loop needed.

```rust
// Proposed trait (extract from MultiModel's current implicit API):
pub trait ZpaqModel: Send + Sync {
    fn predict(&self, ctx: &ModelContext) -> u16;  // probability 0..65536
    fn update(&mut self, byte: u8, ctx: &ModelContext);
}
```

Then `MultiModel` holds `Vec<Box<dyn ZpaqModel>>` and the mixer
adapts to whatever subset is active.

## Acceptance criteria

- [ ] `Model` trait extracted; existing 4 models converted to impls.
- [ ] At least 3 new models added (order-3, word, run-length).
- [ ] Ratio on Enwik8 improves by ≥5% vs current ZPAQ.
- [ ] Memory bounded (each model declares its budget).
- [ ] Determinism preserved (snapshot test).
- [ ] Workspace tests pass.

## Files

- `omnizip-zpaq/src/model.rs` — extract trait, convert existing models
- `omnizip-zpaq/src/models/` — new module, one file per model
- `omnizip-zpaq/src/lib.rs` — re-export `Model` trait
