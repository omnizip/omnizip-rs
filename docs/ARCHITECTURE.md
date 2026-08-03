# omnizip-rs Architecture

**Last updated:** 2026-08-03

This document records the architectural decisions in omnizip-rs and
flags areas that need future work. It complements `CLAUDE.md` (which
captures *invariants* and *workflow*) and `TUNABLE.md` (which
catalogues the user-facing tunability surface).

## Workspace layout

```
omnizip-rs/
├── omnizip-codecs/       — shared Codec trait + CodecRegistry + helpers
│   ├── codec.rs           (CodecId, Codec trait)
│   ├── registry.rs        (CodecRegistry, dispatch)
│   ├── error.rs           (OmnizipError)
│   ├── level.rs           (CompressionLevel u8 newtype)
│   ├── arith.rs           (binary ArithEncoder/ArithDecoder)
│   └── hash.rs            (FNV-1a, DJB2, tagged variants)
├── omnizip-filters/      — BCJ, delta, shuffle (Filter trait)
├── omnizip-lzma/         — LZMA / LZMA2 / XZ / lzip containers
├── omnizip-zstd/         — Zstd (frame, FSE, Huffman, sequences, dict)
├── omnizip-deflate/      — DEFLATE (miniz_oxide wrapper)
├── omnizip-deflate64/    — DEFLATE64 (true 64KB window)
├── omnizip-brotli/       — Brotli (brotli crate wrapper)
├── omnizip-snappy/       — Snappy (snap wrapper)
├── omnizip-lz4/          — LZ4 + LZ4 HC (lz4_flex wrapper)
├── omnizip-bzip2/        — BZip2 (BWT + MTF + RLE + Huffman)
├── omnizip-flac/         — FLAC (verbatim + FIXED + LPC + Rice)
├── omnizip-fsst/         — FSST (Fast Static Symbol Table)
├── omnizip-ricepp/       — Rice++ (DwarFS ricepp)
├── omnizip-blosc/        — BLOSC2 (multi-codec container)
├── omnizip-glza/         — GLZA (grammar-based LZ)
├── omnizip-ppmd/         — PPMd7 + PPMd8 (byte-level PPM)
│   ├── ppmd7/             (arena-allocated context trie)
│   └── ppmd8/             (recursive trie + glue count + RLE)
├── omnizip-zpaq/         — ZPAQ (context mixing)
└── tests/differential/   — cross-language parity harness
```

## Layered design

Three strict layers, dependencies always downward:

1. **`omnizip-codecs`** — shared trait + utilities. No codec deps.
2. **Codec crates** — implement `Codec` trait. Depend on `omnizip-codecs`
   only. Never reach into each other's internals.
3. **Consumer (LimniFS, etc.)** — picks codecs via `CodecRegistry`,
   never imports codec-internal modules.

Violating this layering (e.g., a codec crate that depends on another
codec crate) is a hard error.

## Adding a codec — open/closed principle

The `Codec` trait + `CodecRegistry` pattern means adding a codec
requires:

1. A new crate (e.g. `omnizip-foo/`).
2. Add to `members` in root `Cargo.toml`.
3. Implement `Codec` for a unit struct (e.g. `pub struct FooCodec;`).
4. Caller registers it: `registry.register(FooCodec::new())`.

No edits to dispatch code, no edits to `omnizip-codecs`. This is the
open/closed principle applied to codecs.

The single place that DOES require editing when adding a codec is
`omnizip-codecs/src/codec.rs` — to allocate a new `CodecId` constant.
This is a known minor OCP violation; it's only a few lines per codec
and centralises ID assignment (preventing collisions). See
`TODO.complete/88-architecture-audit.md` for discussion.

## Shared utilities (DRY)

These live in `omnizip-codecs` and are used by multiple codec crates:

- **`omnizip_codecs::arith`** — binary arithmetic encoder/decoder.
  Used by PPMd7 and PPMd8 (both previously had their own copy).
- **`omnizip_codecs::hash`** — FNV-1a, DJB2, and tagged variants.
  Used by PPMd8 (was inline); other codecs that need a hash should
  reuse, not re-implement.

If you find yourself copy-pasting a hash function or arithmetic
coder, extract it here.

## Codec-specific options

Each codec has its own tunables (see `TUNABLE.md`). The `Codec`
trait's `compress(level)` is the entry point for the 90% case;
`compress_with_options(...)` is the escape hatch for power users.

The decision NOT to add a generic `compress_with_options(input, &dyn
Any)` to the `Codec` trait is deliberate: type-erased options give
bad UX (no compile-time checks, runtime panics). Each codec's options
struct stays concrete at its call site.

## Error types

Each codec has its own error type (`LzmaError`, `Ppmd7Error`, etc.).
The `OmnizipError` enum unifies them at the trait boundary.

Future: consider `thiserror` for ergonomics. Low priority — the
current pattern works and is explicit.

## Determinism invariant

**Every** codec MUST be deterministic: same input + same parameters
⇒ byte-identical output across runs, machines, and Rust versions.

This is enforced by:
- `#![forbid(unsafe_code)]` workspace-wide (no platform-specific
  integer widths, no uninit memory).
- Dedicated determinism tests in each codec (encode twice, assert
  byte-equal).
- No `HashSet`/`HashMap` iteration in encode paths (use `Vec` with
  deterministic insertion order).
- No time-seeded RNGs.

A codec that violates this invariant breaks content-addressed
storage (LimniFS `DropId = BLAKE3(plaintext)`).

## Known smells (audit pending)

See `TODO.complete/88-architecture-audit.md` for the full list.
Highlights:

- PPMd7 and PPMd8 still have separate context-trie implementations
  with similar but not identical code. A shared `PpmCore` would
  eliminate duplication.
- Adding a codec requires editing `codec.rs` for an ID constant
  (minor OCP violation).
- No benchmark suite (see `TODO.complete/86-benchmark-suite.md`).

## Convergent encryption boundary

omnizip-rs is the **codec layer only**. Convergent encryption (CE) —
where the encryption key is derived deterministically from the
plaintext hash, enabling cross-user dedup while preserving
confidentiality — lives in the **storage layer** (e.g. LimniFS).

The boundary is deliberate:

- omnizip-rs codecs are **content-defined**: same input + same params
  ⇒ byte-identical output. This is the foundation that CE builds on.
- `DropId = BLAKE3(plaintext)` (LimniFS) is convergent in spirit: the
  identity is a pure function of the content, so two clients storing
  the same plaintext converge on the same id. That property enables
  dedup at the storage layer without coordination.
- omnizip-rs codecs never see keys, IVs, or authentication tags.
  Adding crypto here would couple two orthogonal concerns and break
  the layered design.

**Reference:** *Convergent Encryption Enabled Secure Data Deduplication*
(Wiley 2024, https://onlinelibrary.wiley.com/doi/10.1002/cpe.8205)
surveys modern CE schemes (CE-1, CE-2, Dekey) and the known attacks
they address. The architectural split documented here means omnizip-rs
can be dropped into any of these schemes — the codec layer is
agnostic to which CE variant the storage layer picks.

## References

- `CLAUDE.md` — project invariants and workflow
- `TUNABLE.md` — user-facing tunability reference
- `RESEARCH.md` — 2024–2026 academic compression literature review
- `TODO.complete/README.md` — enhancement backlog
- `PLAN.md` — original Ruby → Rust port plan
