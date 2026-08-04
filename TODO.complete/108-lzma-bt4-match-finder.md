# 108 — LZMA BT4 match finder for optimal parser

**Priority:** P1 — MEDIUM
**Source:** Performance audit (2026-08-04)
**Status:** ⏳ Pending

## Problem

`encoder/match_finder.rs` uses hash chains with `max_chain_length=256`.
Reference LZMA uses BT4 (binary tree with 4 children) for levels 4+.
Hash chains find shorter matches on inputs with many similar contexts
(large source files, repetitive data).

**Impact:** 3-8% ratio gap vs `xz -6` on text.

## Proposed fix

Add a BT4 match finder alongside the existing hash chain. The optimal
parser already calls `find_match()` per position — swap the underlying
data structure.

## Acceptance criteria

- [ ] BT4 module exists in `encoder/bt4.rs`
- [ ] Optimal parser uses BT4 when `level >= 6`
- [ ] Round-trip preserved
- [ ] Calgary book1 ratio improves by ≥3% vs current

## Effort estimate

2-3 days.
