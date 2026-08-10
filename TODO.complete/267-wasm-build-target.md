# 267 — WebAssembly Build Target

- **Priority:** P3 (browser / Edge runtime support)
- **Crate:** workspace
- **Depends on:** [ADR-0001](../docs/adr/0001-pure-rust-only.md) (pure-Rust only)
- **Estimated effort:** 1 day

## Problem

omnizip-rs is pure Rust (`#![forbid(unsafe_code)]` workspace-wide)
which means it SHOULD compile to `wasm32-unknown-unknown`. But:

1. Some crates may have transitive dependencies that don't (e.g.,
   `rayon`, system time).
2. CI doesn't verify WASM builds.
3. JS/wasm-bindgen wrappers don't exist.

For LimniFS-in-browser use cases (decompress blobs on demand), this
matters.

## Design

### CI target

```yaml
# .github/workflows/wasm.yml
jobs:
  wasm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - name: Build each codec to WASM
        run: |
          for crate in omnizip-brotli omnizip-zstd omnizip-lzma omnizip-lz4 omnizip-deflate; do
            cargo build -p "$crate" --target wasm32-unknown-unknown
          done
```

### JS bindings (optional)

```rust
// omnizip-wasm crate (new)
#[wasm_bindgen]
pub fn compress_brotli(input: &[u8], level: u8) -> Vec<u8> {
    omnizip_brotli::from_spec_encoder::compress_with_quality(
        input,
        level as i32,
    )
}

#[wasm_bindgen]
pub fn decompress_brotli(input: &[u8]) -> Result<Vec<u8>, String> {
    omnizip_brotli::decoder::decode(input).map_err(|e| e.to_string())
}
```

### Polyfills

The pure-Rust constraint avoids libc dependencies. But:
- `std::time::Instant` doesn't exist on WASM. Codecs that use it
  (e.g., for timeout checks) need a no-op polyfill.
- `std::thread::scope` doesn't work on WASM (single-threaded).
  `ParallelBatch` needs a sequential fallback.

## Acceptance criteria

- [ ] All codec crates compile to `wasm32-unknown-unknown`.
- [ ] CI runs WASM build verification.
- [ ] `omnizip-wasm` crate with JS bindings (optional).
- [ ] Browser demo: HTML page that compresses input, displays ratio.

## Why this matters

WASM targets unlock browser/Edge runtimes without code changes. The
pure-Rust invariant (ADR-0001) was set specifically to make this
possible — WASM support closes the loop.
