# 75 — omnizip-glza: grammar-based LZ compression

## Source
- Proposal: `../../limnifs/limnifs/docs/omnizip-glza-proposal.md`
- Format spec: Smith's `GLZA_format.md` (published)
- Academic: SA-IS (Nong et al., DCC 2009), Nevill-Manning & Witten 1997
- GPL-3 C reference — black-box testing ONLY

## Why
GLZA builds a context-free grammar over the input, replacing repeated
substrings with rule references. For data with hierarchical repetition
(DNA sequences, structured logs, XML), grammar-based coding beats LZMA
by 5-15%. DNA compresses to ~28% (GLZA) vs ~38% (LZMA-9).

## Architecture

1. Build a suffix array of the input (SA-IS algorithm).
2. Greedily extract the most profitable repeated substring as a new
   grammar rule, replacing all occurrences with rule references.
3. Repeat until no rule improves compression.
4. Entropy-code the grammar (start symbol + rules) with Huffman.

```
omnizip-glza/
  src/
    lib.rs              — public API + Codec trait
    suffix_array.rs     — SA-IS construction (~600 LOC)
    grammar.rs          — grammar construction + rule extraction (~500 LOC)
    rules.rs            — rule dedup/numbering (~200 LOC)
    encode.rs           — entropy-coded grammar stream (~500 LOC)
    decode.rs           — grammar expansion (~300 LOC)
    optimize.rs         — grammar pruning, LZ fallback (~400 LOC)
    codec.rs            — GlzaCodec struct
```

## Phased plan

| Phase | Scope | LOC | Gate |
|-------|-------|-----|------|
| 1 | Suffix array + grammar construction + naive Huffman | ~1500 | round-trips; DNA ≤ 30% |
| 2 | Entropy-coded grammar stream (wire format) | ~800 | DNA ≤ 30%, XML ≤ 12% |
| 3 | Optimization: pruning, parallel SA, memory cap | ~700 | DNA ≤ 25%, ≥ 1 MB/s |
| CI | Differential vs GPL GLZA CLI | ~200 | |
| **Total** | | **~3200** | |

## Acceptance criteria

1. DNA sample ≤ 30% of input (ref GLZA ~28%).
2. XML sample ≤ 12% of input (ref GLZA ~10%).
3. Round-trip identity.
4. Determinism.
5. Memory: grammar stays within `max_rules` cap.
6. `#![forbid(unsafe_code)]`.

## Codec ID
`0x0D` (assigned by LimniFS).

## Key constraint
SA-IS suffix array construction is O(n) time and space but complex to
implement correctly. The grammar extraction loop is the ratio
determinant — the greedy "most profitable rule" heuristic must balance
rule overhead (header bytes) against match savings.

## Routing heuristic
GLZA should be gated behind a structural-repetition detector — it's
not the default codec. Use it for inputs where grammar rules would
fire frequently (DNA, XML with repeated tags, structured logs).
