# Task 61 — format converter

Status: done + batch (2026-09-19 second pass — `ozip convert SOURCE... DIR -f fmt` batch form (Ruby batch_convert): unambiguous arity rule (2 paths = single, >=3 = batch with dir last) so a mistyped batch can never silently degrade into single mode and overwrite a source; convert owns its arg shape via convert_command (the router no longer strips its first path). The Ruby 'dedicated' zip<->7z strategies are THEMSELVES extract-repack (Dir.mktmpdir + repack_tree — read them before assuming entry-copy); entry-at-a-time has no Ruby counterpart and remains an optional optimization. Original:
the Ruby ExtractRepackStrategy. Source = ANY openable archive incl.
the task-53 single-file views; target = every container create
format. Metadata via fs round-trip (mtimes/modes/symlinks/empty
dirs); staging dir named from the source stem so output is
BYTE-DETERMINISTIC across runs (pinned). Lossy cells documented:
hardlinks materialize, Other kinds skip, single-file sources wrap
under '<stem>.converted/'. The dedicated zip<->7z entry-at-a-time
strategies + batch_convert + bounded-memory spillover remain as
follow-ups (need task-57 streaming); the ConversionRegistry shape
is one match on OutputFormat — add strategies when entry-at-a-time
lands

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
