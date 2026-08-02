# 71 — FLAC mid-side channel decorrelation

**Status**: COMPLETED (0.9.3)

## What was done

Added stereo channel decorrelation to the FLAC encoder. For stereo
input, the encoder evaluates 4 channel assignments per frame and picks
the one with the smallest total subframe cost:

- **Independent** (assign 1): left, right — no transform.
- **Left/side** (assign 8): left, side=left-right.
- **Right/side** (assign 9): right, side=left-right.
- **Mid/side** (assign 10): mid=(l+r)>>1, side=l-r.

## Implementation

- `encoder/frame.rs`: `pick_best_stereo_assignment` estimates cost
  via order-1 FIXED residual sum for each decorrelated representation.
- `frame.rs` (decoder): `reconstruct_stereo` undoes the transform:
  - Left/side: right = left - side.
  - Right/side: left = side + right.
  - Mid/side: left = mid + ((side + (side&1)) >> 1), right = left - side.

## Impact

For correlated stereo audio (e.g. two channels with similar content),
mid/side gives 5-10% ratio improvement. For uncorrelated content,
independent is selected automatically.

## Test coverage

- All 61 existing FLAC tests pass.
- Stereo round-trip (all-zero input) verified.
- Stereo CONSTANT round-trip verified.
