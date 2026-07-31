# 10 — ZSTD frame format

A ZSTD file is one or more frames. Each frame contains a header, one or
more blocks, and optional checksum.

## Frame structure

```text
 Magic_Number (4 bytes, LE) = 0xFD2FB528
 Frame_Header (variable: 2–14 bytes)
 Block_1
 Block_2
 ...
 Block_N (last block flag set)
 [Content_Checksum (4 bytes, LE, optional)]
```

## Magic number

```text
 0x28 0xB5 0x2F 0xFD    (little-endian u32 = 0xFD2FB528)
```

MUST be present at the start of every frame. A decoder MAY seek past
padding (zero bytes) before the magic in streaming mode.

## Frame header

```text
  Byte 0 (descriptor):
    bit 7-6: Frame_Content_Size_flag (FCS_flag)
    bit 5:   Single_Segment_flag
    bit 4:   Unused (MUST be 0)
    bit 3:   Reserved (MUST be 0)
    bit 2:   Content_Checksum_flag
    bit 1-0: Dictionary_ID_flag (DID_flag)
```

### FCS_flag → Frame_Content_Size field

| FCS_flag | Field width | Notes |
|---|---|---|
| 0 | 0 bytes (if Single_Segment_flag = 0) or 1 byte (if Single_Segment_flag = 1) | FCS absent or 1 byte |
| 1 | 2 bytes | value + 256 |
| 2 | 4 bytes | raw u32 LE |
| 3 | 8 bytes | raw u64 LE |

When `FCS_flag = 0` and `Single_Segment_flag = 1`, the FCS is 1 byte
(stored in the byte immediately after Window_Descriptor, value 0–255).

### Single_Segment_flag

- `0`: the frame uses a Window_Descriptor byte.
- `1`: no Window_Descriptor; the entire frame is a single segment. The
  window size equals the Frame_Content_Size.

### Content_Checksum_flag

- `1`: a 4-byte xxHash32 checksum follows the last block.
- `0`: no checksum.

### DID_flag → Dictionary_ID field

| DID_flag | Field width |
|---|---|
| 0 | 0 bytes (no dictionary) |
| 1 | 1 byte |
| 2 | 2 bytes |
| 3 | 4 bytes |

### Window_Descriptor (absent if Single_Segment_flag = 1)

```text
  Byte:
    bit 7-3: Exponent (5 bits)
    bit 2-0: Mantissa (3 bits)

  windowSize = (1 << exponent) + (mantissa * (1 << (exponent - 3)))
  windowLog  = 10 + exponent  (minimum windowLog = 10)
```

Valid range: `windowSize` MUST be in `[1 KiB, 1 TiB]` (ZSTD 0.5+). The
decoder rejects window sizes larger than its configured `maxWindowSize`.

### Window_Size (effective)

```text
  if Single_Segment_flag:
    effective_window = Frame_Content_Size
  else:
    effective_window = window_descriptor_value
```

The decoder must allocate at least `effective_window` bytes for the
sliding window.

## Frame header example (common case)

```text
  fd 2b b5 28     ← magic
  a0               ← descriptor: FCS=2 (4-byte), Single_Segment=1,
                     Unused=0, Reserved=0, Checksum=0, DID=0
                     Actually: bit 7-6 = 10, bit 5 = 1 → 0xA0
  00 10 00 00     ← Frame_Content_Size = 4096 (4 bytes LE)
```

No Window_Descriptor (Single_Segment = 1). No dictionary. Total header:
1 + 4 = 5 bytes.

## Block

Each block starts with a 3-byte header:

```text
  Byte 0 (low bits of size):
    bit 0: Last_Block flag
    bit 1-2: Block_Type
    bit 3-7: Block_Size low 5 bits

  Byte 1: Block_Size bits 5-12
  Byte 2: Block_Size bits 13-20
```

| Block_Type | Name | Block_Size meaning |
|---|---|---|
| 0 | Raw_Block | Uncompressed data, Block_Size bytes follow |
| 1 | RLE_Block | Single byte repeated Block_Size+1 times (1 data byte follows) |
| 2 | Compressed_Block | Compressed data (see spec 11) |
| 3 | Reserved | MUST be rejected |

Block_Size for Compressed_Block MUST NOT exceed the minimum of
`windowSize` and `3 * windowSize / 2`. The decoder rejects blocks
exceeding this limit.

## Cross-references

- Ruby: `omnizip/lib/omnizip/algorithms/zstandard/frame/header.rb`
- Ruby: `omnizip/lib/omnizip/algorithms/zstandard/frame/block.rb`
- Spec: RFC 8878 §3 (Frame)
- C: `zstd/lib/decompress/zstd_decompressBlock.c`
- Rust port: `omnizip-zstd/src/frame/header.rs` (pending)
