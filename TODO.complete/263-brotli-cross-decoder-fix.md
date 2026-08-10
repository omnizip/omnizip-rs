# 263 — Brotli Cross-Decoder Wire-Format Fix

- **Priority:** P0 (correctness — encoder output rejected by vendored C decoder)
- **Crate:** `omnizip-brotli`
- **Depends on:** [244](244-brotli-decoder-wire-format-bugs.md)
- **Estimated effort:** 3-5 days

## Problem

Every `brotli_benchmark.rs` output shows:

```
Q11 vend:  20001 bytes (25.4%) in 0.00s DECODE-FAIL
brotli decode failed: invalid back-reference distance
```

The vendored C reference decoder (`brotli -d`) REJECTS our encoder
output with "invalid back-reference distance". Our own decoder
accepts the output and round-trips correctly, but the cross-decoder
divergence indicates a wire-format bug somewhere.

This is the headline issue blocking "real Brotli compatibility"
claims. Even though our ratios now beat vendored, real-world users
use vendored (or browsers, or curl) to decode and would see failures.

## Root cause hypothesis

The error message points at distance computation. Possible causes:

1. **Distance > max_backward_distance**: encoder might emit a
   distance larger than the window allows.
2. **Dict reference address calculation**: bug in
   `dictionary.rs::dictionary_lookup` for some transforms.
3. **Distance context mapping**: bug in how dist context is
   computed for NTREESD > 1 (we always use NTREESD=1, but
   something else might leak).
4. **NPOSTFIX/NDIRECT mismatch**: encoder and decoder disagree on
   the alphabet configuration.

## Design

### Reproduction

1. Capture our encoder output for a 1-byte input.
2. Hex-dump + decode step-by-step following RFC 7932.
3. Compare to what vendored C would produce for the same input.
4. Identify the first divergent bit.

### Fixture-based differential test

```bash
# In tests/differential/
echo "hello world" > /tmp/fixture.txt
cargo run -- --encode brotli --level 5 < /tmp/fixture.txt > /tmp/out.br
brotli -d < /tmp/out.br > /tmp/out.txt  # expect: hello world
```

If vendored rejects, the harness dumps:
- Our compressed bytes (hex)
- The exact RFC 7932 step where divergence happens
- Suggested fix

## Acceptance criteria

- [ ] `brotli -d` accepts our encoder output for all Silesia text
      fixtures at Q1, Q5, Q11.
- [ ] `cargo test --workspace` includes a cross-decoder test that
      runs `brotli -d` as a subprocess.
- [ ] The brotli_benchmark `DECODE-FAIL` lines are replaced with
      actual ratio comparisons.
- [ ] Wire-format test fixtures added to tests/fixtures/brotli/.

## Why this matters

Without C reference decoder acceptance, our "Brotli" is just a
format that happens to look like Brotli. Real users (browsers,
curl, brotli CLI) cannot decode our output. This is the single
most important issue for production Brotli use.

Until TODO 244 (decoder wire-format bugs) is resolved, our own
decoder accepts outputs that vendored rejects — meaning our
decoder is too lenient about something.
