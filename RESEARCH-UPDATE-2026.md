# RESEARCH UPDATE — 2022–2026 Compression Papers + Performance Audit

**Date:** 2026-08-04
**Scope:** Latest academic/industry compression research + omnizip-rs codebase performance audit

---

## Part 1: Latest Research (2022–2026)

### 1.1 Hutter Prize — Current SOTA

**fx2-cmix** (October 2024, Kaido Orav & Byron Knoll) remains the leader
on the 1GB enwik8 benchmark at ~111 MB compressed. Key innovations:
- Gated Linear Networks (GLN) alongside PAQ8-style context mixing
- Preprocessing (fx2) before cmix compression
- ~9 hours per run (impractical for production, but sets the ratio ceiling)

**Relevance to omnizip-rs:** Our ZPAQ with 7-model portfolio (order-0/1/2/3,
match, run-length, word) is the production-feasible subset of this approach.
The gap to fx2-cmix is the neural predictor (GLN), which we can't ship
(model weights + non-determinism).

### 1.2 ANS / Entropy Coding Advances

**FSAR-tANS** (ICLR 2024) — Finite-State Autoregressive Entropy Coding.
Combines tANS with an autoregressive probability model for discrete latent
spaces. Achieves favorable ratios for ML-quantized data.

**Kosolobov (2022)** — "Efficiency of ANS Entropy Encoders" (arXiv:2201.02514).
Theoretical analysis of ANS overhead. Key finding: the ANS redundancy bound
is tight for practical table sizes.

**Relevance:** Our 2-state interleaved FSE decoder already implements the
standard technique. The FSAR-tANS approach requires an autoregressive model
(similar to ZPAQ's mixer). Our ZPAQ mixer + arithmetic coder is the
analogous design.

### 1.3 ZSTD Ecosystem Adoption (2024–2025)

ZSTD achieved **30–50% better compression** than MS_XPRESS in SQL Server 2025,
**42% faster** than Brotli at similar ratio (Cloudflare benchmark), and is
now the default in NGINX, OpenResty Edge, and Yarn Berry.

No fundamental algorithmic changes to ZSTD itself — the gains are from
ecosystem adoption and tuning.

### 1.4 Learned Compression

Still impractical for omnizip-rs:
- Requires model weights (140GB+ for Chinchilla 70B)
- Non-deterministic on GPU across vendors
- "Language Modeling Is Compression" (DeepMind 2024, updated) remains
  the benchmark paper
- RWKV-based L3TC (AAAI 2025) is promising (48% saving vs gzip) but
  still requires embedded weights and is non-deterministic

### 1.5 Domain-Specific Compression

- **DNA data**: Novel encoding algorithm (Frontiers in Bioinformatics, 2025)
- **IoT/LoRa**: Comprehensive evaluation of classical algorithms for
  low-power networks (PMC, 2025)
- **Point clouds**: LDG-PCGC (IEEE, 2025) — out of scope for omnizip-rs

### 1.6 DCC 2026

The 37th IEEE Data Compression Conference was held March 24–27, 2026 in
Snowbird, Utah. Proceedings (ISBN 979-8-3315-8261-6) are indexed on DBLP
but specific papers are not yet widely available online.

### 1.7 Algorithm Selection Frameworks

arXiv 2509.25219 (2025) proposes a **mathematical method** for choosing
the optimal lossless algorithm based on evaluation criteria. Could inform
LimniFS's file categorizer.

---

## Part 2: Performance Audit of omnizip-rs

### Critical Finding: ZSTD High-Level Strategies Not Implemented

**Severity: HIGH — 5-15% ratio gap vs reference zstd at levels 16-22.**

The `cparams.rs` table defines `Btopt`, `Btultra`, `Btultra2` strategies
for levels 16-22, but `encoder/block.rs:416` falls through to `lazy2`
for ALL of these:

```rust
Strategy::Lazy2 | Strategy::Btlazy2 | Strategy::Btopt | Strategy::Btultra | Strategy::Btultra2 => {
    compress_block_lazy2(...)  // ← ALL high levels use lazy2!
}
```

The reference zstd uses a **binary tree match finder** for Btopt/Btultra2.
We have no binary tree implementation — only hash chains. This means:
- Level 16-22 produces output identical to level 9-12
- The "ultra" compression mode doesn't exist in our encoder
- Users paying the CPU cost of high levels get no ratio benefit

**Fix:** Implement a binary tree (BT4) match finder. Estimated 3-5 days.

### Finding 2: LZMA Match Finder is Hash-Chain Only

**Severity: MEDIUM — 3-8% ratio gap vs xz CLI at level 6+.**

Reference LZMA uses BT4 (binary tree with 4 children) for high levels.
Our encoder uses hash chains with max_chain_length=256. The optimal
parser runs but operates on hash-chain-quality matches, which find
shorter matches than BT4.

**Fix:** Add BT4 match finder. Estimated 2-3 days.

### Finding 3: BZip2 BWT is O(n log² n)

**Severity: MEDIUM — 2-5x slower than reference bzip2 on large blocks.**

Manber-Myers prefix doubling with `sort_by` per iteration. Reference
bzip2 uses a fallback BWT that degrades to suffix sorting only when
needed. SA-IS (O(n)) would be faster but requires careful implementation.

**Fix:** Replace with SA-IS or divsufsort-lite. Estimated 2-3 days.

### Finding 4: CRC32/XXHash Use Software Tables

**Severity: LOW — 2-4x slower than PCLMULQDQ-backed CRC on x86.**

Slice-by-8 benefits from ILP but can't match hardware-accelerated CRC32C
(PCLMULQDQ instruction). XXHash64 uses scalar accumulators instead of
SIMD vectorised wide accumulators.

**Fix:** Would require opt-in `unsafe-simd` feature (PCLMULQDQ intrinsics).
Not possible under `#![forbid(unsafe_code)]`. Documented in TODO 82.

### Finding 5: ZPAQ WordModel refresh_cache is O(N) Per Hash Change

**Severity: LOW — only affects ZPAQ Best portfolio on text inputs.**

Each time the current word hash changes (every byte inside a word),
`refresh_cache` iterates ALL entries in `next_byte_freq` that match the
hash. With 65K entries and frequent hash changes, this is expensive.

**Fix:** Maintain per-hash incremental counters instead of full rescans.
Estimated 1 day.

### Finding 6: Bench Doesn't Use ZstdCompressor

**Severity: LOW — affects benchmark numbers but not production code.**

The `omnizip-bench` runner creates a fresh `ZstdCodec` per case, missing
the `ZstdCompressor` reuse opportunity. The 5× per-case compress calls
each allocate a fresh match-state table.

**Fix:** Add `BenchCodec::with_state()` that holds a reusable compressor.
Estimated 1 day.

### Finding 7: libdeflate LZ77 Chain Depth is 32

**Severity: LOW — limits match quality for the in-house encoder.**

Reference zlib uses chain depths of 128 (level 6) to 4096 (level 9).
Our libdeflate encoder uses a fixed 32. This means shorter matches on
average, producing worse compression than zlib.

**Fix:** Map compression level → chain depth. Estimated 0.5 days.

### Finding 8: format! in 90 Error Paths

**Severity: NEGLIGIBLE — error paths are cold.**

90 `format!` calls in error paths across the workspace. Each allocates
a String. Not a hot-path issue but adds to binary size.

**Fix:** Use `&'static str` where possible, or lazy formatting.
Low priority.

---

## Part 3: Recommended Priority

| Priority | Finding | Impact | Effort |
|----------|---------|--------|--------|
| P0 | ZSTD BT match finder for levels 16-22 | 5-15% ratio gap | 3-5 days |
| P1 | LZMA BT4 match finder | 3-8% ratio gap | 2-3 days |
| P2 | BZip2 SA-IS BWT | 2-5x speed | 2-3 days |
| P3 | ZPAQ WordModel incremental cache | Text encode speed | 1 day |
| P4 | Bench ZstdCompressor reuse | Bench accuracy | 1 day |
| P5 | libdeflate chain depth tuning | Ratio improvement | 0.5 days |
| — | CRC32 PCLMULQDQ | Requires unsafe | Blocked |
| — | Learned compression | Non-deterministic | Blocked |

---

## References

- [Hutter Prize official site](http://prize.hutter1.net/)
- [fx2-cmix GitHub](https://github.com/kaitz/fx2-cmix)
- [Kosolobov (2022), "Efficiency of ANS Entropy Encoders"](https://arxiv.org/html/2201.02514v3)
- [FSAR-tANS (ICLR 2024)](https://proceedings.iclr.cc/paper_files/paper/2024/file/c7138635035501eb71b0adf6ddc319d6-Paper-Conference.pdf)
- [ZSTD in SQL Server 2025](https://techcommunity.microsoft.com/blog/azuresqlblog/zstd-compression-in-sql-server-2025/4415418)
- [Cloudflare ZSTD benchmarks](https://blog.cloudflare.com/new-standards/)
- [State of SIMD in Rust 2025](https://www.reddit.com/r/rust/comments/1op5jlj/the_state_of_simd_in_rust_in_2025/)
- [Safe SIMD in Rust (Shnatsel)](https://shnatsel.medium.com/safe-simd-in-rust-even-on-the-inside-c6f1ff381828)
- [DCC 2026](https://datacompressionconference.org/)
- [Algorithm selection framework (arXiv 2025)](https://arxiv.org/html/2509.25219v1)
