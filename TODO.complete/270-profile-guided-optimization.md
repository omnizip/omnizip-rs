# 270 — Profile-Guided Optimization (PGO)

- **Priority:** P3 (perf: 5-15% speedup via PGO)
- **Crate:** workspace
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 2 days

## Problem

Rust's default release profile (`-O3`) is good but generic. It
optimizes for the average input. For specific workloads (Silesia
text, LimniFS CSV patterns), branch prediction and inlining can be
tuned better.

Profile-Guided Optimization (PGO) collects runtime profiles from
representative inputs and feeds them back to the compiler. Typical
gains: 5-15% on hot paths.

## Design

### PGO build pipeline

```bash
# Step 1: Instrument build
RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data" \
    cargo build --release --workspace

# Step 2: Run with representative workloads
./target/release/omnizip-bench --corpus silesia,enwik8,calgary
./target/release/omnizip-bench --corpus limnifs  # if available

# Step 3: Merge profile data
llvm-profdata merge /tmp/pgo-data -o /tmp/pgo.profdata

# Step 4: Optimized build using profile
RUSTFLAGS="-Cprofile-use=/tmp/pgo.profdata" \
    cargo build --release --workspace
```

### CI integration

A monthly GHA workflow:

1. Build instrumented binaries.
2. Run on Silesia + Enwik8 + Calgary + synthetic CSV.
3. Merge profile data.
4. Cache profile for use by PR builds.
5. PR builds use the cached profile.

### Caveats

- PGO doubles build time (instrument + rebuild).
- Profile data is platform-specific (Linux profile doesn't help
  macOS builds).
- The profile reflects the workload used to generate it; if real
  workloads differ, gains shrink.

## Acceptance criteria

- [ ] `scripts/build-pgo.sh` automates the instrument-run-merge-rebuild flow.
- [ ] Monthly GHA workflow regenerates the cached profile.
- [ ] PR builds use cached profile.
- [ ] Measured 5-15% throughput improvement on Silesia vs. baseline.
- [ ] Documentation explains when to use PGO builds.

## Why this matters

PGO is a low-risk way to extract more performance from the same
code. Once set up, every codec benefits automatically. The 5-15%
gain is significant for LimniFS write throughput.
