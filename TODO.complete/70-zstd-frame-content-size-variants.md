# 70 — ZSTD frame content size flag variants

## Gap

The frame encoder hard-codes `fcs_flag = 3` (8-byte Frame Content Size
field) and always emits a `Window_Descriptor`. This is the most
compatible encoding but wastes bytes on small frames:

- 1-byte FCS (fcs_flag = 0, content size ≤ 255): saves 7 bytes.
- 2-byte FCS (fcs_flag = 1, content size ≤ 65535): saves 6 bytes.
- 4-byte FCS (fcs_flag = 2, content size ≤ 2³²-1): saves 4 bytes.
- Single_Segment flag (no Window_Descriptor): saves 1 byte.

For LimniFS, every DropId frame is content-addressed and small frames
are common (config blobs, metadata). Choosing the smallest header
saves 5-8 bytes per frame, which adds up at scale.

## Implementation

1. In `block::write_frame_header`, pick the smallest FCS variant that
   fits.
2. Add `Single_Segment_flag` when content fits in one window (always
   true for inputs ≤ `window_size`).
3. Match the C reference's `ZSTD_writeFrameHeader` priority order.

## Test strategy

- Empty input → 1-byte FCS (0).
- 100-byte input → 1-byte FCS + single segment.
- 1000-byte input → 2-byte FCS + single segment.
- 1 MB input → 4-byte FCS + window descriptor.
- 10 GB input → 8-byte FCS + window descriptor.
- Round-trip through own decoder and `zstd -d`.
