# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

Pure-Rust ports of omnizip's Ruby compression codecs. The Ruby implementations at
[`../omnizip`](https://github.com/omnizip/omnizip) (sibling directory) are the
**algorithmic reference**; every Rust module is a line-by-line translation of the
corresponding Ruby file. The C reference libraries (`tukaani-project/xz`,
`facebook/zstd`) are consulted only for perf tuning **after** the Ruby port is
verified correct.

Consumer: [LimniFS](https://github.com/limnifs/limnifs) — content-addressed FS
where `DropId = BLAKE3(plaintext)`. Codec non-determinism breaks dedup, so
**every encoder must produce byte-identical output for the same input + level
across runs, machines, and Rust versions.**

Two source-of-truth docs you must read before any porting work:
- [`PLAN.md`](PLAN.md) — Ruby → Rust module map and phased delivery for LZMA + ZSTD.
- [`TODO.omnizip-rs/README.md`](TODO.omnizip-rs/README.md) — MECE task breakdown
  for the entire workspace, with priorities and dependencies.

## Build, test, lint

```bash
cargo build --workspace                 # build every crate
cargo test --workspace                  # run every unit + integration test
cargo test -p omnizip-lzma              # tests for one crate only
cargo test -p omnizip-lzma --lib bit_model  # single test module / filter
cargo test --workspace --test differential   # cross-language gate (when wired in)
cargo clippy --workspace --all-targets -- -D warnings   # pedantic = warn at workspace level
cargo fmt --all -- --check
```

Toolchain: stable Rust ≥ 1.75 (per `Cargo.toml` `rust-version`); edition 2021;
`resolver = "2"`.

## Workspace architecture

One crate per algorithm family, plus a shared trait crate. No crate reaches into
another's internals — cross-crate communication is via the `omnizip-codecs`
trait + registry + error types only.

```
omnizip-codecs/      Codec trait, CodecRegistry, CodecId, CompressionLevel, OmnizipError
omnizip-lzma/        LZMA / LZMA2 / XZ               (porting — Phase A in flight)
omnizip-zstd/        Zstandard                       (skeleton — Phase A next)
omnizip-filters/     BCJ-x86, delta (Filter trait)
omnizip-deflate/     wraps miniz_oxide
omnizip-brotli/      wraps brotli crate
omnizip-snappy/      wraps snap
omnizip-lz4/         wraps lz4_flex (LZ4 + LZ4_HC)
```

The crate set and Cargo workspace lints are defined at the root `Cargo.toml`.
`#![forbid(unsafe_code)]` is workspace-wide (`[workspace.lints.rust]`); SIMD
acceleration plans use `std::simd`, never raw `unsafe`. Clippy `pedantic = "warn"`
is also workspace-wide.

### Adding a codec = one new crate + one `register()` call

1. New crate dir under workspace root, member listed in root `Cargo.toml`.
2. Implement `Codec` trait from `omnizip-codecs` (or `Filter` from
   `omnizip-filters` for preprocessing transforms).
3. Caller registers it on a `CodecRegistry`; dispatch code never changes. This
   is the OCP applied to codecs — don't add per-codec branches in dispatch sites.

`CodecId` is a `u16` newtype with assigned constants in
`omnizip-codecs/src/codec.rs`. Allocate new ids there, in task-README order.
`CompressionLevel` is a `u8` newtype; per-codec clamping happens at compress
time, returning `OmnizipError::LevelOutOfRange` for out-of-range values.

## Porting LZMA / ZSTD — the workflow that matters

The user's current priority is fully porting `../omnizip`'s Ruby LZMA and ZSTD
to Rust. The phased plan lives in `PLAN.md` and per-task files in
`TODO.omnizip-rs/`. Phases must ship in order — each one is the next one's
oracle:

- **Phase A — decoder + range coder + match finder.** Decode parity with the
  Ruby first. The decoder is the encoder port's oracle, so porting it first
  means every later encoder test can ask "does my output decode correctly?"
- **Phase B — encoder core (level 0–3 equivalent).**
- **Phase C — optimal parser + LZMA2 chunking + XZ container (LZMA) /
  FSE + multi-block (ZSTD).**

Task files:
- LZMA: [`10`](TODO.omnizip-rs/10-lzma-phase-a-decoder.md) →
  [`11`](TODO.omnizip-rs/11-lzma-phase-b-encoder.md) →
  [`12`](TODO.omnizip-rs/12-lzma-phase-c-optimal-xz.md)
- ZSTD: [`13`](TODO.omnizip-rs/13-zstd-phase-a-decoder.md) →
  [`14`](TODO.omnizip-rs/14-zstd-phase-b-encoder.md) →
  [`15`](TODO.omnizip-rs/15-zstd-phase-c-fse.md)

### Ruby source layout (the authoritative reference)

```
../omnizip/lib/omnizip/algorithms/lzma/      31 files, 7,558 LOC — core LZMA
  ├── constants.rb, bit_model.rb, probability_models.rb, state.rb, lzma_state.rb, xz_state.rb
  ├── range_coder.rb, range_decoder.rb, range_encoder.rb
  ├── xz_range_encoder.rb, xz_range_encoder_exact.rb, xz_buffered_range_encoder.rb
  ├── match_finder.rb, match_finder_config.rb, match_finder_factory.rb, xz_match_finder_adapter.rb
  ├── literal_encoder.rb, literal_decoder.rb, length_coder.rb, distance_coder.rb
  ├── decoder.rb, lzip_decoder.rb, lzma_alone_decoder.rb, xz_utils_decoder.rb   ← decoders
  ├── encoder.rb, xz_encoder.rb, xz_encoder_fast.rb, optimal_encoder.rb          ← encoders (Phase B/C)
  ├── xz_probability_models.rb, xz_price_calculator.rb, match.rb, dictionary.rb
../omnizip/lib/omnizip/algorithms/lzma2/      7 files, 906 LOC — LZMA2 chunking + container adapter
../omnizip/lib/omnizip/algorithms/zstandard/  11 files, 3,150 LOC — ZSTD
  ├── constants.rb, encoder.rb, decoder.rb, sequences.rb
  ├── frame.rb + frame/   (frame, header, block)
  ├── fse.rb + fse/        (FSE entropy coder)
  ├── huffman.rb, huffman_encoder.rb
  ├── literals.rb, literals_encoder.rb
```

The per-task files (`10-…`, `13-…`) contain the precise Ruby → Rust module
mapping with LOC counts and phase assignments. Use them as the work checklist.
The Rust-side module structure for each phase is also documented in
[`TODO.omnizip-rs/00-architecture.md`](TODO.omnizip-rs/00-architecture.md).

### Differential conformance gate (release blocker)

[`TODO.omnizip-rs/02-cross-language-differential-harness.md`](TODO.omnizip-rs/02-cross-language-differential-harness.md)
defines the harness. Once wired (location: `tests/differential/`):

1. CI clones `omnizip/omnizip` at a pinned Ruby ref (recorded in
   `tests/differential/ruby-ref.txt` so a Ruby change can't silently break Rust).
2. For each fixture under `../omnizip/spec/fixtures/{xz,lzma,zst}/`:
   - Decode through Ruby → capture bytes.
   - Decode through Rust → capture bytes.
   - Assert byte-identical.
3. For encoder PRs: encode through both at the same level, then run both
   outputs through reference `xz -d` / `zstd -d`, assert byte-identical.
4. Any divergence blocks merge.

The Ruby runner is a small subprocess invoked as
`ruby tests/differential/ruby_runner.rb <mode> <fixture>` that prints
hex-encoded output to stdout. Level mapping for encoder parity lives in
`tests/differential/level_map.toml`.

### Porting idioms (Ruby → Rust)

These are the traps noted in the task files:

- **Range coder arithmetic.** Ruby uses bignum `Integer`; Rust must use `u32`/
  `u64` explicitly with deliberate carry handling. Don't rely on auto-promotion.
- **Match finder hash chains.** Ruby rebuilds on every call; Rust should reuse
  allocations via a `reset()` method. Hash table is `Vec<u32>` head pointers,
  chain is `Vec<u32>` of positions.
- **XZ-utils decoder** (`xz_utils_decoder.rb`, 1,311 LOC) is the largest single
  file — port it last, after simpler alone + lzip decoders validate the coder
  pieces.
- Keep methods named after the Ruby method they translate, modulo snake_case
  conventions being identical in both languages.
- Source-file copyright headers from Ruby (Ribose Inc., MIT) carry over. The
  Rust port is dual MIT OR Apache-2.0; see [`LICENSE-NOTICE.md`](LICENSE-NOTICE.md).

## Invariants

1. **`#![forbid(unsafe_code)]` is workspace-wide** and pre-existing in every
   crate. Don't add `unsafe` blocks; if a hot path genuinely needs SIMD, use
   `std::simd` (task [`32`](TODO.omnizip-rs/32-simd-acceleration.md)).
2. **Determinism is a hard requirement.** No thread-scheduling-dependent block
   boundaries, no `HashSet` iteration in encode paths, no time-seeded RNGs.
3. **One `in_progress` task per crate at a time.** Move task status in the
   TODO file header from `pending` → `in_progress` → `done` — done means its
   acceptance criteria pass in CI on linux + macOS + stable Rust with the
   differential harness green.
4. **No shims, no stubs.** Placeholder functions (like the current
   `lzma2_compress` returning `LevelUnavailable`) are scaffolding to keep the
   crate compiling between phases; replace them with the real implementation
   when the corresponding phase ships.
5. **Spec-first.** Wire-format and codec-id changes update `PLAN.md` +
   `TODO.omnizip-rs/README.md` before code.
6. **Rebase-merge all PRs.** No direct pushes to `main` (also enforced by the
   user's global git rules — never push tags, never push to main, never merge
   to main without explicit approval).
