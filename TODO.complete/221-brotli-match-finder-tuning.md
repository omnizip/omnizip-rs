# 221 — Brotli Quality-Dependent Match Finder Tuning

- **Priority:** P2 (moderate ratio win across all quality levels)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 0.5 days

## Goal

Tune hash-chain depth (`max_chain`) and `nice_match` per quality level
to match the C reference's parameters more closely.

## Current parameters

```
Q0-1:  max_chain=4,   nice_match=8
Q2-3:  max_chain=16,  nice_match=16
Q4-5:  max_chain=32,  nice_match=32
Q6-7:  max_chain=64,  nice_match=64
Q8-9:  max_chain=128, nice_match=128
Q10-11: max_chain=256, nice_match=271
```

## C reference parameters (for reference)

```
Q0-2:  max_chain=0 (single probe), nice_match=8
Q3-4:  max_chain=4,   nice_match=16
Q5-6:  max_chain=16,  nice_match=32
Q7-8:  max_chain=32,  nice_match=64
Q9:    max_chain=64,  nice_match=128
Q10:   max_chain=128, nice_match=271
Q11:   max_chain=4096, nice_match=271
```

The C reference uses much deeper chains at high quality. Our Q10-11
max_chain=256 is 16× shallower than C's Q11 max_chain=4096.

## Plan

1. Increase hash_log from 16 to 17 (128K entries) for Q5+
2. Adjust max_chain to match C reference more closely
3. Add hash_log parameter to `HashChainConfig` instead of hardcoding 16

## Acceptance criteria

- [ ] hash_log scales with quality (16 for Q0-4, 17 for Q5-9, 18 for Q10-11)
- [ ] max_chain matches C reference within 2× at each quality tier
- [ ] No speed regression at Q1-3
- [ ] Ratio improvement at Q10-11 on repetitive inputs
