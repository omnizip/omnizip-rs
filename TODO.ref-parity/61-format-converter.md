# Task 61 — format converter

Status: open (part of task 51 phase 3; composes existing container
readers/writers — this is orchestration, not new wire formats)

## Gap

Ruby `converter/`: `ConversionRegistry` + `ConversionStrategy`
protocol with `extract_repack_strategy` (generic any→any via
extract-to-temp + repack) and dedicated `zip_to_seven_zip` /
`seven_zip_to_zip` strategies; top-level `convert`,
`convert_with_options`, `supported?`, `strategies`, `batch_convert`
(with per-item yield for progress). Rust: nothing.

## Scope

- `convert.rs` in omnizip-archive-core (or `omnizip-convert` crate
  if archive-core should stay I/O-shape-agnostic — prefer the new
  crate to keep archive-core's dependency graph flat):
  - `ConversionRegistry`: (source_format, target_format) → strategy,
    with the generic `ExtractRepack` strategy as fallback and
    dedicated strategies where metadata mapping matters.
  - Entry-at-a-time repack (reader → writer streaming per entry)
    with bounded memory: no full-archive buffering; spillover via
    the task 57 chunked writer when a single entry exceeds the
    budget.
  - Metadata preservation matrix (mtime, perms, symlinks, dirs,
    comments) per format pair — port the Ruby strategies'
    mappings; lossy cells documented loudly.
  - `batch_convert` with the task 59 reporter.
- ozip: `ozip convert <src> <dst> [--profile p]` (profile-aware
  target level when 60 lands).

## Acceptance

- zip → 7z → zip round-trip: entry set + contents + mtimes/perms
  preserved (fixture corpus with symlinks, empty dirs, large file).
- Output determinism: same inputs → byte-identical target archive
  (container writers are already deterministic — keep it so).
- Bounded memory: conversion of an archive with one entry larger
  than the declared budget completes via spillover (CI proxy:
  budget smaller than the test fixture).
- `supported?` matrix test mirroring the Ruby strategy registry.

## References

`../omnizip/lib/omnizip/converter/*.rb` + `converter.rb` (API shape).
