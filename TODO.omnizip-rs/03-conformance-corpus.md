# 03 — Conformance corpus

- **Priority:** P0 (blocks ratio benchmarks and differential tests)
- **Depends on:** —
- **Estimated effort:** half a day
- **Location:** `tests/corpus/` (git LFS for large files)

## Goal

Curate a standard corpus of test inputs covering every compression
scenario: text, code, binary, structured, random, repetitive, mixed.
Every codec's ratio claim and conformance gate runs against this corpus.

## Corpus composition

| Category | File | Size | Origin | License |
|---|---|---:|---|---|
| **Text** | | | | |
| | dickens.txt | 10 MB | Silesia | permissive |
| | enwik9 | 1 GB | Mahoney | GPL (CI downloads; not redistributed) |
| | wikipedia-xml.html | 100 MB | Silesia | CC-BY-SA |
| **Source code** | | | | |
| | linux-5.15.tar | 1.1 GB | kernel.org | GPL-2 (CI downloads) |
| | samba-4.0.tar | 200 MB | samba.org | GPL-3 (CI downloads; we don't redistribute) |
| **Structured binary** | | | | |
| | osdb.bin | 10 MB | Silesia | permissive |
| | x-ray.bin | 10 MB | Silesia | medical demo |
| **Repetitive** | | | | |
| | repetitive-aaa.bin | 1 MB | synthetic | — |
| | zeros.bin | 1 MB | synthetic | — |
| **Random / incompressible** | | | | |
| | random.bin | 1 MB | `/dev/urandom` | — |
| **Mixed real-world** | | | | |
| | mozilla-build.tar | 50 MB | Silesia | MPL |
| | mr-models.bin | 5 MB | Silesia | permissive |
| **Edge cases** | | | | |
| | empty | 0 B | synthetic | — |
| | one-byte | 1 B | synthetic | — |
| | all-0xff | 1 KB | synthetic | — |
| | highly-skewed-huffman | 10 KB | synthetic | — |

## Why these

- **Silesia** is the standard compression benchmark corpus used by zstd,
  brotli, and DwarFS. Ratio numbers are comparable across codecs.
- **enwik9** stresses entropy coding on natural language.
- **linux-5.15.tar** stresses mixed content (source + build artifacts +
  text) inside a tar container.
- **Edge cases** catch off-by-one errors, empty-input panics, and
  pathological small-file behavior.

## Air-gapped redistribution

The corpus lives in `tests/corpus/` for small files (< 10 MB) and in Git LFS
for medium files (10–200 MB). Large files (> 200 MB) are NOT redistributed;
CI downloads them on first run and caches in `$CI_CACHE`.

This keeps the repo clone small for LimniFS's air-gapped build scenario —
the corpus is dev-only, gated behind `[dev-dependencies]`.

## Acceptance

- `tests/corpus/` contains all small + edge-case files committed directly.
- `tests/corpus/LARGE_FILES.md` documents download URLs and SHA-256 for each
  large file.
- CI caches large files across runs to avoid re-downloading.
- `tests/corpus/README.md` lists every file with its category, size, and
  license.

## Implementation notes

- Git LFS is configured in `.gitattributes` for `*.bin` and `*.tar` over
  10 MB.
- The benchmark suite (task 30) iterates over `tests/corpus/` and produces
  a JSON report keyed by `(codec, level, file)`.
- A differential-test variant (task 02) iterates the same files through
  Ruby + Rust for encode parity.
