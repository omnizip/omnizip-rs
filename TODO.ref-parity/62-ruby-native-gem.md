# Task 62 — Ruby acceleration: native gem exposing the Rust codecs

Status: partially done (2026-09-19, RUST SIDE COMPLETE AND PROVEN —
omnizip-ffi cdylib: C ABI (ozip_compress/ozip_decompress/
ozip_last_error/ozip_free) over zstd/bzip2/lzma(+xz alias),
catch_unwind at every entry, thread-local last-error, explicit
buffer ownership (ozip_free), per-item allow(unsafe_code) on the
raw-pointer shim only (crate stays deny; the workspace forbid
cannot be scoped — lint-exception documented in Cargo.toml).
PROVEN FROM REAL RUBY: ruby 3.4.8 + stdlib Fiddle round-trips 200 KB
through all three codecs with a working error surface
(tests/ruby_fiddle_smoke.rb, run after cargo build --release -p
omnizip-ffi). DESIGN CHANGE vs the original sketch: Fiddle over a
cdylib REPLACES magnus/rb-sys as the binding mechanism — no
compiled Ruby extension, no rake-compiler, works with the stdlib
alone; magnus remains an option if zero-copy RSTRING access ever
matters. REMAINING: the gem-side tier (implementations/rust +
Fiddle loader + pure-Ruby fallback) — a PR against ../omnizip,
gated on the owner's alignment call; prebuilt per-platform dylib
distribution story

## Goal

"use rust to accelerate the ruby gem." The gem already has the seam:
`implementations/` swaps codec implementations behind
`algorithm_registry`. Add a `rust` implementation tier backed by a
native extension, pure-Ruby fallback intact.

## Scope

- Gem (in the Ruby repo, coordinated there): `ext/omnizip_rust/`
  via **magnus** (rb-sys underneath), built with rake-compiler;
  static-link the codec crates (cargo cdylib). Start set: **bzip2,
  zstd, lzma** (stable APIs, biggest Ruby-side wins), then brotli,
  deflate64, ppmd.
- Ruby side: `Omnizip::Implementations::Rust::<Codec>` wrapping the
  extension, registered in the algorithm registry as a preferred
  implementation with automatic pure-Ruby fallback when the native
  ext is absent (require-rescue, feature-flagged).
- FFI surface (kept deliberately narrow):
  `compress(bytes, level) / decompress(bytes, max) → bytes`,
  later `compress_stream` once task 57's traits stabilize. GVL
  released around every native call (magnus `do` on Ruby objects
  off-thread only).
- Memory: bytes cross as Ruby String ↔ Vec<u8> with one copy each
  way; no shared buffers, no object caching across calls.

## Hard gates

1. **Byte-identical**: the extension's outputs must equal the
   pure-Ruby encoder's outputs on the differential corpus (same
   input + level ⇒ same bytes — our encoders already guarantee this
   contract for LimniFS dedup; the gem tier swap must not disturb
   DropId stability). Run the existing cross-language harness with
   the native tier enabled.
2. **Determinism**: no thread-scheduling-dependent output (inherited
   from the Rust side; FFI adds none).
3. Error mapping: OmnizipError → typed Ruby exceptions, never
   panics unwinding across FFI (catch_unwind at the boundary).
4. Windows/macOS/Linux precompiled gems or vendored source build;
   MSRV of the cdylib ≤ the gem's Ruby support floor.

## Acceptance

- Gem spec suite green with `OMNIZIP_RUST=1` and `=0` (both tiers).
- Differential corpus byte-identical between tiers.
- Benchmark: ≥5× Ruby speed on bzip2/zstd/lzma at matched levels
  (expected ~10-100×; the bar is deliberately modest).

## References

`../omnizip/lib/omnizip/implementations/{base,xz_utils}.rb` (the
seam pattern), `algorithm_registry.rb`.
