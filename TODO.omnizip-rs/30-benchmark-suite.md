# 30 — Benchmark suite

- **Priority:** P1 (proves ratio claims)
- **Depends on:** [03](03-conformance-corpus.md)
- **Estimated effort:** 1 week
- **Crate:** `omnizip-bench`

## Goal

Cross-codec benchmark framework: for every `(codec, level, fixture)`
combination in the conformance corpus, measure encode throughput, decode
throughput, ratio, and peak memory. Output JSON + Markdown for CI artifacts
and trend tracking.

## Design

```
omnizip-bench/
├── Cargo.toml
├── src/
│   ├── lib.rs           # bench runner API
│   ├── harness.rs       # timing, memory measurement
│   ├── report.rs        # JSON + Markdown output
│   └── codecs.rs        # registry of every codec to benchmark
└── benches/
    └── full_matrix.rs   # criterion benches for every (codec, level, file)
```

The bench runner iterates the conformance corpus (task 03) × every
registered codec × every level the codec supports. For each combination:

1. Read fixture into memory.
2. Encode: measure wall time, peak allocations.
3. Decode: measure wall time.
4. Compute ratio = compressed_size / input_size.
5. Record `(codec, level, fixture, encode_ns, decode_ns, ratio,
   peak_bytes)`.

## Output

JSON:
```json
{
  "timestamp": "2026-08-01T12:00:00Z",
  "rustc": "1.97.1",
  "results": [
    {
      "codec": "lzma",
      "level": 6,
      "fixture": "dickens.txt",
      "input_bytes": 10192446,
      "output_bytes": 2847291,
      "ratio": 0.279,
      "encode_ns": 847000000,
      "decode_ns": 92000000,
      "peak_bytes": 67108864
    }
  ]
}
```

Markdown summary: per-codec aggregate (geomean ratio, encode/decode
throughput across fixtures), sorted by ratio and by speed.

## CI integration

- Runs on every release tag.
- Compares current run vs previous release's JSON. Fails on ratio
  regression > 2% or speed regression > 10%.
- Uploads JSON + Markdown as workflow artifacts.

## Acceptance

- Every codec registered in `omnizip-codecs::CodecRegistry::default_pure_rust()`
  is benchmarked.
- A new codec added via the registry is automatically included (open/closed).
- The Silesia subset runs in < 5 minutes per codec.
- JSON output validates against a JSON schema.
- Clippy clean, no `unsafe`.

## Implementation notes

- Use `criterion` for the bench harness (statistical rigour).
- Peak memory: use `jemalloc_ctl` or platform-specific stats. On macOS use
  `mach_task_basic_info`; on Linux use `/proc/self/status`.
- Don't benchmark in debug mode — release only.
- For trends: store the JSON in a `bench-history/` branch, append-only.
