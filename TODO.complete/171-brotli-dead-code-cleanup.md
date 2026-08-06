# 171: Brotli Decoder — Remove Dead Code from Exploration Phase

## Priority: P2 (code cleanliness)

## Status: pending

## Context

`omnizip-brotli/src/decoder.rs` accumulated several exploration stubs
during the TODO 170 investigation. With the round-trip working, these
are dead code that confuse readers and bloat compile times.

## Dead code to remove

Inside `omnizip-brotli/src/decoder.rs`:

- `struct CmdLut` and `fn build_cmd_lut()` — shadowed by
  `crate::prefix::kCmdLut`. Remove both.
- `fn insert_length_prefix`, `fn copy_length_prefix`,
  `fn combine_length_codes` — only used by `build_cmd_lut`.
- `fn decode_copy_len` — exploration stub never called by the
  working decoder path.
- `fn decode_long_distance` — exploration stub superseded by
  `decode_distance_from_code`.
- `pub fn decode_distance_code` (the Phase C stub returning
  `ceil_log2(num_direct.max(1))` bits) — never used.
- `pub enum InsertCopyCommand` and `pub fn decode_insert_copy_command`
  — Phase C stubs never used.

## Snake-case warnings on vendored upstream code

`omnizip-brotli/src/fast_encoder.rs` is a line-by-line port of
upstream's `compress_fragment_two_pass.rs` (BSD-3-Clause). It carries
119 `should have a snake case name` warnings because upstream uses
CamelCase Rust functions (`EmitInsertLen`, `BrotliWriteBits`, etc.).
Renaming them would break the line-by-line correspondence with upstream
that makes audits easy.

Add `#![allow(clippy::too_many_lines, clippy::cast_possible_truncation, clippy::needless_range_loop)]` at the top of `fast_encoder.rs` for
vendored style. Keep the rest of the crate under `pedantic = warn`.

## Acceptance Criteria

- `cargo build -p omnizip-brotli` emits zero `unused` warnings.
- `cargo test -p omnizip-brotli --lib` still passes 68+ tests.
- Round-trip property test still passes.
- Line-by-line diff vs upstream `compress_fragment_two_pass.rs`
  remains mechanical (only transport/alloc adapters differ).
