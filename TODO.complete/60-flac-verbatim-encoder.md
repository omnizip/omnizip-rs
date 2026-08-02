# 60 — FLAC encoder: VERBATIM subframe

## Gap

`omnizip-flac` can decode any FLAC stream (CONSTANT, VERBATIM, FIXED,
LPC, Rice residuals, CRC-8/CRC-16) but the encoder produces raw PCM
wrapped in a trivial container. Real FLAC output requires:

1. `fLaC` magic (4 bytes).
2. STREAMINFO metadata block (34 bytes, CRC-8 protected).
3. One or more frames, each containing subframes.

## Phase A: VERBATIM encoder (minimal viable FLAC)

VERBATIM stores samples directly — no prediction, no residual coding.
It's the FLAC equivalent of ZSTD's Raw literals. Output is larger than
the input (34-byte STREAMINFO + 6-byte frame header + 2-byte CRC +
uncompressed samples), but it round-trips through any FLAC decoder.

### Frame layout (VERBATIM)

```
Frame_Header (6+ bytes):
  sync code (0xFFF8 or 0xFFF9 for fixed block size)
  blocking strategy (0 = fixed)
  block size encoding (4-bit code → lookup table)
  sample rate encoding (4-bit code)
  channel assignment (4-bit)
  sample size encoding (3-bit)
  UTF-8 coded frame number
  (optional block size extra bytes)
  (optional sample rate extra bytes)
  CRC-8 of header bytes

Subframe:
  subframe header (1 byte):
    bit 0: reserved (0)
    bit 1-6: subframe type (0b000001 = VERBATIM)
    bit 7: reserved (0)
  unencoded samples (n × bits_per_sample bits)

Frame_Footer:
  CRC-16 of all frame bytes
```

### STREAMINFO layout

```
min_block_size (u32 BE, 3 bytes)
max_block_size (u32 BE, 3 bytes)
min_frame_size (u32 BE, 3 bytes)
max_frame_size (u32 BE, 3 bytes)
sample_rate (u32, 20 bits) | channels-1 (3 bits) | bps-1 (5 bits) | total_samples (u64, 36 bits)
MD5 of unencoded audio data (16 bytes)
```

## Implementation

1. `omnizip-flac/src/encoder.rs` — top-level `encode(pcm, params)`.
2. `omnizip-flac/src/encoder/streaminfo.rs` — build STREAMINFO block.
3. `omnizip-flac/src/encoder/frame.rs` — frame header + footer writer.
4. `omnizip-flac/src/encoder/subframe.rs` — VERBATIM subframe writer.
5. Wire `encode` into `FlacCodec::compress`.

## Test strategy

- Encode PCM at 44.1 kHz / 16-bit / mono. Verify `fLaC` magic,
  STREAMINFO fields, frame structure.
- Decode via own decoder → assert round-trip.
- Decode via libFLAC (reference) if available.
