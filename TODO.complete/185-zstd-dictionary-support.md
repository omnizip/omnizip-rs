# 185: ZSTD Dictionary Support

## Priority: P2 (feature completeness)

## Status: partial — trainer exists, encoder path incomplete

## Context

ZSTD dictionaries improve ratio on small, similar files (JSON logs,
protocol messages, etc.) by 10-50%. The decoder can already decode
dictionary-compressed frames. The encoder has a `compress_with_dict`
path and a `ZstdDictTrainer`, but:

1. The trainer may not produce dictionaries compatible with the C
   reference's `zstd --train`.
2. The encoder's dict path uses a simple prefix strategy (present the
   dict as a virtual prefix to the match finder) rather than the full
   ZSTD dictionary format (which includes a header + content + offset
   adjustments).

## Remaining work

### A. Dictionary format compliance

The ZSTD dictionary format (RFC 8478 §3.1.1):
```
Magic_Number (4 bytes) = 0xEC30A437
Dictionary_ID (4 bytes)
Entropy_Tables (variable)
Content (variable)
```

Our trainer may skip the entropy tables. Verify and fix.

### B. Encoder dictionary references

When the match finder finds a match into the dictionary prefix, the
offset encoding must account for the dictionary size. Verify the
current prefix strategy produces correct offsets.

### C. Differential test against `zstd --train`

1. Train a dict via `zstd --train samples/* -o dict`
2. Compress with `zstd -D dict input`
3. Compress with our encoder + the same dict
4. Both should decode to the same plaintext via `zstd -D dict -d`

## Files

- `omnizip-zstd/src/dict_trainer.rs`
- `omnizip-zstd/src/dict.rs`
- `omnizip-zstd/src/encoder/block.rs` (dict path)
- New: `tests/differential/tests/zstd_dict_parity.rs`

## Acceptance criteria

- [ ] Trained dicts are accepted by `zstd -D`
- [ ] Encoder with dict produces frames accepted by `zstd -D -d`
- [ ] Ratio improves ≥10% on a corpus of small similar files
- [ ] Round-trip via own decoder with dict
- [ ] Deterministic: same samples → same dict
