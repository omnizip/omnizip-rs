# omnizip-rs — Recent Research Analysis (2024–2026)

**Date:** 2026-08-03
**Scope:** Academic and industry literature review from the last 24 months, with concrete recommendations for omnizip-rs.

This document maps recent compression research onto actionable enhancements for omnizip-rs. Each recommendation is annotated with **fit** (high/medium/low) given omnizip-rs's hard constraints:

- **Pure Rust**, `#![forbid(unsafe_code)]` workspace-wide
- **Deterministic** — same input + same params ⇒ byte-identical output (LimniFS `DropId = BLAKE3(plaintext)` invariant)
- **No external model weights** — must be fully self-contained
- **Random-access friendly** — content-addressed FS use case

---

## 1. LLM-based compression — DeepMind ICLR 2024

**Paper:** *Language Modeling Is Compression* (Delétang et al., ICLR 2024)
**URL:** https://arxiv.org/abs/2309.10668

**Finding:** Chinchilla 70B used as a predictor + arithmetic coder beats PNG on images, FLAC on audio, and LZMA on text. The compression viewpoint yields new insights into LLM scaling laws.

**Fit for omnizip-rs: ❌ Not feasible**
- Requires 70B-parameter model (140 GB+ of weights)
- Non-deterministic sampling unless top-k is fixed (and even then, GPU floating-point ops are non-deterministic across devices)
- Incompatible with the byte-identical determinism invariant
- Decompression requires running the same model on the decoder side — kills random access

**What we COULD borrow:**
- The arithmetic-coder-with-neural-predictor pattern is already what ZPAQ does in our crate (just with a smaller, deterministic context-mixing model)
- For small, fixed corpora we could ship a small static LSTM as a "predictor sidecar" — but this is a research project, not a near-term enhancement

---

## 2. cmix / PAQ lineage — Hutter Prize SOTA

**Project:** cmix (Byron Knoll) and PAQ8 family
**URLs:** https://www.byronknoll.com/cmix.html, http://prize.hutter1.net/

**Finding:** Context mixing with hundreds of sub-models (text, byte-pair, word, hash, match, run, NN) won the Hutter Prize milestones in 2024–2025. fx2-cmix preprocessing won €7950 for a 1.59% improvement.

**Fit for omnizip-rs: ✅ Directly relevant to ZPAQ crate**
- Our `omnizip-zpaq` already implements context mixing with logistic mixer + SGD adaptation
- Gap: only 4 sub-models today (order-0/1/2 + match)
- Enhancement: add more models — order-3, word-level, hash-context, run-length

**Concrete TODOs:** see `TODO.complete/80-zpaq-more-models.md`

---

## 3. Learned compression with random access — ICDE 2025

**Paper:** *Learned Compression of Nonlinear Time Series With Random Access* (Ferragina et al., ICDE 2025)
**URL:** https://arxiv.org/html/2412.16266v1

**Finding:** "Titchy" — a dictionary-based learned compressor for IoT time series that supports random access. Trains a small dictionary on representative data; encodes new samples by nearest-neighbor lookup.

**Fit for omnizip-rs: ✅ Highly relevant**
- LimniFS is a content-addressed FS — random access is a first-class requirement
- Our ZSTD dictionary support (`omnizip-zstd::compress_with_dict`) is the foundation
- Gap: no dictionary TRAINER in omnizip-rs — only consumes pre-built dicts
- Enhancement: implement `dict_trainer` (already skeleton'd at `omnizip-zstd/src/dict_trainer.rs` but incomplete) using FastCover algorithm

**Concrete TODOs:** see `TODO.complete/81-zstd-dict-trainer.md`

---

## 4. SIMD acceleration in Rust — 2025 state

**Article:** *The State of SIMD in Rust in 2025* (Shnatsel)
**URL:** https://shnatsel.medium.com/the-state-of-simd-in-rust-in-2025-32c263e5f53d

**Finding:** Four approaches ordered by effort: (1) auto-vectorization, (2) fancy iterators, (3) `std::simd` portable SIMD, (4) raw intrinsics. For compression specifically, **zlib-rs** demonstrates 2-3x speedup over stock zlib using explicit intrinsics + auto-vectorization combo.

**Fit for omnizip-rs: ✅ Already on roadmap (`TODO.omnizip-rs/32-simd-acceleration.md`)**
- `#![forbid(unsafe_code)]` is workspace-wide — `std::simd` is the only path
- Highest-ROI targets:
  - **CRC-32 / Adler-32** — table-based; trivial to SIMD
  - **XXHash-64** — already used in ZSTD; SIMD-friendly
  - **Huffman decode** — table lookup pattern
  - **Match finder hash chains** — memcmp is the hot path
- Lower-ROI but possible:
  - BWT (bzip2) — suffix sort is hard to SIMD
  - LZ77/LZMA literal decoder — branch-heavy

**Concrete TODOs:** see `TODO.complete/82-simd-crc32-xxhash.md` and `TODO.complete/83-simd-huffman-decode.md`

---

## 5. Multi-byte ANS encoding — ACM 2024

**Paper:** *Efficient and Portable ANS Encoding for Multi-Byte Integer Sequences*
**URL:** https://dl.acm.org/doi/10.1145/3712285.3759825

**Finding:** Variants of FSE/rANS that consume 2/4/8-byte integers per step instead of one byte. ~30% throughput improvement with negligible ratio cost.

**Fit for omnizip-rs: ✅ Applicable to ZSTD FSE**
- Our FSE decoder processes one symbol per state transition
- Multi-byte variant would speed up sequence-table decoding
- Requires careful renormalization math

**Concrete TODOs:** see `TODO.complete/84-multibyte-fse.md`

---

## 6. Hybrid lossless + lossy outperforms pure lossless

**Paper:** *State-of-the-Art Trends in Data Compression* (MDPI Entropy, 2024)
**URL:** https://www.mdpi.com/1099-4300/26/12/1032

**Finding:** Combining lossy preprocessing (e.g., DCT) with lossless back-end compression can outperform pure-lossless approaches on real-world multimedia.

**Fit for omnizip-rs: ❌ Out of scope**
- omnizip-rs is lossless-only by design
- LimniFS dedup requires byte-exact reconstruction
- Document this constraint in the README

---

## 7. Convergent encryption + dedup — 2024–2025

**Paper:** *Convergent Encryption Enabled Secure Data Deduplication* (Wiley, 2024)
**URL:** https://onlinelibrary.wiley.com/doi/10.1002/cpe.8205

**Finding:** Convergent encryption (CE) — where the encryption key is derived from the plaintext hash — enables cross-user dedup while preserving confidentiality. Modern CE schemes (CE-1, CE-2, Dekey) address known attacks.

**Fit for omnizip-rs: ✅ Informational**
- omnizip-rs is the codec layer; CE lives in the storage layer
- But: documenting this in the omnizip-rs README clarifies the architectural split
- LimniFS already uses `DropId = BLAKE3(plaintext)` which IS convergent in spirit

**Action:** document the relationship in CLAUDE.md and `TODO.complete/85-convergent-encryption-note.md`

---

## 8. Lossless compression for ML workloads — Shannonic

**Paper:** *Efficient Entropy-Optimal Compression for ML Workloads*
**URL:** https://openreview.net/pdf?id=NhMxI0GbB8

**Finding:** Shannonic achieves entropy-optimal compression for ML tensor data using only ~530 bytes of state for combined encoding. Targets quantized LLM weights specifically.

**Fit for omnizip-rs: ⚠️ Medium**
- ML tensor data is a growing workload for content-addressed storage
- Existing ZSTD dictionary support handles this case OK
- Future: dedicated tensor codec? Probably not — ZSTD with dict is fine

---

## 9. Silesia / Enwik8 benchmark gaps in omnizip-rs

**Paper:** *Performance Evaluation of Efficient Hybrid Compression* (arXiv 2025)
**URL:** https://arxiv.org/html/2504.20747v1

**Finding:** Comprehensive 2025 evaluation of LZMA, ZSTD, Brotli, BZip2 — exactly omnizip-rs's portfolio. Uses standard corpora (Silesia, Enwik8, Calgary).

**Fit for omnizip-rs: ✅ Critical**
- **omnizip-rs has no benchmark suite today** (`TODO.omnizip-rs/30-benchmark-suite.md` is still open)
- Without benchmarks, we cannot demonstrate we're competitive
- Action: build `omnizip-bench/` crate that runs Silesia + Enwik8 + Calgary across all 17 codecs and produces a CSV report

**Concrete TODOs:** see `TODO.complete/86-benchmark-suite.md`

---

## 10. Hardware co-design — NetZIP (MICRO 2025)

**Paper:** *NetZIP: Algorithm/Hardware Co-design of In-network Lossless Compression*
**URL:** https://research.ibm.com/publications/netzip-algorithmhardware-co-design-of-in-network-lossless-compression-for-distributed-large-model-training

**Finding:** Custom hardware for in-network compression in distributed AI training. 5x throughput vs software.

**Fit for omnizip-rs: ❌ Out of scope**
- We target general-purpose CPUs only
- But: confirms the algorithm choices (LZ4/ZSTD) are well-aligned with industry direction

---

## Synthesis — prioritized roadmap

Ranked by impact × feasibility × omnizip-rs fit:

| # | Enhancement | Impact | Effort | Status |
|---|-------------|--------|--------|--------|
| 1 | ZSTD dictionary trainer (FastCover) | High | Medium | TODO 81 |
| 2 | Benchmark suite (Silesia + Enwik8) | High | Medium | TODO 86 |
| 3 | SIMD CRC-32 / XXHash-64 | High | Low | TODO 82 |
| 4 | ZPAQ more sub-models | Medium | Medium | TODO 80 |
| 5 | Multi-byte FSE | Medium | High | TODO 84 |
| 6 | SIMD Huffman decode | Medium | High | TODO 83 |
| 7 | Differential harness vs C reference | High | Medium | TODO 87 |
| 8 | Architecture audit (OCP/MECE) | Medium | Low | TODO 88 |
| 9 | Spec coverage analysis | Medium | Low | TODO 89 |
| 10 | Document convergent-encryption boundary | Low | Low | TODO 85 |

---

## References

- Delétang, G. et al. (2024). *Language Modeling Is Compression.* ICLR 2024. https://arxiv.org/abs/2309.10668
- Knoll, B. (2024). *cmix.* https://www.byronknoll.com/cmix.html
- Ferragina, P. et al. (2025). *Learned Compression of Nonlinear Time Series With Random Access.* IEEE ICDE 2025. https://arxiv.org/html/2412.16266v1
- Davidoff, S. (2025). *The State of SIMD in Rust in 2025.* https://shnatsel.medium.com/the-state-of-simd-in-rust-in-2025-32c263e5f53d
- Trifecta Tech (2024). *zlib-rs: SIMD-accelerated Rust compression.* https://trifectatech.org/initiatives/data-compression/
- Kosolobov, D. (2022). *Efficiency of ANS Entropy Encoders.* https://arxiv.org/pdf/2201.02514
- ACM (2024). *Efficient and Portable ANS Encoding for Multi-Byte Integer Sequences.* https://dl.acm.org/doi/10.1145/3712285.3759825
- MDPI Entropy (2024). *State-of-the-Art Trends in Data Compression.* https://www.mdpi.com/1099-4300/26/12/1032
- Wiley (2024). *Convergent Encryption Enabled Secure Data Deduplication.* https://onlinelibrary.wiley.com/doi/10.1002/cpe.8205
- MaskRay (2025). *Benchmarking Compression Programs.* https://maskray.me/blog/2025-08-31-benchmarking-compression-programs
- arXiv (2025). *Performance Evaluation of Efficient Hybrid Compression.* https://arxiv.org/html/2504.20747v1
- Hutter Prize. http://prize.hutter1.net/

---

# Update — 2026 academic literature

A new wave of 2026 papers and conferences. Adding 6 new items below.

## 11. Tsai 2026 — Revisiting Data Compression with Language Modeling

**Paper:** *Revisiting Data Compression with Language Modeling*
**Author:** Chen-Han Tsai (2026)
**URL:** https://arxiv.org/abs/2601.02875

**Finding:** Continues the DeepMind 2024 line of work on LLM-as-compressor.
Explores different methods to achieve lower adjusted compression rate
using LLMs. Cited by 1 — recent.

**Fit for omnizip-rs: ❌ Same constraints as DeepMind 2024**
- Requires LLM weights
- Non-deterministic across GPU architectures
- Incompatible with content-addressed determinism

## 12. 2026 Algorithmic Information Theory Data Compression Challenge

**Paper:** *The 2026 Algorithmic Information Theory Data Compression Challenge*
**URL:** https://arxiv.org/abs/2606.17712

**Finding:** New 2026 benchmark for general-purpose lossless compression.
16 heterogeneous files, 117 valid submissions. Standard metrics:
compression ratio, encode time, decode time. Public training set +
hidden test set.

**Fit for omnizip-rs: ✅ Critical**
- This is a current SOTA benchmark suite
- We should download the corpus and add to our `omnizip-bench` (TODO 86)
- Compare omnizip-rs ratios against the leaderboard
- Identify whether our codecs are competitive or lagging

**Action:** update TODO 86 to include the AIT 2026 corpus alongside
Silesia / Enwik8 / Calgary.

## 13. ZipServ — ASPLOS 2026

**Paper:** *ZipServ: Fast and Memory-Efficient LLM Inference with Hardware-Aware Lossless Compression*
**Authors:** Ruibo Fan et al. (HKUST Guangzhou)
**URL:** https://arxiv.org/abs/2603.17435
**Code:** https://github.com/HPMLL/ZipServ_ASPLOS26

**Finding:** First hardware-aware lossless compression framework co-designed
for LLM inference on GPUs. Reduces model size by up to 30% while
accelerating inference.

**Fit for omnizip-rs: ⚠️ Informational**
- omnizip-rs targets CPU; ZipServ is GPU-specific
- But the insight (compression designed for hardware decompressors)
  is applicable: if we ever target embedded CPU/GPU, the same
  co-design principle applies
- Document as architectural inspiration for future SIMD work

## 14. LDG-PCGC — Lossless point cloud compression

**Paper:** *LDG-PCGC: Lossless Dynamically Grouped Point Cloud Compression*
**URL:** https://ieeexplore.ieee.org/abstract/document/11463998/

**Finding:** Lossless point cloud compression with 48.4% ratio. Uses
dynamic grouping + entropy coding.

**Fit for omnizip-rs: ❌ Out of scope**
- Point clouds are a domain-specific data type (3D scanning, LiDAR)
- No plans for a 3D-geometry codec in omnizip-rs
- However, if LimniFS ever stores volumetric data, this could
  become relevant

## 15. DCC 2026 (Data Compression Conference)

**Venue:** DCC 2026, Snowbird, Utah, March 24–27, 2026
**URL:** https://signalprocessingsociety.org/events/2026-dcc-2026-data-compression-conference
**Proceedings:** IEEE, ISBN 979-8-3315-8261-6

**Finding:** Premier compression venue. 2026 papers cover:
- Volumetric data compression
- Point cloud geometry compression
- Improvements to LZ77, ANS, arithmetic coding

**Fit for omnizip-rs: ✅ Worth tracking**
- DCC papers often become the next RFCs / codec standards
- No immediate action — bookmark the proceedings page and review
  quarterly

## 16. LLM-generated data lossless compression

**Paper:** *Lossless Compression of Large Language Model (LLM)-Generated Data*
**URL:** https://arxiv.org/html/2505.06297v1

**Finding:** LLM-generated text has different statistical properties
than human text — more repetitive, more "templated". Standard
compressors underperform on it. Paper proposes LLM-aware dictionary
preprocessing.

**Fit for omnizip-rs: ⚠️ Emerging**
- If LimniFS users store AI-generated content (likely!), our codecs
  should be tuned for it
- Action: add LLM-generated corpus to benchmark suite (TODO 86)
- The "in-context dictionary" idea from arXiv 2604.13066 (de Campos
  2026) is essentially our existing ZSTD dictionary support — we
  already have the infrastructure

## 17. L3TC — RWKV-based learned text compression

**Paper:** *L3TC: Leveraging RWKV for Learned Lossless Low-Complexity Text Compression*
**URL:** https://github.com/alipay/L3TC-leveraging-rwkv-for-learned-lossless-low-complexity-text-compression

**Finding:** 48% bit saving vs gzip. RWKV is a small RNN architecture
(50× smaller than other learned compressors).

**Fit for omnizip-rs: ⚠️ Research project**
- RWKV is small enough that we COULD embed it (~10 MB)
- Still requires model weights
- Still non-deterministic on different hardware
- Future candidate if we ever relax determinism for offline-only use

---

## Synthesis — 2026 prioritized roadmap

Adding to the 2024–2025 roadmap:

| # | Enhancement | Impact | Effort | Status |
|---|-------------|--------|--------|--------|
| 90 | Add AIT 2026 corpus to benchmark suite | High | Low | TODO 86 (updated) |
| 91 | Add LLM-generated text corpus to benchmark | Medium | Low | TODO 86 (updated) |
| 92 | Track DCC 2026 proceedings quarterly | Low | Low | process |
| 93 | Track ZipServ GPU insights for future SIMD work | Low | Low | process |

---

## Updated references (2026)

- Tsai, C.-H. (2026). *Revisiting Data Compression with Language Modeling.* arXiv:2601.02875.
- (2026). *The 2026 Algorithmic Information Theory Data Compression Challenge.* arXiv:2606.17712.
- Fan, R. et al. (2026). *ZipServ: Fast and Memory-Efficient LLM Inference with Hardware-Aware Lossless Compression.* ASPLOS 2026. arXiv:2603.17435.
- (2026). *LDG-PCGC: Lossless Dynamically Grouped Point Cloud Compression.* IEEE.
- DCC 2026. *Data Compression Conference proceedings.* IEEE, ISBN 979-8-3315-8261-6.
- (2025). *Lossless Compression of Large Language Model (LLM)-Generated Data.* arXiv:2505.06297.
- de Campos, A.R. (2026). *Enabling Cost-Effective LLM Analysis of Repetitive Data via In-Context Dictionary.* arXiv:2604.13066.
- Zhang, J. (2025). *L3TC: Leveraging RWKV for Learned Lossless Low-Complexity Text Compression.* AAAI 2025.
