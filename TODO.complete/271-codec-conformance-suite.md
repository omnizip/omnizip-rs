# 271 — Codec Conformance Test Suite

- **Priority:** P1 (correctness — every codec must accept spec-conformant input)
- **Crate:** workspace (`tests/conformance/`)
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 3 days

## Problem

Each codec has unit tests + round-trip tests, but no comprehensive
conformance suite that verifies "this codec accepts ALL spec-conformant
inputs from the official test vector corpus".

For example:
- RFC 7932 has a property test corpus (~4,000 files) that real Brotli
  decoders must accept. We don't test against it.
- LZMA has the XZ test corpus. We don't systematically decode every file.
- ZSTD has the official zstd test corpus.

Without this, "spec compliance" is a claim, not a verified fact.

## Design

### Per-codec test vector corpora

Download (or vendor) each format's official test corpus:

- Brotli: `brotli/tests/testdata/*` (4,000+ files)
- ZSTD: `zstd/tests/regression/*`
- LZMA: `xz-utils/tests/files/*`
- LZ4: `lz4/tests/*`

### Conformance harness

`tests/conformance/` with one test per codec that walks its corpus:

```rust
#[test]
fn brotli_accepts_all_test_vectors() {
    for entry in walk_dir("tests/fixtures/conformance/brotli/") {
        let compressed = std::fs::read(&entry.path()).unwrap();
        let _ = omnizip_brotli::BrotliCodec::new()
            .decompress(&compressed, u32::MAX)  // size unknown
            .expect("must accept spec-conformant input");
    }
}
```

### CI integration

Nightly GHA workflow runs conformance on all codecs. Failures open
issues automatically.

## Acceptance criteria

- [ ] Brotli test vectors (4,000+ files) downloaded and integrated.
- [ ] ZSTD test vectors integrated.
- [ ] LZMA test vectors integrated.
- [ ] LZ4 test vectors integrated.
- [ ] `cargo test --test conformance --workspace` passes.
- [ ] Nightly GHA workflow.

## Why this matters

"Spec compliant" should be testable, not asserted. The conformance
suite is the test that backs the claim. It also catches regressions
when we refactor decoders.
