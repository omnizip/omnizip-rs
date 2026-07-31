# 19 — BCJ filters + delta filter

- **Priority:** P1 (essential for executable compression)
- **Depends on:** [01](01-codec-trait-registry.md)
- **Estimated effort:** 1 week
- **Crate:** `omnizip-filters`

## Goal

Port the Branch / Call / Jump (BCJ) filters and delta filter. These are
preprocessing transforms applied before LZMA / ZSTD / etc. to convert
executable-code branch instructions to relative form, which compresses
much better. Used in every `.7z` and `.xz` archive of binaries.

## Ruby → Rust module map (~1,200 LOC)

| Ruby source | Rust module | Architecture |
|---|---|---|
| `filters/bcj_x86.rb` | `bcj/x86.rs` | x86 / x86_64 |
| `filters/bcj_arm.rb` | `bcj/arm.rs` | ARM 32-bit |
| `filters/bcj_arm64.rb` | `bcj/arm64.rs` | ARM 64-bit |
| `filters/bcj_ia64.rb` | `bcj/ia64.rs` | Itanium (rare) |
| `filters/bcj_ppc.rb` | `bcj/ppc.rs` | PowerPC (big-endian) |
| `filters/bcj_sparc.rb` | `bcj/sparc.rs` | SPARC (rare) |
| `filters/bcj2.rb` + `filters/bcj2/` | `bcj/bcj2.rs` | x86 4-stream variant |
| `filters/bcj.rb` | `bcj/mod.rs` | shared trait |
| `filters/delta.rb` | `delta.rs` | delta filter |
| `filters/xz_delta.rb` | `delta_xz.rs` | XZ delta variant |

## Phase scope

1. **BCJ trait** (1 day): `bcj/mod.rs`. Define `trait Filter { fn
   encode(&mut self, input: &[u8]) -> Vec<u8>; fn decode(&mut self, input:
   &[u8]) -> Vec<u8>; }`. Every BCJ variant implements this.
2. **Simple BCJs** (3 days): x86, ARM, ARM64, PPC, SPARC, IA64. These are
   ~50–100 LOC each; the transform is a single forward/backward pass
   converting branch instructions to relative encoding.
3. **BCJ2** (3 days): the x86 4-stream variant. More complex — splits the
   input into 4 streams (call targets + main). Used in 7-Zip's `.7z`.
4. **Delta filter** (1 day): `delta.rs`. Simple N-byte delta encoding,
   configurable distance (1–256).

## Acceptance

- **Differential gate:** every BCJ filter produces byte-identical output
  between Ruby and Rust on every executable fixture (`.exe`, `.so`, `.dll`,
  `.dylib`).
- **Round-trip gate:** encode then decode returns original input.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- BCJ filters are stateful: they maintain a small state across the input
  (last branch target, etc.). The state must be deterministic across runs.
- BCJ2 is the only complex one. Port it from `liblzma/lzma/lzma_decoder.c`
  + the Ruby; they should agree.
- The delta filter's distance parameter defaults to 1; this is the most
  common setting for raw PCM audio.
