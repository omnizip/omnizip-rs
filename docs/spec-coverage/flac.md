# Spec Coverage — omnizip-flac

**Spec:** FLAC 1.4.0 format specification (https://xiph.org/flac/format.html)
**Last updated:** 2026-08-03

## Coverage matrix

| Section | Clause | Description | Status |
|---------|--------|-------------|--------|
| §2 | STREAM | "fLaC" magic | ✅ |
| §2 | STREAMINFO | min/max block size, sample rate, bps | ✅ |
| §3 | FRAME_HEADER | Sync code 0x3FFE | ✅ |
| §3 | FRAME_HEADER | Block size codes 1-15 | ✅ |
| §3 | FRAME_HEADER | Sample rate codes 0-14 | ✅ |
| §3 | FRAME_HEADER | Channel assignment 0-10 | ✅ |
| §3 | FRAME_HEADER | Sample size codes 0-7 | ✅ |
| §3 | FRAME_HEADER | UTF-8 frame number | ✅ |
| §3 | FRAME_HEADER | CRC-8 | ✅ |
| §3 | FRAME_FOOTER | CRC-16 (big-endian) | ✅ |
| §4 | CONSTANT | Type 0 subframe | ✅ |
| §4 | VERBATIM | Type 1 subframe | ✅ |
| §4 | FIXED | Types 8-12, orders 0-4 | ✅ |
| §4 | LPC | Types 32-63, orders 1-16 | ✅ |
| §4 | WASTED_BITS | Decoder supports; encoder always 0 | ❌ gap |
| §5 | RESIDUAL | RICE method (4-bit k) | ✅ |
| §5 | RESIDUAL | RICE2 method (5-bit k) | ❌ gap |
| §5 | RESIDUAL | Partition order 0-6 | ✅ |
| §5 | RESIDUAL | Exhaustive k-selection | ✅ |
| §6 | LPC | Autocorrelation + Levinson-Durbin | ✅ |
| §6 | LPC | Coefficient quantization + shift | ✅ |
| §6 | LPC | i32 wrapping (matches libFLAC) | ✅ |
| — | INTEROP | libFLAC CLI parity (6 fixtures) | ✅ |
| — | INTEROP | Multi-block-size sweep | ✅ |
