# Differential fuzzer scaffold (TODO 253)

This directory contains **differential fuzz targets** that verify
omnizip-rs encoders/decoders against reference implementations.

## What it tests

Three categories of bugs:

1. **Round-trip**: encode → decode must equal original.
2. **Decode-only safety**: malformed input must never panic.
3. **Cross-implementation**: encode via Rust → decode via C reference
   (and vice versa) must produce identical output.

## Setup

`cargo-fuzz` uses libFuzzer under the hood. Install with:

```bash
cargo install cargo-fuzz
```

## Running

Each target lives in `targets/<name>.rs`. Run with:

```bash
# Run brotli round-trip fuzzer for 60 seconds
cargo fuzz run brotli_round_trip -- -max_total_time=60

# Run all targets
cargo fuzz run --all -- -max_total_time=300
```

Crash artifacts go to `tests/fuzz/artifacts/` (gitignored).

## CI integration

A nightly GHA workflow runs each fuzzer for 5 minutes. Crashes
automatically open issues with the failing input attached.

## Why fuzz?

Hand-written tests catch bugs the author thought to test. Fuzzers
find bugs the author didn't think of — especially:

- Edge cases at byte boundaries
- Off-by-one errors in length encoding
- Integer overflow in distance/length arithmetic
- Decoder panics on truncated/malformed input
- Wire-format divergences between encoder and reference decoder

The brotli encoder currently shows DECODE-FAIL on vendored C decoder
output for some inputs (see `brotli_benchmark.rs`). A fuzzer would
have caught this earlier.
