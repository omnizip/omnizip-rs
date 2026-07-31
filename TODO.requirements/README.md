# Requirements — acceptance criteria and use cases

These documents define **what must be true** for each codec to be
considered production-ready. They are the acceptance criteria that gate
merge.

## Relationship to other TODO directories

| Directory | Question answered |
|---|---|
| `TODO.spec/` | What is the format? (bit-level) |
| **`TODO.requirements/`** | **What must the implementation do?** |
| `TODO.references/` | Where is the source material? |
| `TODO.omnizip-rs/` | How do we implement it? |

A requirement changes only when a use case changes (new platform, new
performance target, new security constraint). It is independent of the
spec — the same "decode must not panic on malformed input" requirement
applies regardless of the wire format.

## Hard requirements (apply to every codec)

| ID | Requirement | Verification |
|---|---|---|
| R01 | **Determinism.** Same input + same level + same parameters ⇒ byte-identical output across runs, machines, and Rust versions. | CI runs encode twice; asserts byte-identical. |
| R02 | **No panics on untrusted input.** Every decoder returns `Err` on malformed data, never panics. | cargo-fuzz target runs 5 minutes per codec nightly. |
| R03 | **Bounded memory.** Memory usage is proportional to input size, not unbounded by length fields. Decoder rejects `dict_size` or `window_log` claims that would allocate > the configured limit. | Unit test with adversarial lengths. |
| R04 | **Pure Rust.** No C dependencies, no `unsafe` outside vetted FFI (currently zero). | `cargo tree` confirms no C deps; `#![forbid(unsafe_code)]` workspace-wide. |
| R05 | **Air-gapped.** Builds offline with no network access. | CI builds with `--offline` after initial cargo fetch. |
| R06 | **Cross-language parity.** Rust output byte-identical to Ruby reference on every fixture. | Differential test harness (TODO.omnizip-rs/02). |
| R07 | **C reference interop.** Rust output decompresses through the reference C tool (`xz -d`, `zstd -d`, `bzip2 -d`, `gzip -d`). | CI integration test. |
| R08 | **Clippy clean.** `#![warn(clippy::pedantic)]` passes. | CI clippy gate. |
| R09 | **Documented public API.** Every public item has a doc comment. | `cargo doc` with `-D warnings`. |

## Per-codec requirements

| # | File | Codec |
|---|---|---|
| 01 | [01-lzma-requirements.md](01-lzma-requirements.md) | LZMA / LZMA2 / XZ |
| 02 | [02-zstd-requirements.md](02-zstd-requirements.md) | Zstandard |
| 03 | [03-wrapper-requirements.md](03-wrapper-requirements.md) | Snappy, LZ4, DEFLATE, Brotli (wrappers) |
| 04 | [04-filter-requirements.md](04-filter-requirements.md) | Delta, BCJ-x86 |
