# 244 — Brotli Decoder Wire-Format Bugs

- **Priority:** P0 (blocks TODO 228, 229, 232, 242)
- **Crate:** `omnizip-brotli` (`decoder_full.rs`)
- **Depends on:** none
- **Estimated effort:** 3-5 days
- **Blocks:** [228](228-brotli-block-type-switching.md) (block switch),
  [229](229-brotli-smart-context-clustering.md) (smart clustering),
  [232](232-zstd-fse-mode-selection.md) (FSE mode),
  [242](242-block-split-huffman.md) (block split)

## Problem

Three encoder features have infrastructure complete and ready to ship,
but are disabled because the decoder rejects the output. Each can be
reproduced by toggling the corresponding boolean in
`from_spec_encoder.rs:encode_huffman_chunk_into` and running
`cargo test -p omnizip-brotli`.

### Bug A: Block type switching (`use_block_switch = true`)

When the encoder emits multiple literal block types (NBLTYPESL > 1)
via `write_block_type_trees`, the decoder diverges in
`decode_compressed_metablock_full`. Symptoms:

- "metablock overran mlen" on text inputs ≥ 4 KiB
- "invalid literal" on shorter inputs
- The first block-switch command is decoded correctly; subsequent
  switches diverge by 1-3 bits.

Hypothesis: the decoder's `BlockTypeState` advances the block counter
on the wrong side of the literal/context-map indexing. The encoder
writes NBLTYPES *before* the per-block-type context-mode fields (RFC
7932 §9.3) but the decoder reads them in the opposite order in one
branch.

### Bug B: Smart context clustering (data-dependent ctx_map)

The fixed `ctx >> 4` split (4 trees, contexts 0-15 → tree 0, etc.)
works end-to-end. Replacing it with a clustering-derived map (e.g.,
`cluster_contexts()` output) produces corrupted literals even when
the map is bit-reversed to match the decoder's Huffman code
interpretation. Symptoms:

- Decoded bytes match for the first ~256 literals, then diverge.
- The context IDs at the divergence point look correct (computed
  manually from the previous two bytes).
- The Huffman trees decode to plausible-but-wrong symbols.

Hypothesis: the decoder uses a different `dist_context_map` indexing
than the encoder assumes. Specifically, when NTREESL > 1 and the
context map has non-contiguous tree indices, the decoder may apply
the bit-reversal at the wrong layer.

### Bug C: Static dictionary references

The decoder's `dictionary_lookup` (in `decoder_full.rs`) accepts
only the length-preserving transform paths. Length-changing
transforms (prefix/suffix/omit/uppercase) emit valid `dictionary_lookup`
calls per RFC 7932 §11, but the decoder rejects them with
"static dictionary not supported".

Hypothesis: the decoder's `dictionary_lookup` doesn't apply the
suffix/prefix transform to the looked-up word, so the output bytes
don't match what the encoder simulated.

## Reproduction

```bash
# Bug A
sed -i '' 's/let use_block_switch = false;/let use_block_switch = true;/' \
    omnizip-brotli/src/from_spec_encoder.rs
cargo test -p omnizip-brotli --release

# Bug B
# In encode_huffman_chunk_into, replace the fixed ctx>>4 split with
# the output of cluster_contexts() from encoder/context.rs

# Bug C
# Already reproduces in tests::dictionary_transform_helps_mixed_case
# when extended to assert vendored decoder interop.
```

## Design

For each bug:

1. **Capture the exact wire bytes** emitted by the encoder.
2. **Decode them by hand** following RFC 7932 step-by-step.
3. **Identify the divergence point** in `decoder_full.rs`.
4. **Add a fixture-based test** that round-trips through BOTH the
   from-spec decoder AND `brotli -d` from the C reference.
5. **Fix the decoder**, verify all 86 brotli tests still pass.

### Test scaffolding

Add `tests/brotli_wire_format_conformance.rs` with fixtures:

- `block_switch_simple.txt` (4 KiB text, 2 literal block types)
- `cluster_ctx_map.csv` (CSV with skewed byte distribution)
- `dict_transform_length_changing.txt` (text with words that benefit
  from prefix/suffix transforms)

Each fixture is encoded by our encoder, then decoded by:
- Our `decoder_full.rs`
- `brotli -d` (C reference, subprocess)

Both must produce byte-identical output.

## Acceptance criteria

- [ ] Bug A fixed: `use_block_switch = true` round-trips on all
      text fixtures ≥ 4 KiB.
- [ ] Bug B fixed: data-dependent `cluster_contexts()` output
      round-trips.
- [ ] Bug C fixed: length-changing dictionary transforms round-trip
      via vendored C decoder.
- [ ] `cargo test -p omnizip-brotli --release` — all 86+ tests pass.
- [ ] Differential parity with `brotli -d` on the new fixtures.

## Why this matters

These three decoder bugs are the single biggest blocker to closing
the Brotli ratio gap. Each disabled feature costs 1-3 ratio points
on text-heavy inputs. Together they explain most of the residual gap
to the vendored C reference.
