# 73 — omnizip-zpaq: context-mixing archival codec

## Source
- Proposal: `../../limnifs/limnifs/docs/omnizip-zpaq-proposal.md`
- Spec: `zpaq.pdf` (public domain, Matt Mahoney)
- Academic: Mahoney DCC 2006/2009 context-mixing papers
- GPL-3 C++ reference — black-box differential testing ONLY

## Why
ZPAQ achieves the best archival compression ratio of any public codec
— 10-20% better than LZMA on enwik8. For LimniFS cold-storage archival
(rarely-accessed content-addressed blobs), ratio matters more than
speed. ZPAQ's `Best` level targets ≤ 15 MB on enwik8 vs LZMA's ~20 MB.

## Architecture

Context-mixing: multiple independent probability models (order-0,
order-1, word, match) feed a logistic mixer with SSE (Squash/Stretch),
which drives an arithmetic coder. The model configurations are
expressed as ZPAQ bytecode — a stack-based VM ISA that defines context
hash functions, mixer weights, and probability tables.

```
omnizip-zpaq/
  src/
    lib.rs              — public API + Codec trait
    arithmetic.rs       — binary arithmetic coder (800 LOC Phase 1)
    models.rs           — context models (order-0/1, word, match)
    mixer.rs            — logistic mixer + SSE
    vm.rs               — ZPAQ bytecode VM (ISA interpreter)
    container.rs        — multi-block segment container
    levels.rs           — level 1-5 standard configs
    codec.rs            — ZpaqCodec struct
```

## Phased plan

| Phase | Scope | LOC |
|-------|-------|-----|
| 1 | Arithmetic coder + order-2 model + minimal container | ~800 |
| 2 | Logistic mixer + SSE + multi-model + ZPAQ VM | ~1200 |
| 3 | Level 1-5 configs + multi-block + perf | ~1000 |
| CI | Differential vs GPL zpaq CLI | ~200 |
| **Total** | | **~3200** |

## Acceptance criteria

1. `compress(enwik8, Default)` ≤ 18 MB.
2. `compress(enwik8, Best)` ≤ 15 MB.
3. Round-trip identity for all inputs.
4. Determinism: same input+level → byte-identical output across runs.
5. Can decompress archives from GPL `zpaq` CLI.
6. `#![forbid(unsafe_code)]`.

## Codec ID
`0x0B` (assigned by LimniFS).

## Levels
- `Fast`: ~50 MB/s, ratio ~20% on enwik8.
- `Default`: ~5 MB/s, ratio ~18% on enwik8.
- `Best`: ~1 MB/s, ratio ~15% on enwik8.

## Key constraint
The ZPAQ VM is the hard part — it's a stack-based ISA with ~20 opcodes
that must be faithfully reimplemented from the public-domain spec.
Without the VM, level configs from the reference won't decode.
