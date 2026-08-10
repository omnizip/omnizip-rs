# 253 — Wire-Format Differential Fuzzer

- **Priority:** P1 (catches wire-format bugs tests miss)
- **Crate:** workspace (`tests/fuzz/`)
- **Depends on:** [247](247-real-world-test-corpora.md) for seed corpus
- **Estimated effort:** 3 days

## Problem

Tests verify round-trip: encode → decode = original. This catches
internal inconsistencies but misses bugs where:

1. Our decoder accepts malformed input that `brotli -d` rejects.
2. Our encoder produces output that `brotli -d` rejects (the
   "DECODE-FAIL" lines in `brotli_benchmark.rs` output).
3. Our decoder produces different output than `brotli -d` on the
   same malformed input.

The current brotli benchmark shows every vendored decode FAILS.
That's a wire-format bug we can't even diagnose without a fuzzer.

## Design

### Fuzz targets

For each codec, two fuzz targets:

**Encode-then-reference-decode**: random input → our encoder →
reference decoder (C/Ruby subprocess). Output must equal input.

```rust
fn brotli_encode_ref_decode(data: &[u8]) {
    let compressed = omnizip_brotli::BrotliCodec.compress(data, ...);
    let decoded = run_cli("brotli", "-d", &compressed);
    assert_eq!(decoded, data);
}
```

**Reference-encode-then-our-decode**: reference encoder → our
decoder. Output must equal input.

```rust
fn ref_encode_brotli_decode(data: &[u8]) {
    let compressed = run_cli("brotli", "-qf", data);
    let decoded = omnizip_brotli::BrotliCodec.decompress(&compressed, data.len());
    assert_eq!(decoded, data);
}
```

**Malformed-input-survival**: fuzz the decoder with semi-valid
compressed input. Must never panic.

```rust
fn brotli_decode_no_panic(data: &[u8]) {
    let _ = omnizip_brotli::BrotliCodec.decompress(data, 65536);
    // No panic allowed; errors are fine.
}
```

### Reference implementations

Wire up CLI subprocesses for:
- `brotli` (C reference)
- `zstd` (C reference)
- `xz` (LZMA reference)
- `gzip` / `inflate` (DEFLATE)
- `lz4` (LZ4 reference)
- `bzip2` (libbz2)

For Ruby references (omnizip), add a `ruby_runner.rb` script that
encodes/decodes via the Ruby implementation.

### Fuzzer setup

Use `cargo-fuzz` with `libFuzzer`. Seed corpus from
`tests/fixtures/corpora/` sliced into 1 KiB - 1 MiB chunks.

```bash
# Run fuzzer for 60 seconds on brotli encode+ref-decode
cargo fuzz run brotli_encode_ref_decode -- -max_total_time=60

# Run all fuzzers in CI for 5 minutes each
cargo fuzz run --all -- -max_total_time=300
```

### CI integration

GHA workflow `fuzz.yml`:
- Triggers nightly on `schedule: cron: "0 4 * * *"`
- Runs each fuzzer for 5 minutes
- Uploads any crash artifacts
- Files issue automatically on crash

## Acceptance criteria

- [ ] `tests/fuzz/` set up with `cargo-fuzz`.
- [ ] Fuzz targets for: brotli, zstd, lzma, lz4, deflate, bzip2.
- [ ] At least 2 wire-format bugs found and fixed (typical first run).
- [ ] Nightly GHA workflow running all fuzzers.
- [ ] Crash artifacts stored in `tests/fuzz/artifacts/` and
      gitignored.

## Why this matters

A fuzzer finds bugs no human would think to test. The current
"DECODE-FAIL" on vendored decoder output is exactly the kind of
bug a fuzzer would have caught early. Wire-format bugs are
security-relevant (a malformed input crash is a DoS vector for
any service that decompresses untrusted input).
