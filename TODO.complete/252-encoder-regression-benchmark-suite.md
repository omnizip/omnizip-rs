# 252 — Encoder Regression Benchmark Suite

- **Priority:** P1 (catch perf regressions before merge)
- **Crate:** workspace (`tests/benchmarks/`)
- **Depends on:** [247](247-real-world-test-corpora.md)
- **Estimated effort:** 2 days

## Problem

Currently, ratio and speed regressions are caught:
- Manually, by running `cargo run --example brotli_benchmark` before
  a PR.
- After-merge, by the user noticing "1.5× slower than last release".

This is unreliable. The brotli perf cliff (TODO 110) and the WAV
speed regression (DistanceConfig 7-candidate evaluation) both shipped
to released versions because no automated benchmark existed.

## Design

### Benchmark harness

`tests/benchmarks/regression.rs` runs each codec on each fixture
in `tests/fixtures/corpora/`, records:

```json
{
  "version": "0.16.22",
  "commit": "d04df42",
  "timestamp": "2026-08-10T05:30:00Z",
  "results": {
    "brotli/Q5/silesia/dickens": {
      "input_bytes": 10192446,
      "output_bytes": 3456789,
      "ratio": 0.3391,
      "elapsed_ms": 1247,
      "mbps": 7809.4
    },
    ...
  }
}
```

### Baseline comparison

Store the last green run's results in `tests/benchmarks/baseline.json`.
The harness compares each result to baseline:

- Ratio regression: output_bytes increased by > 1% → fail
- Speed regression: elapsed_ms increased by > 5% → fail
- Improvement: log as info (no fail)

Plots ratio + speed over time when run locally.

### CI integration

Workflow `regression-bench.yml`:
- Triggers on PRs touching `omnizip-*/src/**`
- Runs `tests/benchmarks/regression.rs` against the PR branch
- Compares to baseline.json on main
- Fails PR if any benchmark regresses beyond thresholds
- Comments on PR with summary table

### Noise reduction

CI benchmarks are noisy. Mitigations:
- Run each benchmark 3 times, take median.
- Use `cargo build --release` with fixed `RUSTFLAGS` (no debug info).
- Pin to a specific runner type (`ubuntu-22.04-large`).
- Warm-up run before measurement.
- Compare only same-runner results.

If noise is still too high, switch to criterion's statistical
comparison (which uses t-tests to detect real differences).

## Acceptance criteria

- [ ] `tests/benchmarks/regression.rs` runs on Silesia, Enwik8,
      Calgary, LimniFS fixtures.
- [ ] Output JSON format documented.
- [ ] `baseline.json` committed with current numbers.
- [ ] GHA workflow runs on PRs, comments summary.
- [ ] At least 3 benchmarks flagged as "improvement" since last
      release (sanity check that the system works).

## Why this matters

Without continuous benchmarks, perf is "felt" rather than measured.
Users notice "feels slower" only after a release. By then, multiple
regressions have stacked. Continuous benchmarks catch each regression
at the PR that introduced it.
