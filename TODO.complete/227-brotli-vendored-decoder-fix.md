# 227 — Vendored Brotli Decoder Bug Fix

- **Status:** DONE (WBITS threaded from frame header through all
  decode paths; dictionary lookup uses correct max_backward_distance)
- **Priority:** P1 (correctness — vendored path unusable as fallback)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 1 day

## Problem

The vendored C reference encoder (`fast_encoder::vendored_compress`) produces
output that our decoder rejects with "invalid back-reference distance". This
makes the vendored path unusable as a fallback for high-ratio scenarios where
the from_spec encoder underperforms.

The bug manifests on ALL inputs larger than a few bytes. The vendored
encoder's distance coding doesn't match our decoder's expectations.

## Root cause hypotheses

1. **Distance code range mismatch**: The vendored encoder may use distance
   codes outside our decoder's expected range (e.g., NPOSTFIX/NDIRECT
   configuration differences).

2. **Dictionary reference encoding**: The vendored encoder uses dictionary
   references with specific transform indices. Our decoder's
   `dictionary_lookup` may reject valid references.

3. **Window size mismatch**: The vendored encoder may use a different WBITS
   than our decoder expects.

## Plan

1. Capture a minimal failing case (vendored encode → our decoder).
2. Hex-dump the compressed bytes and trace through the decoder to find
   the exact instruction that fails.
3. Compare against the C reference decoder's behavior on the same input.
4. Fix the divergence in our decoder.

## Acceptance criteria

- [ ] Vendored encoder output round-trips through our decoder
- [ ] All existing brotli crate test vectors decode correctly
- [ ] `compress_with_options` produces decoder-valid output
