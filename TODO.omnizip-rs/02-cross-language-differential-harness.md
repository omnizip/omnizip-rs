# 02 — Cross-language differential test harness

- **Priority:** P0 (blocks every codec port — it IS the conformance gate)
- **Depends on:** [00](00-architecture.md)
- **Estimated effort:** 1 day
- **Location:** `tests/differential/`

## Goal

Every codec port is verified by running the same fixtures through both the
Ruby reference (omnizip) and the Rust port, then asserting byte-identical
output. This harness is the single source of truth for "the Rust port is
correct."

## Why this is the strongest guarantee

Unit tests verify the Rust port against hand-written expected outputs.
Differential tests verify it against an independent implementation. Two
implementations producing the same bytes on the same input is far stronger
than one implementation matching a fixture — it rules out fixture bugs AND
implementation bugs simultaneously.

This is why omnizip-rs exists as a separate repo rather than a fork of a C
library: the Ruby and Rust are co-evolving oracles, and divergence is
immediately visible.

## Harness design

```
tests/differential/
├── Cargo.toml                   # dev-dependency on each omnizip-* crate
├── run_differential.rs          # main entry; runs all fixture pairs
├── ruby_runner.rb               # subprocess that invokes omnizip Ruby
└── fixtures/                    # symlink or git submodule → omnizip/spec/fixtures
```

### Protocol

For each fixture file `F` (e.g. `test_hello.xz`):

1. **Decode parity:** Run both decoders on `F`'s compressed bytes.
   - Ruby: `Omnizip::Algorithms::Lzma::Decoder.new(...).decode(F)`
   - Rust: `omnizip_lzma::lzma2_decompress(bytes, expected_len)`
   - Assert `ruby_output == rust_output` byte-for-byte.

2. **Encode parity (when encoder lands):** Run both encoders on `F`'s
   plaintext at the same level.
   - Ruby: `Omnizip::Algorithms::Lzma::Encoder.new(level: 6).encode(...)`
   - Rust: `omnizip_lzma::lzma2_compress(plaintext, level)`
   - Assert `ruby_compressed == rust_compressed` byte-for-byte.
   - Then assert both decompress through reference `xz -d` to the same bytes.

3. **Cross-check with C reference:** For fixtures produced by reference C
   tools (`xz`, `zstd`), confirm Rust decompresses them correctly. This
   catches omnizip Ruby bugs too — three implementations agreeing is
   stronger than two.

### CI integration

```yaml
# .github/workflows/differential.yml
- checkout omnizip/omnizip at pinned Ruby ref (submodule or sparse-checkout)
- bundle install (Ruby)
- cargo test --workspace --test differential
```

The pinned Ruby ref lives in `tests/differential/ruby-ref.txt` so a Ruby
change can't silently break Rust without a coordinated PR.

### Fixture sources

| Corpus | Source | License | Use |
|---|---|---|---|
| omnizip fixtures | `omnizip/spec/fixtures/` | MIT | primary |
| tukaani xz fixtures | `xz/tests/files/` | 0BSD | negative tests + odd cases |
| facebook zstd fixtures | `zstd/tests/` | BSD-3 | full-level matrix |
| Silesia | silesia.cc | per-file | ratio benchmarks |
| enwik9 | Matt Mahoney | GPL-licensed payload (we don't redistribute; CI downloads) | ratio benchmarks |
| Calgary | University of Calgary | public | legacy corpus |

## Acceptance

- `cargo test --test differential` runs decode parity on every `.xz`,
  `.lzma`, `.zst` fixture in `omnizip/spec/fixtures/`.
- CI workflow runs on every PR; a divergence blocks merge.
- The harness reports per-fixture: bytes-in, bytes-out-ruby, bytes-out-rust,
  match/mismatch. On mismatch, writes both outputs to `$TMPDIR` for diffing.
- Documented protocol for adding new fixtures (drop file in
  `tests/differential/fixtures/`, re-run).

## Implementation notes

- The Ruby runner is a small subprocess (`ruby tests/differential/ruby_runner.rb
  <mode> <fixture>`) that prints hex-encoded output to stdout. The Rust test
  captures stdout, hex-decodes, compares.
- For speed: the harness parallelises fixtures across rayon. The Ruby
  subprocess is the bottleneck (Ruby is slow); we accept this because
  correctness > speed in test mode.
- For encoder parity, we pin the Ruby encoder's level mapping (Ruby level 6
  ⇒ Rust `LzmaLevel::new(6)`). The mapping table lives in
  `tests/differential/level_map.toml`.
