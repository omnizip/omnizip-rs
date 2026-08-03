# 95 — Wire FSST/Rice++/FLAC/BLOSC/GLZA/Deflate64 into omnizip-bench

**Priority:** Low (bench coverage)
**Source:** omnizip-bench currently omits 6 codec crates

## Context

`omnizip-bench/src/lib.rs::default_codecs()` lists 11 codec entries
but skips 6 codec crates that have niche input requirements:

| Crate             | Why skipped initially                          |
|-------------------|------------------------------------------------|
| `omnizip-deflate64`| Should work on any input — try adding         |
| `omnizip-fsst`    | Optimized for short string lists — try adding |
| `omnizip-ricepp`  | Optimized for integer sequences — try adding  |
| `omnizip-flac`    | Audio samples — may need synthetic audio input|
| `omnizip-blosc`   | Multi-codec container — may nest badly        |
| `omnizip-glza`    | Grammar-based — should work on any input      |

## Approach

For each crate, attempt to add it to `default_codecs()` with a
sensible level set. Run the bench on synthetic 4 KB input. If it
round-trips, keep the entry. If it fails or produces nonsensical
ratios, leave a code comment explaining why and skip.

```rust
BenchCodec::new("fsst", Box::new(FsstCodec), vec![1]),
BenchCodec::new("ricepp", Box::new(RiceppCodec::default()), vec![1]),
BenchCodec::new("glza", Box::new(GlzaCodec), vec![1, 6, 9]),
// etc.
```

For FLAC: add a synthetic audio corpus (16-bit PCM, mono, 44.1kHz
sine wave) since FLAC verbatim mode should accept arbitrary bytes
but its real value is on audio-shaped inputs.

## Acceptance criteria

- [ ] At least 3 of the 6 missing codecs added to `default_codecs()`.
- [ ] Each addition documented with a comment if it required special
      handling (e.g. custom corpus for FLAC).
- [ ] `cargo run -p omnizip-bench -- --synthetic 4096` still produces
      a valid report covering ≥ 14 codecs.

## Files

- `omnizip-bench/src/lib.rs` — extend `default_codecs()`
- `omnizip-bench/src/synthetic.rs` — possibly add audio corpus
