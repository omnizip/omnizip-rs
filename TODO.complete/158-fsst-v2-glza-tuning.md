# TODO 158: FSST v2 + GLZA grammar tuning

## Problem

`omnizip-fsst` and `omnizip-glza` are working but their ratio
tuning knobs are unexplored. Both could close more ground vs the
reference implementations.

## Scope

**FSST (Fast Static Symbol Table)**:
- v2 wire format support.
- Symbol table size tuning (currently fixed at 256 entries).
- Escape handling for unusual byte patterns.

**GLZA (Grammar-based compression)**:
- Grammar size cap (currently unbounded for some inputs).
- Rule expansion strategy.
- Adaptive rule pruning.

## Acceptance criteria

- [ ] FSST v2 round-trips.
- [ ] GLZA grammar size bounded for adversarial inputs.
- [ ] Both pass differential tests vs reference tools.

## Priority

P2 — these are research codecs, not on the LimniFS critical path.
