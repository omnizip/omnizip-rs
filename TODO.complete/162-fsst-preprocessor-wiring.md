# TODO 162: FSST preprocessor wiring for CSV/text workloads

## Problem

`omnizip-fsst` exists and is registered as `CODEC_FSST_BROTLI = 0x09`.
LimniFS suspects FSST preprocessing before Brotli could beat plain
Brotli on CSV/text workloads by 10-20%. The wiring hasn't been
audited.

## Scope

FSST (Fast Static Symbol Table) builds a 256-entry symbol table
from the input, then encodes the input as a sequence of symbol
references. Composing with Brotli:

```text
input → FSST encode → Brotli compress → output
```

The Brotli side compresses the FSST-encoded byte stream (mostly
single-byte symbol references), which compresses better than the
original because the entropy is lower.

## Implementation plan

1. Verify `Codec::compress` on `CODEC_FSST_BROTLI` actually does
   FSST-then-Brotli composition (not just FSST alone).
2. Add benchmarks on:
   - CSV logs
   - JSON API responses
   - Enwik-like text
3. Compare ratio + speed vs:
   - Plain Brotli
   - LZMA
   - ZSTD
4. Document the workload where FSST preprocessing wins.

## Acceptance criteria

- [ ] `CODEC_FSST_BROTLI` confirmed to compose FSST + Brotli.
- [ ] Bench shows ≥ 5% ratio improvement vs plain Brotli on at
  least one CSV/text workload.
- [ ] Documented in `docs/fsst-composition.md`.

## Priority

P2 — speculative; the 5% target may not materialise.
