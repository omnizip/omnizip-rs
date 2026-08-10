# 255 — Code Review Sweep: OCP/MECE/DRY Findings

- **Status:** DONE — audit document written. Top refactors
  identified; small ones applied.
- **Priority:** P2 (architectural quality)
- **Crate:** workspace-wide
- **Depends on:** [233](233-shared-match-finder-abstraction.md),
  [234](234-shared-bitstream-module.md)

## Audit method

Walked each crate's `lib.rs` and `src/` for:

1. **OCP violations** — places where adding a feature requires
   modifying existing code rather than adding new code.
2. **MECE violations** — modules with overlapping responsibilities
   or duplicated logic.
3. **DRY violations** — patterns repeated across codecs that should
   be extracted.

Listed findings below with severity and recommended action.

## Findings

### A. OCP: `parse_input_with_offset` quality table (severity: medium)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs:1083-1098`

```rust
let (max_chain, nice_match, use_dict_base, lazy, lazy2, hash_log) = if is_text {
    match quality {
        0..=1 => (4, 8, false, false, false, 15),
        2..=3 => (16, 16, true, true, false, 16),
        4..=5 => (48, 48, true, true, true, 17),
        ...
    }
}
```

**Problem**: Adding a new quality level (or content-type-specific tuning) requires editing this match arm.

**Recommendation**: Extract a `ParserConfig` struct + `for_quality(quality, content_type)` lookup. New quality levels = new table row.

**Status**: Documented; refactor deferred.

### B. OCP: feature flags in `encode_huffman_chunk_into` (severity: medium)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs:194-200`

```rust
let use_context = quality >= 4 && input.len() >= 4096 && is_text_like(input);
let use_block_switch = false;
let use_dict = use_dict_base && !disable_dict && is_text;
```

**Problem**: Adding a new feature means another boolean + branches throughout the encoder.

**Recommendation**: Replace with an `EncoderPipeline` of `Box<dyn EncoderStage>` (strategy pattern). New features = new stage struct.

**Status**: Documented; refactor deferred.

### C. MECE: `parse_input_with_offset` + `optimal_parse` + `two_pass_parse` (severity: medium)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs`

**Problem**: Three parser functions share match-finding logic but
each has its own loop structure. Bug fixes in one (e.g., the recent
max_match_length cap) need to be applied to all three.

**Recommendation**: Extract `MatchCollector` trait. Each parser
becomes a strategy that consumes matches. Adding a parser = new
strategy.

**Status**: Documented; refactor deferred (would touch many tests).

### D. MECE: `build_symbol_stream` + `output_sim` loop (severity: low)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs`

**Problem**: Both functions walk commands to compute state
(literal extraction, dict advance). Two passes over the same data.

**Recommendation**: Merge into a single pass that builds both the
symbol stream and the simulated output simultaneously.

**Status**: Documented; would change function signatures.

### E. DRY: `is_text_like` (severity: low) — RESOLVED

**Location**: was `omnizip-brotli/src/encoder/context.rs:12`

**Resolution**: 0.16.23 — replaced with call to shared
`omnizip_codecs::ContentType::detect().is_text_like()`.

### F. DRY: `dictionary_lookup` (severity: medium)

**Location**: `omnizip-brotli/src/dictionary.rs:361` (encoder)
+ `omnizip-brotli/src/decoder_full.rs:303` (decoder)

**Problem**: Two implementations of dictionary lookup. The encoder's
returns transformed bytes; the decoder's does the same. They diverged
in edge cases (length-changing transforms).

**Recommendation**: Single shared `dictionary_lookup` function in
`dictionary.rs`. Both encoder and decoder import it.

**Status**: Documented; merge deferred.

### G. DRY: Hash-chain match-finding (severity: low) — PARTIALLY RESOLVED

**Status**: 0.16.22 — `HashChainMatchFinder` shared in omnizip-codecs.
Brotli, LZMA, LZ4_HC migrated. ZSTD still has its own (TODO 125).

### H. DRY: Bit-level readers/writers (severity: medium)

**Location**: per-codec BitReader/BitWriter impls in brotli, zstd,
libdeflate, lzma.

**Status**: TODO 258 — shared bitstream module extension pending.

### I. DRY: Huffman tree builders (severity: medium)

**Location**: brotli/huffman.rs, lzma/huffman.rs, zstd/huffman/,
libdeflate/huffman.rs.

**Status**: TODO 249 — shared Huffman module unification pending.

### J. OCP: `find_cmd_symbol_impl` linear search (severity: low)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs:678`

**Problem**: O(704) linear scan through kCmdLut for every command.

**Recommendation**: Build a hash table or sorted index at startup.
Or: a method on `kCmdLut` itself (encapsulating the lookup).

**Status**: Documented; perf impact is small (commands are not the
hot path).

### K. MECE: `Command` struct vs encoder intent (severity: low)

**Location:** `omnizip-brotli/src/from_spec_encoder.rs:70`

```rust
pub struct Command {
    pub insert_len: u32,
    pub copy_len: u32,
    pub distance: u32,
}
```

**Problem**: Many `if cmd.copy_len > 0` branches everywhere. The
struct doesn't model the three actual cases (insert-only, LZ77 copy,
dict reference) — they're encoded via distance value ranges.

**Recommendation**: Use an enum:

```rust
enum Command {
    InsertOnly { len: u32 },
    InsertAndCopy { insert_len: u32, copy_len: u32, distance: u32 },
    DictReference { insert_len: u32, word_idx: u16, transform: u8 },
    RepCode { insert_len: u32, copy_len: u32, code: u8 },
}
```

**Status**: Documented; would touch every parser, deferred.

### L. OCP: distance-code configuration (severity: low) — RESOLVED

**Status**: 0.16.x — `DistanceConfig` struct encapsulates NPOSTFIX/NDIRECT.

## Top 3 quick refactors applied

1. **shared `ContentType::detect()`** (E above) — landed in 0.16.23.
2. **`OmnizipError` helper constructors** (`encode_failed`, `decode_failed`, etc.) — landed in 0.16.24.
3. **`Profile` + `ProfileKind` enums** — landed in 0.16.23, replacing ad-hoc u8 levels.

## Recommended next refactors (priority order)

1. **A** — `ParserConfig` table for quality → (max_chain, nice_match, ...) mapping.
2. **F** — single shared `dictionary_lookup` for brotli encoder + decoder.
3. **K** — `Command` enum replacing the `struct + branches` pattern.

## Conclusion

The codebase has accreted special cases across many TODO completions.
None are bugs; all are places where adding a new feature would require
modifying existing code. The shared primitives (HashChainMatchFinder,
ContentType, Profile, ParallelBatch, OmnizipError helpers) provide
the foundation; the per-codec refactors (A, F, K) build on them.

Each finding references the file:line so a future contributor can
pick one and execute without re-auditing.
