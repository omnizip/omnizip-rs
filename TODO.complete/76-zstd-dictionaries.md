# 76 — ZSTD dictionary support

## Source
- Proposal: `../../limnifs/limnifs/docs/omnizip-zstd-dictionaries-proposal.md`
- Spec: RFC 8878 (Dictionary_ID field in frame header)
- Academic: COVER algorithm (Reformat et al., DCC 2015)
- BSD C reference — black-box testing ONLY

## Why
ZSTD's default encoder struggles with small inputs (< 100 KB) because
the Huffman/FSE tables are amortized over too few bytes. A
pre-trained dictionary provides a shared entropy table that dramatically
improves small-input ratio. For LimniFS config blobs and metadata,
dictionary-mode ZSTD can hit ≤ 50% of dictionary-less output.

## Architecture

1. **Trainer**: scan a corpus, extract the most valuable repeated
   substrings, assemble them into a dictionary blob (≤ 110 KiB).
2. **Encoder**: load the dictionary's entropy tables into the encoder
   state before compressing. Emit Dictionary_ID in the frame header.
3. **Decoder**: read Dictionary_ID, load the matching dictionary,
   decompress using the shared tables.

```
omnizip-zstd/src/
    dict.rs              — dictionary struct + serialization (~150 LOC)
    dict_trainer.rs      — top-K substrings trainer (~200 LOC Phase 1,
                           COVER trainer ~500 LOC Phase 2)
    encoder.rs           — add compress_with_dict entry point (~100 LOC)
    decoder.rs           — add decompress_with_dict (~100 LOC)
    frame/header.rs      — emit/parse Dictionary_ID field (~50 LOC)
```

## Phased plan

| Phase | Scope | LOC | Gate |
|-------|-------|-----|------|
| 1 | compress/decompress_with_dict + simple trainer | ~570 | JSON ≤ 50% of no-dict |
| 2 | COVER trainer (optimal) | ~500 | matches reference trainer quality |
| CI | Differential vs `zstd --train` / `zstd -D` | ~100 | |
| **Total** | | **~1070** | |

## Acceptance criteria

1. `compress_with_dict(json_corpus, dict)` ≤ 50% of `compress(json_corpus)`.
2. Round-trip: `decompress_with_dict(compress_with_dict(x, d), d) == x`.
3. Trained dictionary ≤ 110 KiB (ZSTD standard size).
4. No regression on existing ZSTD test suite.
5. `#![forbid(unsafe_code)]`.

## Dictionary wire format (Phase 1)

```text
Magic: \x37\xA4\x30\xEC (4 bytes)
Dictionary_ID: u32 LE (4 bytes)
EntropyTables:
  Huffman_Table (as in compressed literals)
  FSE tables for LL, ML, OF
  Repeat offsets (3 × u32)
Content:
  Raw sample bytes (the dictionary content for match finding)
```

## Key constraint
The frame header must include the Dictionary_ID field (currently
omitted — our encoder always sets dict_id_flag = 0). Phase 1 must add
this field and the decoder must look up the dictionary by ID.

## COVER algorithm (Phase 2)
Suffix-array-based: build suffix array of the corpus, find the most
frequent d-byte substrings, greedily select those that maximize
compression gain. The C reference's `COVER_best` does this in O(n log n).
