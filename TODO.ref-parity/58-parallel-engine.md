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
FINAL (2026-09-19/20 third pass) — PARALLEL EXTRACT SHIPPED per
the design pass: ParallelReader side-trait (read_entry_shared via
&self; OPTIONAL per reader, OCP — ZipReader first, its decode
proven mutation-free; both paths delegate ONE read_entry_inner —
SSOT). extract_parallel in archive-core: serialized directory
pre-pass in entry order (no mkdir races, parents exist before
writes), strided worker groups over file indices, every write
behind write_entry_secured — the SAME security helper the serial
default extract_to now uses (the security boundary was refactored
into ONE function; security corpus 11/11 green). Output
equivalence pinned: every file byte-identical to serial across
threads=1/3/8; shared decode == trait decode for every entry.
Writer-side parallel CREATE SHIPPED (2026-09-20, zip flagship):
ZipWriter::prepare (pure compress+crc+size precompute, thread-safe)
+ add_file_prepared (serial emission; add_file = prepare + emit —
SSOT); parallel_create(files, method, threads) = strided parallel
prepare + serial emit in ENTRY ORDER => byte-identical to the
serial writer for ANY thread count (pinned t=1/3/8 vs serial,
including the uncompressed-size field the first cut missed —
PreparedEntry carries the plaintext size). Round-trip pinned. The
generic pattern generalizes to the other writers as their
compress-with seams allow

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
