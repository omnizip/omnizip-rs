# 90 — Add AIT 2026 corpus to benchmark suite

**Priority:** High (extends #86)
**Source:** RESEARCH.md §12 (arXiv:2606.17712)

## Context

The 2026 Algorithmic Information Theory Data Compression Challenge
published a 16-file heterogeneous corpus as the public training set
for a current SOTA compression benchmark. 117 submissions on the
leaderboard as of 2026-08.

Our `omnizip-bench` crate (TODO 86, ✅ done) already reserves a
placeholder corpus slot:

```rust
CorpusSpec {
    name: "ait2026",
    url: "https://example.com/ait2026.zip",  // placeholder
    files: &["ait2026_corpus.bin"],
}
```

## Action

1. Locate the official AIT 2026 challenge URL (likely linked from
   arXiv:2606.17712 or the challenge website).
2. Update the `url` field in `omnizip-bench/src/corpus.rs` to the
   real download.
3. Update the `files` list with the actual corpus file names from
   the zip.
4. Run `cargo run -p omnizip-bench -- --corpus ait2026 --codec zstd,lzma,brotli`
   and record ratios vs the leaderboard.

## Acceptance criteria

- [ ] AIT 2026 corpus downloads successfully on first run.
- [ ] At least one omnizip-rs codec's ratio is recorded against the
      leaderboard.
- [ ] If our ratios are competitive (top-half), note in README.
- [ ] If our ratios are not competitive, file follow-up TODOs for
      the specific codecs that lag.

## Files

- `omnizip-bench/src/corpus.rs` — replace placeholder URL with real one.
