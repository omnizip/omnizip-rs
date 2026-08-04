# TODO 110: ZSTD encoder O(N²) perf cliff on text ≥ 8KB

## Symptom

`ZstdCodec::compress` hangs (takes >60 seconds) on text inputs of 8 KiB
and larger, at every level (1, 3, 9, 19, 22). On 4 KiB of identical
content it completes in milliseconds; on 8 KiB it stalls indefinitely.

## Repro

```rust
use omnizip_codecs::{Codec, CompressionLevel};
use omnizip_zstd::ZstdCodec;

let words = ["the", "quick", "brown", "fox"];  // short vocabulary
let mut text = Vec::new();
let mut seed: u64 = 0xDEAD_BEEF_CAFE_BABE;
let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
while text.len() < 8192 {
    text.extend_from_slice(words[(next() as usize) % words.len()].as_bytes());
    text.push(b' ');
}
let _ = ZstdCodec.compress(&text, CompressionLevel::new(1));  // hangs
```

## Suspected root cause

The encoder's match finder + sequence assembly has an O(N²) loop
somewhere. The word-at-a-time `count_match` (just landed) didn't help,
so the cliff is in a different layer. Most likely candidates:

1. `encoder/block.rs` — sequence list rebuilding after block boundaries.
2. `encoder/match_finder.rs` — chain walks not properly bounded.
3. `encoder/sequences.rs` — repeat-offset search loops.

## Acceptance criteria

- [ ] 8 KiB text input compresses in < 1 second at level 1.
- [ ] 64 KiB text input compresses in < 5 seconds at level 1.
- [ ] Differential parity against `zstd -d` preserved.
- [ ] Bench harness can include ZSTD at all levels without timeouts.

## Priority

**P0** — currently blocks the bench harness and any production use of
the pure-Rust ZSTD encoder on text inputs.
