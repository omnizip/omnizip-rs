# omnizip-brotli

Pure-Rust Brotli codec (RFC 7932) — no external dependencies.

## Status

**Phase D landed.** Frame header parser, metablock header parser,
and uncompressed-metablock decoder + encoder all working end-to-end.
The upstream `brotli` crate dependency has been removed entirely.

The encoder currently produces **uncompressed metablocks** (wire-format
correct, zero compression). Huffman-coded literal metablocks + LZ77
back-references are tracked as TODO 168 (Phase C.3a — static tree
path).

## What works

- Decode any RFC 7932 uncompressed metablock.
- Encode arbitrary input as uncompressed Brotli.
- Cross-compat: our encoder output decodes via `brotli -d` (verified
  by the differential test `brotli_round_trips_through_reference_cli`).
- Round-trip via our in-house decoder for all property-based test
  fixtures.

## What doesn't work yet

- Decoding Huffman-coded metablocks (returns
  `Err("huffman-coded metablock not yet supported")`).
- Encoding with actual compression (output size ≈ input size + 5
  bytes overhead).

## Usage

```rust
use omnizip_brotli::BrotliCodec;
use omnizip_codecs::{Codec, CompressionLevel};

let codec = BrotliCodec::new();
let input = b"hello world".repeat(100);
let compressed = codec.compress(&input, CompressionLevel::default()).expect("compress");
let decompressed = codec.decompress(&compressed, input.len() as u32).expect("decompress");
assert_eq!(decompressed, input);
```

## Architecture

```text
src/
├── lib.rs            — BrotliCodec + BrotliOptions + BrotliMode
├── decoder.rs        — BitReader, parse_frame_header, parse_metablock_header,
│                      parse_block_type_header, parse_distance_header,
│                      HuffmanTable, decode() orchestrator
├── encoder.rs        — encode_uncompressed (Phase D), BitWriter
├── encoder_error.rs  — EncodeError type
└── dictionary.rs     — Static dictionary + transforms (RFC 7932 §10.4)
                      — 120 transforms landed; 121st + full dict lands
                      with TODO 151
```

## Wire format (RFC 7932 §9)

```text
Frame header (WBITS):
  1 bit  → window_bits = 16
  4 bits → window_bits = 17 + NBL (NBL = 1..7)
  7 bits → window_bits = 8 + N2   (large-window extension, NBL=0)

Metablock header (§9.2):
  1 bit  → ISLAST
  if ISLAST:
    1 bit → ISLASTEMPTY
    if ISLASTEMPTY:
      END (no body)
    else:
      2 bits → MNIBBLES (0 → 4)
      4*MNIBBLES bits → MLEN (encoded value = mlen - 1)
      1 bit → IS_UNCOMPRESSED
      1 bit → reserved (must be 0)
  else:
    2 bits → MNIBBLES
    4*MNIBBLES bits → MLEN
    1 bit → IS_UNCOMPRESSED
    1 bit → reserved

If IS_UNCOMPRESSED:
  Byte-align, then `mlen` raw bytes.

Else (Huffman-coded — TODO 168):
  Block-type headers + distance header + context modes + Huffman trees
  + Huffman-coded data.
```

## Determinism

`BrotliCodec::compress` is deterministic: same input always produces
identical bytes. Verified by the workspace-wide determinism audit
(`tests/determinism/`).

## License

MIT OR Apache-2.0.