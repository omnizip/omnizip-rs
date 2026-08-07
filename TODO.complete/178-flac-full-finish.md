# 178: FLAC — Full Finish

## Priority: P2 (feature completeness)

## Status: documented — encoder + decoder work, remaining work is ratio + format coverage.

## Context

The FLAC codec implements:
- PCM header parsing (WAV, AIFF)
- LPC encoder with autocorrelation estimation
- Rice residual coding with parameter optimization
- Verbatim and fixed-predictor modes
- Full frame/subframe decode

All round-trip tests pass. The encoder produces valid FLAC streams
accepted by `flac -d`.

## Current encoder modes

| Mode      | Order | When used                    |
|-----------|-------|------------------------------|
| Verbatim  | N/A   | Incompressible audio         |
| Fixed     | 0-4   | Simple signals, low overhead |
| LPC       | 1-32  | Complex audio, best ratio    |

## Remaining work

### A. Mid-side stereo encoding (TODO 71)

**Problem**: Stereo audio is encoded as two independent channels. FLAC
supports mid-side (MS) stereo which can save 5-15% on correlated
stereo (most music).

**Fix**: Add channel decorrelation modes:
- `INDEPENDENT` (current)
- `MID_SIDE` (left+right, left-right)
- `LEFT_SIDE` (left, left-right)
- `RIGHT_SIDE` (right, left-right)

The encoder tries each mode per frame and picks the best.

**Files**: `encoder/subframe.rs`, `encoder/frame.rs`

### B. Block-size auto-selection (TODO 111)

**Problem**: The encoder uses a fixed block size (4096 samples). The
optimal block size depends on the audio content:
- tonal music: larger blocks (8192-16384) for better LPC prediction
- transient-heavy: smaller blocks (192-960) to limit damage

**Fix**: Try 2-3 block sizes per file and pick the one that produces
the smallest output.

**Files**: `encoder/mod.rs`

### C. FFT-based autocorrelation (TODO 112)

**Problem**: The LPC autocorrelation is O(N*max_order). For large
blocks and high orders, this dominates encode time.

**Fix**: Use the FFT-based autocorrelation (O(N log N)) when
`block_size * max_order > threshold`. The `fft-acf` feature flag
already exists but is off by default.

**Files**: `encoder/fft.rs` (enable and verify)

### D. LPC coefficient quantization refinement

**Problem**: The current LPC encoder quantizes coefficients to a fixed
precision. The C reference (`flac_encoder.c`) tries multiple
precisions (4-12 bits) and picks the best.

**Fix**: Add precision search in the LPC encoder.

**Files**: `encoder/lpc.rs`

### E. Seek table + metadata blocks

**Problem**: The encoder writes a minimal STREAMINFO block. Full FLAC
files benefit from:
- SEEKTABLE for random access
- VORBIS_COMMENT for tags
- PICTURE for album art

**Fix**: Add optional metadata block writers.

**Files**: New `encoder/metadata.rs`

## Acceptance criteria

- [x] All round-trip tests pass (86 FLAC tests)
- [x] `flac -d` accepts all encoder output
- [ ] Mid-side stereo encoding (5-15% ratio improvement on music)
- [ ] Auto block-size selection
- [ ] FFT autocorrelation enabled by default for large blocks
- [ ] LPC precision search
- [ ] Optional SEEKTABLE + VORBIS_COMMENT metadata
