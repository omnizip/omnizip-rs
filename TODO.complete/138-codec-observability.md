# TODO 138: Codec observability — metrics + tracing hooks

## Problem

Long-running compress/decompress calls are opaque. LimniFS can't
show progress, can't measure per-call cost, can't detect
pathological inputs without running the call to completion.

## Proposed fix

Add a `CompressContext` parameter (default = no-op) that codecs
call into at meaningful points:

```rust
pub trait CompressObserver {
    fn on_block_start(&self, block_index: u64, input_size: usize);
    fn on_block_end(&self, block_index: u64, output_size: usize);
    fn on_match_found(&self, distance: u32, length: u32);
    fn on_literal_emitted(&self, byte: u8);
    fn progress(&self) -> ProgressHint;  // Continue / Abort
}

pub struct NoOpObserver;
impl CompressObserver for NoOpObserver { /* all no-ops */ }
```

Each codec takes `&dyn CompressObserver` in its extended API. The
default `Codec::compress` uses `NoOpObserver`.

LimniFS can pass a counting observer for progress UIs, or a
tracing observer for diagnostics.

## Acceptance criteria

- [ ] `CompressObserver` trait lands in `omnizip-codecs`.
- [ ] LZMA + ZSTD call into it at block boundaries.
- [ ] Example: `omnizip-bench` shows live progress for long runs.

## Priority

P2 — observability is good engineering but not on the critical path.
