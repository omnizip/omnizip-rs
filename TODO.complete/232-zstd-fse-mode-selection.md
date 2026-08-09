# 232 — ZSTD FSE Mode Selection Improvement

- **Priority:** P2 (ratio win via better entropy coding)
- **Crate:** `omnizip-zstd`
- **Depends on:** [210](210-zstd-optimal-fse.md)
- **Estimated effort:** 1 day

## Goal

Make the FSE table mode selection more aggressive about using FSE_Compressed
mode when it produces smaller output than Predefined mode. Currently
conservative: always uses Predefined when all symbols have non-zero default
norm.

## Background

ZSTD sequence sections encode LL (literal length), ML (match length), and
OF (offset) symbols using FSE tables. Three modes:
- Predefined: fixed norm tables (no table description needed)
- RLE: single symbol repeated (1-byte description)
- FSE_Compressed: custom norm table (variable-size description)

The current `choose_table_mode` only uses FSE_Compressed when Predefined
isn't viable. But FSE_Compressed can be smaller even when Predefined works,
especially when the symbol distribution differs significantly from the
predefined table's assumptions.

## Plan

1. For each symbol type (LL, ML, OF), compute:
   a. Predefined cost: sum of -log2(predefined_norm[sym]) * count[sym]
   b. FSE_Compressed cost: optimal FSE table cost + sum of -log2(optimal_norm[sym]) * count[sym]
2. Pick the mode with lower total cost
3. The FSE_Compressed table description size must be included in the cost

## Acceptance criteria

- [ ] FSE_Compressed mode selected when it saves bits vs Predefined
- [ ] Table description overhead correctly accounted for
- [ ] Ratio improvement >= 0.5% on inputs with skewed symbol distributions
- [ ] No regression on inputs where Predefined is already optimal
- [ ] Determinism preserved (same input → same mode selection)
