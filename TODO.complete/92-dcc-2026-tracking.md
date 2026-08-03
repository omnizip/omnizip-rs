# 92 — Track DCC 2026 proceedings quarterly

**Priority:** Low (process)
**Source:** RESEARCH.md §15 (DCC 2026)

## Context

The Data Compression Conference (DCC) is the premier compression
venue. DCC 2026 (Snowbird, Utah, March 24–27, 2026) papers cover
volumetric data compression, point cloud geometry, LZ77 / ANS /
arithmetic coding improvements — work that often becomes the next
RFC or codec standard.

This is **process work**, not a code change. We just need a
recurring reminder to review the proceedings.

## Action

Quarterly (every January / April / July / October):

1. Visit the IEEE DCC proceedings page for the most recent year.
2. Skim paper titles for relevance to omnizip-rs:
   - LZ77/LZMA match-finder improvements → omnizip-lzma
   - ANS/FSE improvements → omnizip-zstd
   - PPM/context-mixing improvements → omnizip-ppmd, omnizip-zpaq
   - Entropy coding (Huffman, arithmetic) → all codecs
3. For any directly-applicable paper, file a TODO in this directory
   with: paper URL, summary, fit analysis (high/medium/low), proposed
   enhancement.

## DCC proceedings

- DCC 2026: IEEE, ISBN 979-8-3315-8261-6
- DCC 2027 (when published): track via
  https://signalprocessingsociety.org/events/calendar

## Acceptance criteria

- [ ] Standing reminder in the project calendar (or personal) to
      review DCC proceedings every quarter.
- [ ] First review of DCC 2026 proceedings completed.
- [ ] Any directly-applicable papers filed as new TODOs (with the
      `n-` numbering continuing from 93+).
