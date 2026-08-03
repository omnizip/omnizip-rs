# Audio corpus — FLAC LPC verification (TODO 105)

This directory hosts the FLAC LPC verification corpus described in
`TODO.complete/105-flac-lpc-finish.md`. It is **not** part of the
omnizip-rs source tree; LimniFS contributes the corpus-fetching
script and runs the differential harness here.

## Layout

```
tests/audio_corpus/
├── README.md           ← this file
├── fixtures/           ← corpus files (not in git; .gitignore'd)
│   ├── classical/      ← MusOpen public-domain WAVs (~500 MB)
│   ├── speech/         ← LibriSpeech dev-clean (~200 MB)
│   ├── ambient/        ← Free Music Archive CC-licensed (~300 MB)
│   ├── pop/            ← Internet Archive 78rpm (~200 MB)
│   └── synthetic/      ← swept sine, white/pink noise (~50 MB)
├── fetch.sh            ← LimniFS-provided corpus fetcher (TBD)
└── run_audio_bench.sh  ← omnizip-rs-provided differential script
```

## Differential script

`run_audio_bench.sh` (committed here) iterates every `.wav` file
under `fixtures/` and compares:

1. **omnizip-flac** via the `omnizip-flac::compress` API.
2. **libFLAC** via the `flac` CLI binary (subprocess; not built into
   omnizip-rs).
3. **LZ4** via `lz4 --best` (binary fallback baseline).
4. **ZSTD L12** via `zstd -12` (high-ratio baseline).

The script writes a CSV with one row per fixture. The acceptance
criteria (per TODO 105) require omnizip-flac to:

- Be within 5% of libFLAC on ≥ 95% of tracks
- Beat LZ4 by ≥ 10% on ≥ 90% of tracks
- Beat ZSTD L12 by ≥ 10% on ≥ 80% of tracks

## Skipping when fixtures are absent

The differential tests in `tests/audio_corpus/tests.rs` skip cleanly
when `fixtures/` is empty (so CI in omnizip-rs doesn't fail when the
corpus hasn't been fetched). Run `./fetch.sh` (provided by LimniFS)
before the tests to populate `fixtures/`.

## License

The corpus fixtures are **not** part of the omnizip-rs source
release. The scripts in this directory are MIT/Apache-2.0 (same as
the rest of omnizip-rs).
