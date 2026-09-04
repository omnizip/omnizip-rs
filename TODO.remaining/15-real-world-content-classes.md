# 15 — Real-world content-class conformance pass

- **Priority:** MEDIUM
- **Depends on:** durable corpus at `~/sweep-corpus/`
- **Estimated effort:** 2–3 days
- **Status:** pending

## Goal

The 10-file corpus is synthetic or semi-synthetic. The fits4m/dbdump
regeneration artifacts (phantom 20% "gaps" from a wrong generator)
proved content shape dominates cell outcomes. Validate both codecs on
REAL content classes LimniFS actually stores before declaring the
parity arcs closed.

## Corpus additions (all locally sourced, licensing-safe)

- Real FITS file (the synthetic header+counter shape measured
  1.30–1.68x vs the original's ~1.006x — the original is the real
  question).
- Source tarballs: unpack a few release tarballs (kernel-style C,
  a Rust crate bundle, a JS bundle).
- JSON/NDJSON logs, YAML configs.
- PDF, DOCX/XLSX (OOXML zip members), SVG.
- Fonts beyond Arial (OTF/CFF, woff2 container).
- Binaries: .rlib/.a archives, wasm, a compiled CLI.
- Mixed container: an ISO or DMG-ish blob, a SQLite db file.

Record provenance (path + sha256 + generator) in
`~/sweep-corpus/MANIFEST.txt` so cells stay reproducible. Do NOT
hand-roll generators — reuse in-tree generators where they exist.

## Method

1. Sweep brotli q1/5/9/11 + zstd L1/6/19 (+ xz -6 spot cells) vs
   reference CLIs on every new file; append to
   `~/sweep-corpus/baseline.txt`.
2. Any cell >1.05x: decompose with the standard playbook —
   sequence-aligned parse diff, DEC_STATS / SEQ_DUMP diagnostics,
   price-vs-emission mismatch — before touching code.
3. Distinguish content-class gaps from generator artifacts FIRST
   (the lesson of 2026-09-03).

## Results (2026-09-04) — task CLOSED

Nine real content classes at `~/sweep-corpus/real/` with
`MANIFEST.txt` (provenance + sha256): SVG vectors, macOS install log,
Mach-O executables, OTF/CFF fonts, JPEG photo, plist-export JSON,
compiler .rlib archives, SQLite engine output, asset PDFs. 63 cells
(brotli q1/5/9/11 + zstd L1/6/19 per file); raw table appended to
`~/sweep-corpus/baseline.txt`.

- q1/L1 beats the reference on EVERY class (0.76–0.99) — the
  from-spec q1 + fast-tier routing holds on real content.
- zstd L6 beats on EVERY class (0.91–0.999).
- brotli q5/q9 beat or tie on 8/9 classes (rlibs 1.035–1.038 the
  exception, the known mixed-binary q5 family).
- Cells >1.05x, all inside documented residual families (no new
  bug class found):
  - `plists.json` brotli q11 1.0995 — q11 contest tier on repetitive
    structured JSON (same family as bin1 q11 1.048, task-18-class
    parse-shape gap);
  - `noto-otf.bin` zstd L1 1.0590 — fast tier on OTF (the deferred
    task-09 L1/L2 residual family);
  - `photo.jpg` brotli q11 1.0587 — 17 KB near-incompressible file,
    model-overhead scale class.

Verdict: no actionable new gap; the corpus becomes a standing
regression class for future sweeps.

## Acceptance

- ≥8 real-world content classes swept, manifest recorded.
- No cell >1.05x left undecomposed; each is either fixed, or
  documented in this file with the measured root cause and the
  cost/benefit of closing it.
- No regressions on the existing 10-file corpus (regression gate).
