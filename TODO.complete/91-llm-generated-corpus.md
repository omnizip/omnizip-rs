# 91 — Add LLM-generated text corpus to benchmark

**Priority:** Medium (extends #86)
**Source:** RESEARCH.md §16 (arXiv:2505.06297)

## Context

Recent research (*Lossless Compression of Large Language Model
(LLM)-Generated Data*, arXiv:2505.06297) shows that LLM-generated
text has different statistical properties than human text — more
repetitive, more templated. Standard compressors (gzip, bzip2, lzma)
underperform on it.

If LimniFS users store AI-generated content (likely!), our codecs
should be tuned for it.

## Action

1. Build a synthetic LLM-output corpus (~10 MB) by sampling outputs
   from a few common LLM use cases:
   - 2 MB of ChatGPT-style conversational responses
   - 2 MB of Claude-style long-form writing
   - 2 MB of code generation (Python/Rust/JS)
   - 2 MB of structured JSON output (tool-use)
   - 2 MB of summarised documents
2. Add as a `CorpusSpec` in `omnizip-bench/src/corpus.rs`. Either
   host the zip in the omnizip-rs GitHub releases, or generate
   on-demand via a synthetic.rs variant.
3. Run the bench across all codecs. Compare ratios vs the same codecs
   on Enwik8 (human text).

## Expected outcome

- ZSTD with a trained dictionary (TODO 81, ✅ done) should win
  significantly on LLM text — the templated structure is exactly
  what dictionary training exploits.
- PPMd7/PPMd8 should also do well — byte-level PPM adapts to
  repetitive structure better than LZ77 family.

## Acceptance criteria

- [ ] LLM-output corpus available via `--corpus llm-text`.
- [ ] Benchmark results recorded for at least zstd, lzma, ppmd7, brotli.
- [ ] ZSTD-with-dict beats ZSTD-without-dict by ≥10% on LLM text.
- [ ] README documents the workload and how to reproduce.

## Files

- `omnizip-bench/src/corpus.rs` — new `CorpusSpec`.
- `omnizip-bench/README.md` — workload documentation.
