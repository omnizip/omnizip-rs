# 206 — Brotli Encoder Architecture Refactor

- **Priority:** P3 (code quality, enables future work)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 week

## Goal

Refactor the monolithic `from_spec_encoder.rs` (1300+ LOC) into a
modular architecture following OCP, MECE, DRY, and model-driven
principles.

## Current problems

1. **Violates OCP**: adding a parsing strategy requires editing
   `parse_input_with_offset`.
2. **Violates MECE**: match-finding, parsing, entropy-coding, and
   framing concerns are interleaved in one function.
3. **Violates DRY**: quality→config mapping is inline; distance
   encoding formulas repeated across functions.
4. **No model-driven design**: `Command` is a bare struct with no
   behavior; no `ParsingStrategy` abstraction.

## Proposed architecture

```
omnizip-brotli/src/
  lib.rs                  — Codec trait impl, public API (thin)
  from_spec_encoder.rs    — compress() entry point (thin)
  encoder/
    mod.rs                — Encoder orchestration
    config.rs             — Quality → EncoderConfig mapping
    parser/
      mod.rs              — ParsingStrategy trait
      greedy.rs           — Greedy parser
      lazy.rs             — Lazy parser
      optimal.rs          — Optimal DP parser (TODO 201)
    command.rs            — Command model with behavior
    symbol_stream.rs      — Command → symbol stream conversion
    entropy/
      mod.rs              — EntropyCoder trait
      huffman.rs          — Huffman table building + writing
      rle.rs              — RLE code-length encoding
    metablock.rs          — Metablock framing (header, MLEN, etc.)
    context.rs            — Context modeling (TODO 200)
    bitwriter.rs          — LSB-first bit writer (extracted)
  dictionary.rs           — Static dictionary + transforms
  decoder.rs              — Decoder (trivial path)
  decoder_full.rs         — Decoder (full RFC path)
  prefix.rs               — Static prefix codes
  static_codes.rs         — kCmdLut, kBlockLengthPrefixCode
```

## Scope

1. **Extract BitWriter** (1 day): move to `encoder/bitwriter.rs`
2. **Extract config** (1 day): quality→config mapping to `config.rs`
3. **Parsing strategy trait** (2 days): `ParsingStrategy` trait with
   `parse() -> Vec<Command>`. Existing greedy/lazy become
   implementations.
4. **Entropy module** (2 days): extract Huffman table writing to
   `entropy/huffman.rs`
5. **Metablock module** (1 day): extract framing to `metablock.rs`

## Acceptance criteria

- [ ] No file exceeds 400 LOC
- [ ] Each module has a single responsibility (MECE)
- [ ] Adding a parsing strategy requires only a new file (OCP)
- [ ] All existing tests pass without modification
- [ ] No performance regression (benchmark before/after)
- [ ] No `unsafe` code introduced

## Implementation plan

Each extraction is a pure refactor (no behavior change). Do them
one at a time, running the full test suite after each.

### Step 1: Extract BitWriter

Move `struct BitWriter` and its impl to `encoder/bitwriter.rs`.
Re-export from `from_spec_encoder.rs` for backward compatibility.

### Step 2: Extract config

Move the quality→config `match` expression to `config.rs`:

```rust
pub struct EncoderConfig {
    pub max_chain: u32,
    pub nice_match: u32,
    pub use_dict: bool,
    pub lazy: bool,
    pub strategy: ParsingStrategy,
}

impl EncoderConfig {
    pub fn from_quality(q: i32) -> Self { ... }
}
```

### Step 3: Parsing strategy trait

```rust
pub trait ParsingStrategy {
    fn parse(&mut self, input: &[u8], mlen_offset: usize) -> Vec<Command>;
}

pub struct GreedyParser<'a> { mf: &'a mut HashChainMatchFinder<'a>, config: EncoderConfig }
pub struct LazyParser<'a> { mf: &'a mut HashChainMatchFinder<'a>, config: EncoderConfig }
```

### Step 4: Entropy module

Move `write_huffman_table`, `write_simple_one_symbol`,
`build_rle_sequence`, `canonical_with_reverse` to
`entropy/huffman.rs`.

### Step 5: Metablock module

Move metablock header writing (ISLAST, MNIBBLES, MLEN, NBLTYPES,
NPOSTFIX, etc.) to `metablock.rs`.

## Test plan

- All 80 existing tests pass without modification
- Benchmark: encode speed unchanged (±5%)
- Clippy clean on all new modules
- Doc comments on all public items

## References

- SOLID principles, especially OCP
- Upstream `brotli/c/enc/` module structure for comparison
