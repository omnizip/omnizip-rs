# 85 — Document convergent-encryption boundary

**Priority:** Low (informational) — **✅ DONE**
**Source:** RESEARCH.md §7 (Convergent encryption + dedup)

## Status

Landed in `docs/ARCHITECTURE.md` ("Convergent encryption boundary"
section). The doc explains:

- omnizip-rs is the codec layer; CE lives in the storage layer
  (LimniFS).
- `DropId = BLAKE3(plaintext)` is convergent in spirit.
- omnizip-rs never sees keys/IVs/tags — adding crypto here would
  violate layered design.
- References the Wiley 2024 CE survey paper.

## Original context

omnizip-rs is the **codec layer** for LimniFS, which is a
content-addressed FS using `DropId = BLAKE3(plaintext)`. Recent
academic work (Wiley 2024) surveys convergent encryption (CE)
schemes that combine dedup with confidentiality.

omnizip-rs does NOT do encryption. But the determinism invariant
(required for content addressing) is the same property CE schemes
rely on.

## Files

- `docs/ARCHITECTURE.md` — new "Convergent encryption boundary" section.
