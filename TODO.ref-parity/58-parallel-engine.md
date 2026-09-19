# Task 58 — generic parallel engine

Status: partially done (2026-09-19 — intra-stream parallel_compress
in omnizip-codecs::parallel_batch (SSOT home; no new crate — the
batch module already owned parallelism): any Codec, explicit
job_size (caller owns the output contract), fixed strided worker
groups + results indexed by job = thread-count-invariant output;
threads<=1/single-job = exact one-shot; worker panics surface as
job errors; concatenation contract documented (zstd multi-frame,
gzip multi-member, bzip2/xz multistream qualify; raw deflate does
not). Tests: tagged-codec split/order/invariance, flaky-codec
propagation, empty/tiny. ALSO: fixed the tracker test race from
task 59 (zero-duration first update skipped — sleep-seeded).
REMAINING: archive-level parallel create/extract (entry
distribution via bounded channels, writer serialization)

## Gap

Ruby `parallel/`: thread pool + job queue/scheduler +
`parallel_compressor` (per-FILE compression jobs, Fractor workers)
and `parallel_extractor`. Any codec, any archive op. Rust: zstd
`compress_mt` only (workspace-internal, fixed-size job split).

## Scope

New crate `omnizip-parallel` (std threads only, no rayon — keep
dependency surface minimal and scheduling explicit):

1. **Intra-stream parallel compress** — generalize zstd's
   compress_mt verbatim pattern for any codec with a
   self-delimiting frame form: split input into fixed-size jobs
   (pure function of input length, never of `threads`), each job an
   independent frame on a scoped worker, concatenated in job order.
   Expose `parallel_compress(codec, input, level, threads,
   job_size)`. Threads ≤ 1 → codec one-shot (identical bytes).
2. **Archive-level parallel create/extract** — entries distributed
   across workers with a bounded channel; output ordering fixed by
   ENTRY LIST ORDER (deterministic regardless of completion order —
   archive writers serialize the directory themselves).
3. Progress hook slot (task 59 wires in here).

Determinism contract (same as compress_mt): output bytes identical
for any thread count and any completion interleaving. Cross-job
match loss at boundaries is accepted and documented (the zstd
trade-off note in lib.rs is the template).

## Acceptance

- Thread-invariance test per codec: `parallel_compress(t=1) ==
  parallel_compress(t=8)` AND `== one-shot` (job_size = one job).
- Archive create/extract: byte-identical archives across thread
  counts on the container corpus; content round-trip exact.
- No deadlock/hang under panicked worker (test with an injected
  failing job; bounded channels must drain).
- Bounded-work: worker count = threads, queue depth bounded by
  entries/jobs — no unbounded buffering of results (spill rule
  documented).

## References

`../omnizip/lib/omnizip/parallel/{engine,job_queue,job_scheduler,
parallel_compressor,parallel_extractor,thread_pool,worker_pool}.rb`
(Fractor is Ruby-specific; the Rust engine is std-threads).
