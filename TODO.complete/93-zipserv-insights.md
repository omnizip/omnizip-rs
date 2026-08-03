# 93 — Track ZipServ GPU insights for future SIMD work

**Priority:** Low (process / informational)
**Source:** RESEARCH.md §13 (arXiv:2603.17435, ASPLOS 2026)

## Context

ZipServ (ASPLOS 2026, HKUST Guangzhou) is the first hardware-aware
lossless compression framework co-designed for LLM inference on
GPUs. It reduces model size by up to 30% while *accelerating*
inference by aligning decompression with the GPU memory hierarchy.

omnizip-rs targets general-purpose CPUs only — ZipServ itself is
not applicable. But the **architectural insight** (compression
designed for the hardware decompressor) is directly applicable to
our future SIMD work:

- Just as ZipServ tunes compressed layout for GPU warp-aligned
  access, we should tune for CPU SIMD lane width when adding
  `std::simd` paths (TODO 82 — ✅ done for CRC-32; TODO 83 —
  pending for Huffman decode).
- ZipServ's "decompress-on-demand into shared memory" pattern maps
  to our random-access use case (LimniFS content-addressed FS).

## Action

Informational — no code change. Two follow-ups to keep in mind:

1. When implementing TODO 83 (SIMD Huffman), align Huffman table
   layout with `std::simd` lane width (16 bytes for `u8x16`). The
   ZipServ paper's section on "layout-aware compression" is a
   useful design template.
2. When implementing random-access sub-block decompression for
   LimniFS, reference ZipServ's on-demand decompression pattern.

## Reference

- Fan, R. et al. (2026). *ZipServ: Fast and Memory-Efficient LLM
  Inference with Hardware-Aware Lossless Compression.* ASPLOS 2026.
  arXiv:2603.17435.
- Code: https://github.com/HPMLL/ZipServ_ASPLOS26

## Acceptance criteria

- [ ] ZipServ cited in TODO 83 (SIMD Huffman) as design inspiration.
- [ ] (Future) If omnizip-rs ever adds GPU support, revisit ZipServ
      as the algorithmic baseline.
