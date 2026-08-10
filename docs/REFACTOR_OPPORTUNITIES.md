# Refactor Opportunities (Live Document)

This document captures architectural improvements identified during
session work. Items here are NOT blocking and NOT bugs — they're
opportunities to make the codebase cleaner. Each entry has a one-line
summary, current state, and suggested refactor.

## High-value refactors

### 1. `ParserConfig` for quality → parameters mapping

**Current**: `parse_input_with_offset` has a `match quality { ... }`
table inside the function body. Adding a new quality level requires
modifying this arm.

**Refactor**: Extract a `const` table:

```rust
const PARSER_CONFIGS: [(u8, u8, u32, u32, bool, bool, bool, u32); 12] = [
    // (qmin, qmax, max_chain, nice_match, use_dict, lazy, lazy2, hash_log)
    (0, 1, 4, 8, false, false, false, 15),
    (2, 3, 16, 16, true, true, false, 16),
    // ...
];
```

**Benefit**: Adding levels = adding a row.

### 2. `Command` enum replacing struct + branches

**Current**: `Command { insert_len, copy_len, distance }` with many
`if cmd.copy_len > 0` branches. Distance encodes 3 different things:
- LZ77 back-reference
- Dictionary reference (distance > output.len())
- Zero (insert-only command)

**Refactor**:
```rust
enum Command {
    InsertOnly { len: u32 },
    InsertAndCopy { insert_len: u32, copy_len: u32, distance: u32 },
    DictReference { insert_len: u32, word_idx: u16, transform: u8, copy_len: u32 },
    RepCode { insert_len: u32, copy_len: u32, code: u8 },
}
```

**Benefit**: Each variant carries only its relevant data. Pattern
matching handles dispatch. No more `if copy_len > 0` everywhere.

### 3. Single shared `dictionary_lookup` for encoder + decoder

**Current**: `dictionary_lookup` exists in BOTH `dictionary.rs`
(encoder) and `decoder_full.rs` (decoder). The two diverged in edge
cases.

**Refactor**: Single function in `dictionary.rs`. Both halves import
it.

**Benefit**: 1 implementation instead of 2. Bugs fixed once.

### 4. `EncoderStage` pipeline replacing boolean flags

**Current**: `encode_huffman_chunk_into` has `use_context`,
`use_block_switch`, `use_dict` booleans. Adding a feature means
another boolean + branches.

**Refactor**: Strategy pattern:

```rust
trait EncoderStage {
    fn process(&mut self, ctx: &mut EncoderContext, input: &[u8]);
}

struct EncoderPipeline { stages: Vec<Box<dyn EncoderStage>> }
```

**Benefit**: New feature = new stage struct. No flag changes.

### 5. `kCmdLut::find_symbol()` replacing linear scan

**Current**: `find_cmd_symbol_impl` does O(704) linear scan per
command.

**Refactor**: Build a hash table at construction time:

```rust
impl CmdLutElement {
    pub fn find_symbol(insert_len: u32, copy_len: u32) -> usize { ... }
}
```

**Benefit**: O(1) lookup. ~10% speedup on encode paths with many
commands.

## Medium-value refactors

### 6. Parser MECE: three parser functions share match-finding

**Current**: `parse_input_with_offset`, `optimal_parse`,
`two_pass_parse` each have their own match-finding loop.

**Refactor**: Extract `MatchCollector` trait:

```rust
trait MatchCollector {
    fn consider(&mut self, pos: usize, dist: u32, len: u32);
    fn finalize(self) -> Vec<Command>;
}
```

Each parser is a `MatchCollector` impl.

### 7. Merge `build_symbol_stream` and `output_sim` loop

**Current**: Both walk commands; literal extraction is duplicated.

**Refactor**: Single pass that builds both streams simultaneously.

### 8. Migrate brotli BitWriter to shared omnizip-codecs BitWriterBE

**Current**: Brotli has its own BitWriter. The shared BitWriterBE in
omnizip-codecs exists but isn't used.

**Refactor**: Replace brotli's BitWriter with shared.

**Benefit**: Removes ~200 LOC. SIMD improvements to shared benefit
brotli automatically.

## Low-value refactors (skip unless touching nearby code)

### 9. Per-codec clippy allows vs workspace-level

Some crates have `#![allow(clippy::cast_possible_truncation)]` at the
crate level. Could move to function-level allows where the cast is
actually safe.

### 10. `CompressionLevel` newtype vs raw u8

`CompressionLevel(u8)` is a newtype. Some internal functions take raw
`u8` instead. Could enforce newtype everywhere.

## Architecture-level insights

- The `Codec` trait has grown to 8+ methods (`compress`, `decompress`,
  `compress_with_profile`, `compress_with_options`, `capabilities`,
  `default_fast_level`, etc.). Consider splitting into multiple traits
  (`Codec`, `ProfileAware`, `Streaming`, `ParallelBatch`) for
  cleaner separation.
- The `CodecRegistry` is a `Vec<Box<dyn Codec>>`. For 15+ codecs,
  consider a `BTreeMap<CodecId, Box<dyn Codec>>` for O(log N) lookup.
- The shared bitstream module has both BE and LE readers/writers.
  These could be unified behind a `BitOrder` trait for ergonomics.

## How to use this document

When picking up refactoring work:
1. Match an existing observation here.
2. Write a focused PR with the refactor.
3. Mark the entry as landed.
4. Update CLAUDE.md / ADRs if the refactor changes architectural
   decisions.
