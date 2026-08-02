# 69 — Conformance corpus + differential CI

## Gap

The workspace has parity tests for a few golden fixtures but no
automated differential testing against the Ruby `omnizip` reference or
the C reference libraries at scale. The LimniFS proposal requires:

- ZSTD: decode every fixture under `../omnizip/spec/fixtures/zst/`
  byte-identical to the C decoder.
- LZMA: decode every fixture under `../omnizip/spec/fixtures/xz/`
  byte-identical to the C decoder.
- Encode parity: Rust encoder output at level L decodes identically
  via reference `xz -d` / `zstd -d`.

## Implementation

1. **Wire `tests/differential/`** — clone `omnizip/omnizip` at a pinned
   Ruby ref (stored in `tests/differential/ruby-ref.txt`).
2. **Ruby runner** — subprocess `ruby ruby_runner.rb <mode> <fixture>`
   that hex-dumps output to stdout.
3. **Level mapping** — `tests/differential/level_map.toml` maps
   Rust levels to Ruby levels (since they may differ).
4. **CI workflow** — `.github/workflows/differential.yml` runs the
   suite on every PR.

## Coverage matrix

| Codec | Decode Ruby | Decode C | Encode → C decode |
|-------|------------|----------|-------------------|
| LZMA  | ❌          | ✅ (xz_parity) | ❌ |
| ZSTD  | ❌          | ✅ (zstd_parity) | ✅ (zstd_encoder_parity) |
| Snappy| n/a        | n/a      | ❌ |
| LZ4   | n/a        | n/a      | ❌ |
| DEFLATE| n/a       | n/a      | ❌ |
| Brotli| n/a        | n/a      | ❌ |

## Files

- `tests/differential/ruby_runner.rb` — Ruby subprocess wrapper.
- `tests/differential/ruby-ref.txt` — pinned ref.
- `tests/differential/level_map.toml` — level mapping.
- `.github/workflows/differential.yml` — CI job.

## Test strategy

- Run all decode fixtures through both Rust and C/Ruby decoders.
- Run all encode outputs through the C decoder.
- Block PRs on any divergence.
