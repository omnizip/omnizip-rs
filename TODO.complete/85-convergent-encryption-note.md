# 85 — Document convergent-encryption boundary

**Priority:** Low (informational)
**Source:** RESEARCH.md §7 (Convergent encryption + dedup)

## Context

omnizip-rs is the **codec layer** for LimniFS, which is a
content-addressed FS using `DropId = BLAKE3(plaintext)`. Recent
academic work (Wiley 2024) surveys convergent encryption (CE)
schemes that combine dedup with confidentiality.

omnizip-rs does NOT do encryption. But the determinism invariant
(required for content addressing) is the same property CE schemes
rely on. Documenting this prevents confusion:

- Users asking "is omnizip-rs secure?" — answer: it's deterministic,
  not encrypted. Encryption is LimniFS's responsibility.
- Security researchers looking at CE should know omnizip-rs is the
  layer below.

## Action

Add a short paragraph to:

- `README.md` — under "Security" section
- `CLAUDE.md` — under "Invariants"

Documenting:

1. omnizip-rs provides deterministic compression only.
2. Content addressing is the consumer's responsibility (LimniFS).
3. Encryption (CE or otherwise) is the consumer's responsibility.
4. The determinism invariant makes omnizip-rs compatible with CE
   schemes — same plaintext always produces same compressed bytes,
   which always hashes to the same `DropId`.

## Files

- `README.md` — add Security section
- `CLAUDE.md` — expand Invariants section
