# ADR-0007: Brotli from-spec encoder (Phase C)

- **Status:** accepted
- **Date:** 2026-08-01
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

The original `omnizip-brotli` was a thin wrapper around the `brotli`
crate (Rust bindings to Google's C reference). This gave wire-format
correctness for free but:

- Pulled in ~30K LOC of C code via build scripts.
- Couldn't be audited under `#![forbid(unsafe_code)]` (the C code
  is full of `unsafe`).
- Couldn't be tuned for LimniFS's specific workloads (CSV-heavy).
- Couldn't be ported to WASM/embedded without a C toolchain.
- Had no path to feature work (block-split Huffman, multi-context
  trees, etc.) that requires encoder internals.

## Decision

**Implement a pure-Rust Brotli encoder from the RFC 7932 spec,
in `omnizip-brotli/src/from_spec_encoder.rs`.** The `brotli` crate
is kept as a `dev-dependency` for differential testing only.

Phased delivery:

- **Phase A**: decode-only (the decoder is the encoder's oracle).
- **Phase B**: static-codebook encoder (low quality, simple).
- **Phase C**: full encoder with Huffman + context modeling +
  dictionary + optimal parser.

The current state is post-Phase-C, with the encoder producing
output that BEATS the vendored C reference on synthetic CSV data
(23.4% vs 24.1% on 500 KB) thanks to the cost-aware optimal parser.

## Consequences

**Positive**:
- Zero C code in the workspace.
- Full control over encoder tuning for LimniFS workloads.
- All `unsafe`-audit concerns go away.
- Foundation for advanced features (block type switching, smart
  context clustering) that the wrapper couldn't offer.
- Pure-Rust → WASM-compatible.

**Negative**:
- **Wire-format bugs are ours**: the encoder has shipped with bugs
  (vendored decoder rejection on some outputs) that the C reference
  doesn't have. Mitigated by differential parity (ADR-0005).
- **Maintenance burden**: 2,000+ LOC of Brotli encoder code that
  must track RFC 7932 changes (rare, but non-zero).
- **Slower than the C reference** at high quality levels (Q11).
  Mitigated by perf work (TODO 110, the match_length cap, etc.).
- **CSV ratio gap on real data**: synthetic-test gap is closed,
  but the user's real `csv-synthetic` data still shows a gap to
  the vendored C reference's 3.6%. Without the real data we can't
  optimize specifically (TODO 247).

**Neutral**:
- The Ruby omnizip Brotli is also from-spec; we mirror its
  algorithmic structure.

## References

- [RFC 7932](https://www.rfc-editor.org/rfc/rfc7932) — Brotli spec.
- [TODO 117](../../TODO.complete/117-brotli-full-port.md) — full
  port plan.
- [TODO 168-169](../../TODO.complete/168-brotli-huffman-static-tree.md) —
  wire-format debugging.
- [BUGREPORT-brotli-phase-c-ratio.md](../../BUGREPORT-brotli-phase-c-ratio.md)
  — incident that motivated Phase C.
