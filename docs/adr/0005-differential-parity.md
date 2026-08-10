# ADR-0005: Differential parity vs C/Ruby references

- **Status:** accepted
- **Date:** 2026-07-20
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

omnizip-rs ports codecs from two reference implementations:

1. **Ruby omnizip** ([`../omnizip`](https://github.com/omnizip/omnizip))
   — the algorithmic reference; each Ruby file is a line-by-line
   spec for the Rust port.
2. **C reference libraries** (`tukaani-project/xz`, `facebook/zstd`,
   `google/brotli`) — the wire-format reference; what real-world
   decompressors expect.

Wire-format compatibility is non-negotiable. If our LZMA encoder
produces output that `xz -d` rejects, we have a bug — even if our
own decoder accepts it. Conversely, if our decoder rejects
`brotli -q 11` output, we can't claim Brotli compatibility.

## Decision

**Every encoder/decoder must achieve differential parity with both
references** before being marked "done":

1. **Ruby → Rust**: encode via Ruby, decode via Rust. Output must
   byte-match the original.
2. **Rust → Ruby**: encode via Rust, decode via Ruby. Same.
3. **C reference decode**: encode via Rust, decode via `brotli -d`
   / `xz -d` / `zstd -d`. Same.
4. **C reference encode**: encode via `brotli -qf` / etc., decode
   via Rust. Same.

The harness lives in [`tests/differential/`](../../tests/differential/).
A pinned Ruby ref is recorded in `tests/differential/ruby-ref.txt`
so a Ruby change can't silently break Rust.

## Consequences

**Positive**:
- Wire-format bugs caught before merge, not after release.
- "Works on my machine" claims are testable.
- Pinned Ruby ref means we control when to absorb upstream changes.
- CI runs against real C tools, not just our own decoder.

**Negative**:
- **CI runtime**: differential tests add ~2 minutes per codec.
  Mitigated by running them only on PRs that touch encoder code.
- **Fixture management**: `tests/fixtures/xz/` and `tests/fixtures/zst/`
  grow with each bug repro. Periodic pruning.
- **External tool dependency**: CI must have `xz`, `zstd`, `brotli`,
  `lz4` installed. Cached via `actions/cache`.
- **Ruby interpreter**: requires MRI Ruby 3.x in CI. Some platforms
  (Windows) need careful setup.

**Neutral**:
- Matches the [LimniFS quality bar](https://github.com/limnifs/limnifs)
  for "real codec" status.

## References

- [`tests/differential/`](../../tests/differential/) — harness source.
- [Differential testing](https://en.wikipedia.org/wiki/Differential_testing)
- [tukaani-project/xz](https://github.com/tukaani-project/xz)
- [facebook/zstd](https://github.com/facebook/zstd)
- [google/brotli](https://github.com/google/brotli)
