# LZMA — growing `Vec<u8>` output instead of circular buffer

## Status

**Intentional simplification.** Will be reconciled when LZMA2
multi-chunk support lands.

## Affected code

`omnizip-lzma/src/decoder/lzma1.rs` — `Lzma1Decoder::decode`,
`copy_match`.

## What the C reference / Ruby port does

The C reference (`xz/src/liblzma/lz/lz_decoder.h`) and the Ruby
port (`xz_utils_decoder.rb`) both use a pre-allocated circular
buffer with the layout:

```text
[LZ_DICT_INIT_POS zero bytes][dict_size circular region]
```

Where `LZ_DICT_INIT_POS = 2 * LZ_DICT_REPEAT_MAX = 576`. The
`LZ_DICT_INIT_POS` prefix exists so that match-distance lookups
near the start of the stream do not require special-casing.

The buffer has a fixed total size of `LZ_DICT_INIT_POS + dict_size`.
Writes wrap around: `dic[dicPos] = byte; dicPos = (dicPos + 1) %
dicBufSize;`.

A separate `dict.full` counter tracks the total bytes ever written
(uncapped); lookups use `dict.full - distance - 1` to find the
source byte, then map through `dict_index(pos)` to the physical
buffer index.

## What the Rust port does

`Lzma1Decoder::decode` writes decoded bytes into a growing
`Vec<u8>`:

```rust
fn copy_match(&self, output: &mut Vec<u8>, distance: usize, length: u32) {
    let len = length as usize;
    let src_start = output.len() - distance - 1;
    output.reserve(len);
    for i in 0..len {
        let byte = output[src_start + (i % distance)];
        output.push(byte);
    }
}
```

No circular indexing, no `LZ_DICT_INIT_POS` prefix, no `dict.full`
tracking. The output grows unboundedly with the decoded size.

## Why the divergence exists

The circular buffer is an optimisation: it caps memory use at
`dict_size + 576` bytes regardless of stream length. For
single-stream `.lzma` decode, the decoded size is known up front
(from the 8-byte uncompressed-size header field), so a growing
buffer wastes at most `dict_size - decoded_size` bytes (i.e. none
in practice, since `dict_size` is typically much larger than any
individual file's decoded size).

The circular buffer becomes important for LZMA2 multi-chunk
streams, where the dictionary must persist across chunks but the
total output can be much larger than `dict_size`. In that scenario,
the circular buffer is the only way to bound memory.

## Impact

- Memory use scales with decoded size, not `dict_size`. For a 1 GB
  decode with `dict_size = 64 MiB`, the Rust port uses 1 GB; the
  Ruby / C reference uses 64 MiB + 576 bytes.
- No functional difference — the decoded bytes are identical.
- The `Dictionary` struct in `omnizip-lzma/src/dictionary.rs`
  implements a circular buffer but is currently dead code (not
  wired into `Lzma1Decoder`).

## Reconciliation plan

When LZMA2 multi-chunk support lands (see
[compliance-lzma-single-stream-only.md](compliance-lzma-single-stream-only.md)),
switch `Lzma1Decoder` to use the existing `Dictionary` struct:

1. Replace `output: &mut Vec<u8>` with `dict: &mut Dictionary`.
2. Update `copy_match` to call `dict.copy_match(distance, length)`.
3. Update `literal_state` and the match-byte lookup to use
   `dict.byte_at_distance`.
4. Snapshot the dictionary at the end of decode for return to the
   caller.

The `Dictionary` API already supports this; the refactor is
mechanical.
