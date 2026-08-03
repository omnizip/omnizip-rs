# 105 — FLAC LPC verification corpus (finish criteria)

**Priority:** Medium — feature gap
**Source:** LimniFS proposal `omnizip-proposals/flac-lpc-finish.md`
**Status:** ⏳ Pending — corpus script + acceptance criteria; PR TBD

## Problem

`omnizip-flac` has had multiple rounds of LPC work (Phase 1, 2, 2B,
3 per the omnizip-rs commit log). TODO 98 was marked fixed.
LimniFS still sees FLAC routing disabled in production because
earlier revisions produced output that, while valid, lost to
general-purpose codecs on some audio fixtures.

The LimniFS `pcm_audio` categorizer is **off by default** today.
LimniFS wants to enable it but needs confidence the FLAC encoder
wins on a broad corpus, not just the synthetic sine waves used in
omnizip-flac's own tests.

## Proposed verification suite

Build a 200-track audio corpus spanning:

| Genre     | Source                              | Approx size |
|-----------|-------------------------------------|-------------|
| Classical | MusOpen public-domain WAVs          | 500 MB      |
| Speech    | LibriSpeech dev-clean               | 200 MB      |
| Ambient   | Free Music Archive CC-licensed      | 300 MB      |
| Pop       | Internet Archive 78rpm collection   | 200 MB      |
| Synthetic | swept sine, white noise, pink noise | 50 MB       |

For each track, compare:

1. FLAC via `omnizip-flac` (current revision).
2. FLAC via `libFLAC` reference (CLI binary, run as subprocess for
   testing only — not in source tree).
3. Plain LZ4 (LimniFS's binary fallback).
4. Plain ZSTD L12 (LimniFS's high-ratio binary).

FLAC wins iff `omnizip-flac` ratio is within 5% of `libFLAC` AND
beats both LZ4 and ZSTD L12 by ≥ 10%.

## What this TODO covers

omnizip-rs side:

1. Acceptance criteria (this file).
2. Optional: a `tests/audio_corpus/` directory structure for
   LimniFS to drop the corpus into.
3. Optional: a `tests/audio_corpus/run_audio_bench.sh` script that
   invokes omnizip-flac vs libFLAC vs ZSTD.

LimniFS side (covered by LimniFS proposal):

1. Contributes the corpus-fetching script + differential harness
   under `tests/audio_corpus/` (MIT-licensed).
2. Bumps omnizip dependency when FLAC meets acceptance.
3. Enables `pcm_audio` categorizer by default.

## Acceptance criteria

- [ ] 200-track corpus fetched and committed to a non-source
      directory (e.g. `tests/audio_corpus/`, `.gitignore`'d).
- [ ] omnizip-flac ratio within 5% of libFLAC on ≥ 95% of tracks.
- [ ] omnizip-flac beats LZ4 by ≥ 10% on ≥ 90% of tracks.
- [ ] omnizip-flac beats ZSTD L12 by ≥ 10% on ≥ 80% of tracks.
- [ ] Round-trip verified on every track.
- [ ] LimniFS enables `pcm_audio` categorizer by default.

## Effort estimate

- omnizip-rs: 2 days (acceptance + test harness).
- LimniFS: 5 days (corpus fetcher + bench script + categorizer flip).

## Related

- omnizip-rs TODO 98 (LPC interop bug — fixed).
- LimniFS proposal `omnizip-proposals/flac-lpc-finish.md`.
