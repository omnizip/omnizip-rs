# 266 — Multi-Codec Ensemble Auto-Selection

- **Priority:** P3 (smart default codec per input)
- **Crate:** workspace (`omnizip-bench` or new `omnizip-ensemble`)
- **Depends on:** [256](256-encoder-profile-auto-detection.md), [261](261-codec-capability-metadata.md)
- **Estimated effort:** 3 days

## Problem

Today the caller picks the codec: `BrotliCodec` for text, `Lz4FastCodec`
for hot writes, `ZstdCodec` for balanced. This requires the caller to
know:
- What content type the input is (text/binary/structured)
- What tradeoff they want (speed vs. ratio)
- Which codec best fits that combination

Many callers don't know — they guess. Result: suboptimal ratio or
speed for the data they actually have.

## Design

### Ensemble picker

```rust
/// Picks the best codec for `input` based on `goal` and content
/// detection.
pub fn pick_best(
    input: &[u8],
    goal: Goal,
    registry: &CodecRegistry,
) -> &'static dyn Codec {
    let content = ContentType::detect(input);
    let caps_filter = |c: &Capabilities| c.content_type_aware || !content.is_text_like();
    match goal {
        Goal::Fast => pick_fast(registry, caps_filter),
        Goal::Balanced => pick_balanced(registry, caps_filter),
        Goal::MaxRatio => pick_max_ratio(registry, caps_filter),
    }
}

pub enum Goal {
    Fast,
    Balanced,
    MaxRatio,
}
```

### Quick-taste heuristic

For inputs >= 4 KiB, run each candidate codec on the first 4 KiB,
measure ratio + speed, project to full input, pick the winner.

For smaller inputs, use heuristic:
- Text/Structured + Balanced → Brotli Q5
- Binary + Balanced → ZSTD L9
- Binary + Fast → LZ4 Fast
- Text + MaxRatio → Brotli Q11 or LZMA L9

### Decision tree

```
                  input
                    |
           ContentType::detect
            /       |       \
         Text  Structured   Binary
           |       |          |
        Brotli  ZSTD         LZ4
        (dict)  (balanced)  (fast)
```

## Acceptance criteria

- [ ] `pick_best(input, goal, registry)` returns a codec.
- [ ] Heuristic mode: O(1) decision per input.
- [ ] Taste mode: O(N * 4 KiB) where N = candidate codecs.
- [ ] Documentation explains when to use which.
- [ ] Example in `omnizip-bench/examples/pick_best.rs`.

## Why this matters

Most callers don't care about which codec — they care about getting
good ratio or fast speed. The ensemble picker gives them the right
answer without requiring codec knowledge. This is the "no-think"
API for compression.
