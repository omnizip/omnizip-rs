# 40 — LimniFS integration

- **Priority:** P1 (closes the loop — omnizip-rs exists to serve LimniFS)
- **Depends on:** [10](10-lzma-phase-a-decoder.md), [13](13-zstd-phase-a-decoder.md)
- **Estimated effort:** 2 days
- **Repos touched:** `omnizip/omnizip-rs`, `limnifs/limnifs`

## Goal

Wire LimniFS's `limnifs-core::codec` registry to consume `omnizip-lzma`
and `omnizip-zstd` crates. After this task, LimniFS's codec dispatch
delegates to omnizip-rs for LZMA and ZSTD, replacing the temporary
`ruzstd` + `lzma-rs` dependencies.

## Integration shape

### limnifs-core/Cargo.toml

```toml
[dependencies]
omnizip-codecs = { version = "0.1", path = "../../../omnizip/omnizip-rs/omnizip-codecs" }
omnizip-lzma   = { version = "0.1", path = "../../../omnizip/omnizip-rs/omnizip-lzma" }
omnizip-zstd   = { version = "0.1", path = "../../../omnizip/omnizip-rs/omnizip-zstd" }
```

(Initially path deps for development; switch to crates.io deps after
task [35](35-crates-io-publishing.md).)

### limnifs-core/src/codec/zstd.rs

Replace the `ruzstd`-backed `ZstdCodec` with an adapter that delegates to
`omnizip_zstd::compress` / `omnizip_zstd::decompress`:

```rust
use omnizip_codecs::{Codec, CompressionLevel};
use omnizip_zstd::ZstdLevel;

pub struct ZstdCodec;

impl Codec for ZstdCodec {
    fn id(&self) -> u8 { super::CODEC_ZSTD }
    fn name(&self) -> &'static str { "zstd" }

    fn compress(&self, plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
        omnizip_zstd::compress(plaintext, ZstdLevel::Default)
            .map_err(|e| CoreError::Corrupt { reason: e.to_string() })
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, CoreError> {
        omnizip_zstd::decompress(compressed, expected_len)
            .map_err(|e| CoreError::Corrupt { reason: e.to_string() })
    }
}
```

### Codec id mapping

LimniFS uses u8 wire codec ids; omnizip-codecs uses u16. The mapping lives
in `limnifs-core/src/codec/omnizip_adapter.rs`:

```rust
const WIRE_LZMA: u8 = 0x03;  // LimniFS wire id
const WIRE_ZSTD: u8 = 0x02;
// → adapter maps to omnizip CodecIds at dispatch time
```

### Phased rollout

1. **Phase A (this task)**: switch `limnifs-core`'s ZSTD + LZMA codec
   modules to delegate to omnizip-rs. All existing tests pass unchanged.
2. **Phase B (follow-up)**: expose omnizip-rs's higher levels (ZSTD 2–22,
   LZMA 4–9) via the `--codec-map` flag (limnifs roadmap item 06).
3. **Phase C (follow-up)**: remove `ruzstd` and `lzma-rs` deps from
   `limnifs-core` once omnizip-rs Phase A decoders are byte-identical.

## Acceptance

- `cargo test --workspace --all-features` in limnifs passes with the
  omnizip-rs deps wired in.
- Every existing LZMA / ZSTD fixture round-trips through the new
  delegation path.
- The `limnifs-core` codec registry still holds 5 codecs (store, lz4, zstd,
  xz, brotli); the only change is the implementation behind the zstd and
  xz entries.
- No new `unsafe` in limnifs-core.
- Clippy clean.

## Implementation notes

- Path deps (`path = "../../../omnizip/omnizip-rs/..."`) work for
  development. For LimniFS's air-gapped builds, switch to crates.io deps
  (task 35) so `cargo vendor` doesn't need the omnizip repo on the local
  filesystem.
- The adapter is a thin shim — no algorithm code in limnifs-core. All
  compression logic lives in omnizip-rs.
- Document the integration in `TODO.impl/03-core-reader/03-omnizip-rs-integration.md`
  so future LimniFS contributors understand the relationship.
