# 34 — no_std / embedded support

- **Priority:** P3 (niche; enables firmware / microcontroller use)
- **Depends on:** [10](10-lzma-phase-a-decoder.md)
- **Estimated effort:** 2 weeks
- **Location:** per-crate `alloc` / `no-alloc` features

## Goal

Make `omnizip-lzma` and `omnizip-zstd` decoders workable on `no_std`
targets with only `alloc` (no `std`). EnablesLimniFS-style content
addressing on microcontrollers, bootloaders, and firmware update systems.

## Approach

Each codec crate gains feature flags:

```toml
[features]
default = ["std"]
std = ["alloc"]
alloc = []
```

- `std` mode: full functionality, uses `std::io`, `std::fs`, etc.
- `alloc` mode: uses `alloc::vec::Vec`, `alloc::string::String` but no I/O.
  The API takes `&[u8]` → `Vec<u8>` instead of `Read`/`Write` traits.
- `no-alloc` mode (decoder only): in-place decode into a caller-provided
  buffer. Encode is not supported (encoders need allocations for match
  finders).

## Phase scope

1. **Audit `std` usage** (2 days): grep each codec for `std::io`,
   `std::fs`, `std::net`, etc. Most uses are in test code; the library
   code uses mostly `alloc`.
2. **Abstract I/O** (3 days): the decoder currently takes `&mut dyn Read`.
   In `alloc` mode, add a `fn decompress_bytes(compressed: &[u8]) ->
   Result<Vec<u8>, OmnizipError>` entry point that doesn't need `Read`.
3. **Feature-gate** (2 days): mark `std`-only modules with
   `#[cfg(feature = "std")]`. Build with `--no-default-features --features
   alloc`.
4. **CI matrix** (1 day): add a `no-std` build job that compiles each
   crate with `--no-default-features --features alloc` for `thumbv7em-none-eabi`.

## Acceptance

- `cargo build -p omnizip-lzma --no-default-features --features alloc`
  succeeds on the host.
- `cargo build -p omnizip-lzma --no-default-features --features alloc
  --target thumbv7em-none-eabi` succeeds (with appropriate target installed).
- The decoder round-trips on every fixture in `alloc` mode.
- The encoder is `cfg(feature = "alloc")`-gated; `no-alloc` decode-only
  mode compiles.
- Clippy clean in all feature combinations.

## Implementation notes

- This is P3 because LimniFS itself doesn't need `no_std`. But the codec
  crates are reusable; making them `no_std`-friendly increases the user
  base (firmware update systems, embedded bootloaders).
- The `Read`/`Write` trait abstraction is std-only. In `alloc` mode,
  everything is `&[u8]` → `Vec<u8>`. Add a parallel API; don't try to
  shoehorn `Read` into `no_std`.
- `getrandom` is `no_std`-compatible if the platform provides an entropy
  source. For codecs we don't need randomness; only the AEAD layer does.
