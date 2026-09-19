# Task 53 — xz / lzip / lzma-alone as first-class single-file formats

Status: done (2026-09-19 — detection + lzip/alone/gzip/bzip2 modules
already existed (SSOT survey); this added: the single-file archive
view (t/l/x see one entry; payload tar still unwraps first),
`ozip c -f gzip|bzip2|xz|zstd|lzip|lzma` single-file create (Ruby
compress_command semantics; dir/multi-input = guidance error), and
the CodecSpec table moved into container.rs as the SSOT for both
`-d` codec mode and the archive view. Pin test in ozip/tests/cli.rs;
system gzip/xz/bzip2 decode our output. Remaining follow-up noted:
gzip FNAME is not extracted on READ for entry naming — suffix
stripping is used for all formats)

## Gap

Ruby surfaces `formats/{gzip,bzip2_file,lzip,lzma_alone,xz}.rb` as
first-class formats (detect, read, write, metadata). Rust:
archive-core covers gzip/bzip2 single files; xz/lzip/lzma-alone live
inside omnizip-lzma but are NOT exposed as archive formats — ozip
cannot `t/l/x/c` them.

## Scope

In omnizip-archive-core:

- Extend the format detector: xz magic `FD 37 7A 58 5A`, lzip
  magic `LZIP` + version byte, lzma-alone (13-byte props header —
  heuristic; document the ambiguity, same as Ruby).
- Single-entry `FormatHandler`s delegating to omnizip-lzma's
  existing encode/decode paths (all three already exist there).
- Metadata surface per format, mirroring the Ruby specs: gzip
  (name/mtime fields — verify our writer's behavior against Ruby),
  lzip (dict-size byte), alone (props lc/lp/pb + size field),
  xz (check type, stream flags).
- ozip: `c` gains `--format xz|lzip|lzma-alone` (levels map to the
  lzma crate's levels); `t/l/x` work by detection.

## Acceptance

- Round-trip + differential vs the system `xz`/`lzip` CLIs and the
  Ruby gem on the fixture corpus (`tests/differential/`).
- `ozip t` names all four single-file formats correctly on mixed
  fixtures; detection table test in archive-core.
- gzip metadata (name/mtime) parity test vs Ruby output.

## References

`../omnizip/lib/omnizip/formats/{gzip,lzip,lzma_alone,xz}.rb`,
`xz_const.rb`.
