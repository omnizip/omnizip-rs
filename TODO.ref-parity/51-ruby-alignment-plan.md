# Task 51 — Ruby ⇄ Rust alignment: close the application-layer gap, then accelerate the gem with Rust

Status: open (2026-09-19; owner-directed audit — "what does our rust
package not fully do that the ruby gem does? align them both; later use
rust to accelerate the ruby gem")

## Audit result (2026-09-19)

Rust is AHEAD on codecs (18 registered vs the gem's 8 pure-Ruby
families), containers (RAR4 LZ/PPMd/AES, RAR5 LZ/AES, PAR2, 7z-C),
and filters (all BCJ families + arm-thumb + shuffle — a superset of
the gem's). The debt is the APPLICATION layer the Ruby gem carries:

| # | Gap | Ruby reference | Task |
|---|-----|----------------|------|
| 1 | CLI commands: archive verify/repair, parity c/v/r, metadata, profile list/show | `commands/*.rb` (14 commands vs ozip's c/x/t/l) | 55 |
| 2 | Compression profiles + content-class detector | `profile/*.rb` | 60 |
| 3 | Format converter (strategy registry, batch) | `converter/*.rb` | 61 |
| 4 | Generic parallel engine (any codec / archive op) | `parallel/*.rb` | 58 |
| 5 | Chunked streaming (bounded memory over huge inputs) | `chunked/*.rb` | 57 |
| 6 | Progress/ETA callbacks | `progress/*.rb`, `eta/` | 59 |
| 7 | Password provider abstraction | `password/*.rb` | 54 |
| 8 | Implementation tiers per codec | `implementations/*.rb` | 56 |
| 9 | Checksum registry (pluggable family) | `checksums/`, `checksum_registry.rb` | 52 |
| 10 | ~~rubyzip_compat shim~~ | — | DROPPED (owner): exists only for Ruby-ecosystem migration; meaningless in Rust, obsolete once the native gem lands |
| 11 | xz / lzip / lzma-alone as first-class single-file formats | `formats/{xz,lzip,lzma_alone,gzip}.rb` | 53 |

Item 8 shrinks in Rust: in-house policy is pure-Rust-only codec
implementations, so "tiers" reduces to registry metadata (the live
two-impl case is DEFLATE vs LIBDEFLATE). Item 9 is a prerequisite of
1 (verify needs pluggable checksums), not standalone value.

## Phases

- **Phase 1 — foundations + CLI parity** (52 checksums, 53
  single-file formats, 54 password provider, 56 impl tiers) → then
  55 ozip commands (needs 52; parity commands wrap the par2 crate;
  profile commands stub until 60).
- **Phase 2 — engine** (57 chunked streaming; 58 parallel engine —
  can start independently on the compress_mt job-split model; 59
  progress wires into both).
- **Phase 3 — intelligence + conversion** (60 profiles, 61
  converter; converter composes reader/writer per entry, benefits
  from 57 but does not require it).
- **Phase 4 — acceleration** (62 native gem): magnus/rb-sys
  extension exposing Rust codecs behind the gem's existing
  `implementations/` seam, pure-Ruby fallback, byte-identical
  differential gate.

## Cross-cutting invariants (bind every task here)

1. Determinism: chunk sizes, job splits, and any partitioning are
   pure functions of input length + declared parameters — never of
   buffer arrival, thread count, or wall clock (workspace invariant
   3; LimniFS DropId contract).
2. Pure Rust only; `#![forbid(unsafe_code)]`; no external-tool
   runtime deps (oracles are test-only).
3. Ruby files named in each task are the algorithmic reference;
   wire-visible overlap goes through the differential harness.
4. Bounded-work: any new loop bound gets a worst-case analysis on
   pathological content before shipping (invariant 1 of CLAUDE.md).
5. Reuse before building: checksums reuse RustCrypto/crc crates
   already in the tree; profiles reuse the bench's content classes.

## Dependency graph

52 ─→ 55 ← par2(exists)   60 ─→ 55(profile cmds)
57 ─→ 59 wiring           58 ─(pattern from zstd compress_mt)
53, 54, 56 — independent
61 ← 60 (optional profile-aware convert), benefits from 57
62 ← codec API stability (phases 1–3 landed or explicitly scoped)
