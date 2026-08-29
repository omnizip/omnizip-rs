# Task 11: deflate level tiers (omnizip-libdeflate)

## Status: pending

## Problem

`omnizip-libdeflate::compress` ignores the compression level entirely
(`let _ = level;`): it always runs one dynamic-vs-fixed-vs-stored
contest. That already beats zlib -1 everywhere (0.82-1.02x) but trails
zlib -9 by 2-7% on text (rfc 1.071x, words 1.025x, arial 1.020x).

## Action

Implement tiered LZ77 in the from-spec encoder: greedy (fast levels),
lazy-1 (mid), full lazy chain (level 9), matching zlib's strategy
tiers. The dynamic/fixed/stored contest stays as the final emission
step.

## Acceptance

- Levels 1/6/9 produce different outputs with monotone sizes
- lv9 within 1.02x of `zlib -9` on the broad corpus
