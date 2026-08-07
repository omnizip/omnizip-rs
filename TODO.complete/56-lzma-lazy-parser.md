# Task 56: LZMA lazy parser

## Status: deferred — LZMA encoder works (valid XZ output, xz -d accepts). Ratio is suboptimal (84% vs reference 26%). These are ratio improvement TODOs.
## Priority: P0

## Problem

LZMA greedy parser produces output larger than ZSTD on text.
Lazy (look-ahead-1) parsing improves ratio by 5-10%.

## Plan

At position p:
  m1 = longest_match(p)
  m2 = longest_match(p + 1)
  if m2.len > m1.len + 1:
      emit_literal(byte_at_p)
      emit_match(m2) at p + 1
  else:
      emit_match(m1) at p
