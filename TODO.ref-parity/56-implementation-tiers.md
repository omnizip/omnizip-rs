# Task 56 — implementation tiers in the codec registry

Status: done (2026-09-19 — `Codec::{wire_format, impl_name}` trait
defaults (OCP: zero changes to existing impls; libdeflate declares
DEFLATE), `CodecRegistry::{for_format, codec_for_format}` +
`ImplPreference`; selection = registration order (Vec, never hash
iteration)

## Gap

Ruby `implementations/` (`base`, `seven_zip`, `xz_utils`) lets the
gem swap implementations per codec at runtime. Rust: one
implementation per codec id; the DEFLATE vs LIBDEFLATE split exists
but as two unrelated ids — callers cannot say "deflate format,
whichever impl".

NOTE the hard policy boundary: in-house codecs are pure Rust only.
This task is about REGISTRY MECHANISM, not about adding C-backed
impls. "Tiers" here = named implementations of the same wire format
(reference / accelerated), all pure Rust.

## Scope

In omnizip-codecs:

- `RegisteredCodec` gains `wire_format: CodecId` (self for most;
  LIBDEFLATE declares DEFLATE's id) and `impl_name: &'static str`.
- `CodecRegistry::for_format(id)` returns the registered impls
  ordered by house default (deterministic order — registration
  order, never HashMap iteration).
- Selection API: `registry.codec_for_format(id, preference:
  ImplPreference)` where preference is Default | Named(&str) |
  Fastest. Unknown name → error listing available (mirrors codec
  and checksum registries).
- Document the contract: same wire_format ⇒ outputs
  interchangeable (decoders are shared); encoder outputs may differ
  between impls — determinism is per-impl-name, not per-format.

## Acceptance

- deflate + libdeflate both resolve via `for_format(DEFLATE)`;
  default selection is stable across runs (test with repeated
  queries, not just one).
- Unknown-impl error lists available names.
- No behavior change for existing `codec(id)` callers.

## References

`../omnizip/lib/omnizip/implementations/{base,xz_utils,seven_zip}.rb`
(seam semantics only — the external-tool adapters themselves are
Ruby-only test oracles, never ported to runtime).
