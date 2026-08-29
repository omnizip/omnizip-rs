# Task 12: lz4 fast-tier ratio on low-redundancy data

## Status: pending

## Problem

lz4_flex fast tier vs `lz4 -1`: 0.92-0.99x on most corpora but
1.227x on arial.ttf (23 MB font) and 1.098x on rfc.txt. The HC tier
is 1.00-1.04x (fine). lz4_flex's default acceleration appears to
accept shorter matches on low-redundancy data than the C.

## Action

Measure with explicit acceleration parameters (level → acceleration
mapping) in the codec layer; if lz4_flex cannot reach parity, port
the C's fast loop (LZ4_compress_generic) with its match-acceptance
rules.

## Acceptance

- arial.ttf and rfc.txt within 1.02x of `lz4 -1`
