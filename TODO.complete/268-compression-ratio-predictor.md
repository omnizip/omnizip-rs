# 268 — Compression Ratio Predictor

- **Priority:** P3 (UX: estimate before commit)
- **Crate:** workspace
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 5 days

## Problem

To know how well a codec will compress a given input, you have to
run it. For large inputs (1 GiB), that's seconds to minutes.

If you're picking between 5 codecs × 3 levels, you spend 15× the
trial time. For interactive tools (file choosers, format advisors),
this is too slow.

## Design

### Trained model

Train a simple regression model that takes input features:

- File size (log scale)
- Sample byte histogram (256-dim, normalized)
- Sample 4-byte hash entropy
- File extension (if known)
- ContentType::detect() result

And predicts:

- Compressed size for each codec × level combo
- Wall-clock time for each codec × level combo

### Sampling

For inputs > 64 KiB, sample the first 64 KiB + middle 32 KiB + last
32 KiB = 128 KiB total. Build features from the sample.

### Training corpus

Use Silesia + Enwik8 + Calgary + LimniFS corpora (TODO 247). For
each file × codec × level, record actual compressed size and time.

Train a gradient-boosted decision tree (or just simple per-codec
linear regression). Save as `predictor.json`.

### API

```rust
pub struct RatioPredictor {
    models: BTreeMap<CodecId, Model>,
}

impl RatioPredictor {
    pub fn load() -> Self {
        let json = include_str!("../data/predictor.json");
        Self { models: serde_json::from_str(json).expect("predictor.json") }
    }

    pub fn predict(&self, input: &[u8], codec: CodecId, level: u8)
        -> PredictedStats
    {
        let features = extract_features(input);
        self.models[&codec].predict(&features, level)
    }
}

pub struct PredictedStats {
    pub compressed_bytes: u64,
    pub elapsed_ms: u32,
    pub confidence: f32,  // 0.0-1.0
}
```

## Acceptance criteria

- [ ] Feature extractor: 256-dim byte histogram + size + content type.
- [ ] Model trained on Silesia + Enwik8 with >80% accuracy within
      ±10% of actual compressed size.
- [ ] `RatioPredictor` API in `omnizip-bench`.
- [ ] CLI subcommand `omnizip-bench predict <file>` prints predicted
      ratio for each codec × level.

## Why this matters

Interactive tools that recommend compression settings need to be
fast. A pre-learned predictor answers in milliseconds. Without it,
either the user guesses or the tool wastes time running trials.
