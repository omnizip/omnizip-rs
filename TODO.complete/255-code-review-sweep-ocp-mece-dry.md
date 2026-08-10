# 255 — Code Review Sweep: OCP/MECE/DRY

- **Priority:** P2 (architectural quality)
- **Crate:** workspace-wide
- **Depends on:** [248](248-codec-profile-enum.md),
  [249](249-shared-huffman-module.md), [233](233-shared-match-finder-abstraction.md)
- **Estimated effort:** 3-5 days

## Problem

The codebase has grown organically across many TODO completions.
Patterns that started clean have accreted special cases. Examples
observed:

### OCP violations (modify-existing instead of add-new)

1. `parse_input_with_offset` has hardcoded `match quality` arms:
   ```rust
   match quality {
       0..=1 => (4, 8, false, false, false, 15),
       2..=3 => (16, 16, true, true, false, 16),
       ...
   }
   ```
   Adding a new quality level requires editing this match. Should
   be a table-driven config.

2. `encode_huffman_chunk_into` has many feature-flag booleans:
   `use_context`, `use_block_switch`, `use_dict`. Adding a new
   feature means another boolean + match arm. Should be a strategy
   pattern or pipeline.

3. `build_symbol_stream` has branches for `can_use_rep`, `is_dict`,
   `prev_was_implicit`. Encoding logic accreted as conditionals
   instead of as polymorphic command types.

### MECE violations (overlapping responsibilities)

1. `parse_input_with_offset` AND `optimal_parse` AND `two_pass_parse`
   all do match-finding + command-emission. Their responsibilities
   overlap; refactoring one might break the others.

2. `build_symbol_stream` (in from_spec_encoder.rs) AND
   `output_sim` loop (also in from_spec_encoder.rs) both walk
   commands to compute state. Two passes over the same data, with
   duplicated logic for dictionary lookup, advance computation, etc.

3. `find_cmd_symbol_impl` (linear search through kCmdLut) lives in
   `from_spec_encoder.rs` but is conceptually a property of
   `kCmdLut` itself. Should be a method on the table.

### DRY violations (copy-paste)

1. `is_text_like()` is duplicated in brotli AND should exist in
   omnizip-codecs (TODO 248 will move it).

2. `dictionary_lookup()` exists in BOTH `dictionary.rs` (encoder)
   AND `decoder_full.rs` (decoder). The two implementations differ
   subtly. Should be one shared function.

3. Hash-chain match-finding logic exists in:
   - `omnizip-brotli/src/from_spec_encoder.rs` (calls shared)
   - `omnizip-lzma/src/encoder/match_finder.rs` (calls shared)
   - `omnizip-zstd/src/encoder/match_finder.rs` (own impl, not
     using shared HashChainMatchFinder)
   - `omnizip-lz4/src/block.rs` (own impl)

4. Block-type context-mode handling is duplicated across encode
   and decode paths in brotli.

## Design

### Sweep process

For each module:

1. **Identify responsibilities.** What does this module do? List
   them all.
2. **Check MECE.** Are responsibilities overlapping with other
   modules? Move/separate to achieve mutual exclusivity.
3. **Check OCP.** For each `match` or `if-else` chain, can it be
   replaced with a strategy pattern? A registry? An enum dispatch?
4. **Check DRY.** Are similar patterns repeated? Extract to shared
   helper.

### Specific refactors (high-impact, low-risk)

#### A. Quality → Strategy table

Replace the `match quality` arm in `parse_input_with_offset`:

```rust
// Before
let (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log) = if is_text {
    match quality {
        0..=1 => (4, 8, false, false, false, 15),
        2..=3 => (16, 16, true, true, false, 16),
        ...
    }
};

// After
let config = ParserConfig::for_quality(quality, is_text);
let ParserConfig { max_chain, nice_match, use_dict, lazy, lazy2, hash_log } = config;
```

`ParserConfig::for_quality` is a `const` table lookup. Adding a
new quality level = adding a row to the table.

#### B. Encoder feature pipeline

Replace boolean flags with a pipeline:

```rust
struct EncoderPipeline {
    stages: Vec<Box<dyn EncoderStage>>,
}

trait EncoderStage {
    fn process(&mut self, ctx: &mut EncoderContext, input: &[u8]);
}

// Stages: DictionaryLookup, ContextModeling, BlockSwitching, ...
```

Adding a new stage = adding a new struct + push to the pipeline.
Existing stages unchanged.

#### C. Shared dictionary_lookup

Move `dictionary_lookup` from encoder/decoder files into
`dictionary.rs` as a single function. Both halves call it.

#### D. Command types instead of branching on copy_len

Instead of `Command { insert_len, copy_len, distance }` + many
`if copy_len > 0` branches, use an enum:

```rust
enum Command {
    InsertOnly { len: u32 },
    InsertAndCopy { insert_len: u32, copy_len: u32, distance: u32 },
    DictReference { insert_len: u32, word_idx: u16, transform: u8 },
    RepCode { insert_len: u32, copy_len: u32, code: u8 },
}
```

Each variant carries its own data. Pattern-match handles dispatch.
No more "if copy_len > 0" everywhere.

## Acceptance criteria

- [ ] Sweep document committed listing all findings.
- [ ] Top 5 highest-impact refactors landed as separate PRs.
- [ ] No new `match quality` arms added (test enforces this via
      grep in CI).
- [ ] No new boolean feature flags in encode path.
- [ ] Workspace clippy warnings reduced by 50%+.

## Why this matters

OCP/MECE/DRY aren't academic — they're predictive of how fast we
can ship features. Each violation is a place where adding a new
feature requires understanding coupled code. Fixing them pays
dividends on every future PR.
