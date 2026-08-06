# 173: Brotli — Q≥2 Encoder (Combined Insert+Copy Commands)

## Priority: P4

## Status: pending — comprehensive port plan documented.

## Context

The pure-Rust brotli encoder (`fast_encoder.rs`) is a verbatim port of
upstream's `compress_fragment_two_pass`, the q=0/q=1 fast path. It uses
the 3-tuple `INSERT + DISTANCE + COPY-LD` pattern, with separate
Huffman codes for each.

The q≥2 encoder (`compress_fragment.rs` and the optimal parser in
`backward_references_hq.rs`) emits combined INSERT+COPY commands via
`combine_length_codes(inscode, copycode, use_last_distance)`, producing
a single code in the 704-symbol alphabet. This achieves ~10–20% better
ratio at 10–100× the CPU cost.

## What's needed

### Step 1: Port `compress_fragment.rs` (q=2..6, ~700 LOC of actual code)

Upstream file: `brotli-8.0.4/src/enc/compress_fragment.rs` (1179 LOC
including boilerplate).

**Caution** (from upstream's header comment):
> lots of the functions look structurally the same as two_pass, but
> have subtle index differences. Examples: IsMatch checks p1[4] and
> p1[5]. The hoops that BuildAndStoreCommandPrefixCode goes through
> are subtly different in order (eg memcpy x+24, y instead of +24,
> y+40. **Pretty much assume compress_fragment_two_pass is a trap!**
> except for store_meta_block_header.

Key differences from `compress_fragment_two_pass.rs`:

1. **Hash function**: uses 8-byte hash via `BROTLI_UNALIGNED_LOAD64`,
   not 4-byte. The 8-byte hash captures longer context for better
   match quality.
2. **IsMatch**: checks 5 bytes (4 + 1) instead of 4.
3. **BuildAndStoreCommandPrefixCode**: the 704-entry scatter pattern
   differs from two_pass. The memcpy offsets are different.
4. **StoreCommand**: emits combined INSERT+COPY via
   `combine_length_codes(inscode, copycode, use_last_distance)`.
5. **kCmdHistoSeed**: a 128-entry seed histogram that biases toward
   common command codes.
6. **BuildAndStoreLiteralPrefixCode**: histogram adjustment that
   boosts rare symbols to ensure they get non-zero code lengths.

### Step 2: Port `backward_references_hq.rs` (q=7..11, ~3K LOC)

The Zopfli-style optimal parser. Much larger and more complex than
compress_fragment. Splits input into "commands" via dynamic programming
with detailed cost models.

### Step 3: Quality dispatch in `BrotliCodec::compress`

```rust
fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
    let quality = level.as_u8().min(11);
    match quality {
        0..=1 => Ok(fast_encoder::vendored_compress(plaintext)),
        2..=6 => Ok(compress_fragment::compress(plaintext, quality)),
        7..=11 => Ok(backward_references_hq::compress(plaintext, quality)),
        _ => unreachable!(),
    }
}
```

## Why deferred

Our `compress_fragment_two_pass` encoder already produces valid brotli
that any conformant decoder (including ours, `brotli -d`, browsers)
accepts. The compression ratio is competitive with upstream's q=1.
LimniFS cares about determinism + round-trip integrity, not max ratio.

A q≥2 path would add ~5K LOC and significant complexity for a ratio
bump that doesn't unblock any consumer.

## Acceptance Criteria

- Round-trip via own decoder + `brotli -d` at every quality 0..11.
- Ratio on `enwik8` within 5% of upstream `brotli -q N`.
- No nondeterminism: same input + quality always produces identical
  bytes across runs, machines, and Rust versions.

## Implementation skeleton (for the next session)

```rust
// omnizip-brotli/src/compress_fragment.rs

#![forbid(unsafe_code)]
#![allow(non_snake_case, non_upper_case_globals, clippy::too_many_arguments)]

use crate::fast_encoder::{BrotliWriteBits, HuffmanTree, memcpy};

const kCmdHistoSeed: [u32; 128] = [
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    // ... full table from upstream
];

fn Hash(p: &[u8], shift: usize) -> u32 { ... }
fn IsMatch(p1: &[u8], p2: &[u8]) -> bool { ... }

fn BuildAndStoreLiteralPrefixCode(...) -> usize { ... }
fn BuildAndStoreCommandPrefixCode(...) { ... }
fn StoreCommand(...) { ... }
fn CreateCommands(...) { ... }

pub fn compress(input: &[u8], quality: u8) -> Vec<u8> { ... }
```

Then in `lib.rs`:
```rust
pub mod compress_fragment;
```

And in `BrotliCodec::compress`:
```rust
match quality {
    0..=1 => Ok(fast_encoder::vendored_compress(plaintext)),
    2..=6 => Ok(compress_fragment::compress(plaintext, quality)),
    _ => Ok(fast_encoder::vendored_compress(plaintext)), // fallback
}
```
