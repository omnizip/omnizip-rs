# 18 — `ozip` v0: the codec CLI

- **Priority:** P0 (parallel to the container work — depends only on shipped codecs)
- **Depends on:** codec crates (all shipped)
- **Estimated effort:** 1–2 weeks
- **Crate:** `ozip` (new workspace member)

## Naming decision (recorded)

The Ruby gem already installs an `omnizip` executable. The Rust binary is
**`ozip`** (crate `ozip`), avoiding the collision and giving a tight CLI name.
Revisit only with an explicit family-wide naming decision.

## Goal

A unified single-file compressor/decompressor — the xz(1)/zstd(1) role for
every codec we ship:

```
ozip xz|zstd|brotli|gzip|bzip2|lz4|snappy|lzip|lzma [options] [file …]
ozip --list-codecs          # registry: formats, levels, rw flags
ozip -d file.xz             # decompress (or detect from magic)
```

stdin/stdout support, `-k` keep, `-#` levels, `--threads` once parallel
encoders exist. Zero unsafe end-to-end; byte-deterministic by construction;
exit codes matching xz(1)/zstd(1) conventions.

## Implementation notes

- `clap` for args (pure Rust, ubiquitous) — first workspace CLI dep; fine,
  the no-deps rule is for codec crates.
- Subcommand per codec maps straight onto `CodecRegistry` dispatch — no
  per-codec argument code (OCP).
- Distributes via GitHub Releases + cargo-binstall metadata, `omnizip/tap`
  (Homebrew), winget manifest, Chocolatey nuspec — CI matrix (x86_64+aarch64,
  gnu+musl).

## Acceptance

- [x] Round-trip every codec; `ozip xz -6 f` decodable by `xz -d` and vice versa
- [ ] `--help` docs every codec + level range from the registry
- [ ] Release pipeline produces signed-tagged binaries; binstall works
- [x] This is also the website's install story (`brew install omnizip/tap/ozip`)
