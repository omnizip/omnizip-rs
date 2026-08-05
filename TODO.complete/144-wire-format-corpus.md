# TODO 144: Wire-format conformance corpus

## Problem

Each codec has a handful of test fixtures. Real-world conformance
testing requires a curated corpus of:

- Reference encoder outputs at every level.
- Edge cases the spec mandates (empty input, 1-byte input,
  max-length matches, max-distance matches).
- Adversarial inputs that historically broke decoders.

The workspace has no central corpus; tests inline small fixtures.

## Proposed fix

`tests/corpus/{codec}/{category}.bin` with a manifest listing what
each fixture exercises:

```
tests/corpus/lzma/empty.xz           — empty input
tests/corpus/lzma/single-byte.xz     — 1-byte input
tests/corpus/lzma/max-match.xz       — 273-byte match
tests/corpus/lzma/max-distance.xz    — distance at dict_size limit
tests/corpus/lzma/random.xz          — incompressible
tests/corpus/lzma/text.xz            — enwik-like
tests/corpus/lzma/binary.xz          — ELF-like
tests/corpus/lzma/adversarial-01.xz  — historical decoder crash
```

Each codec's tests load the corpus + verify round-trip.

Generate fixtures once via reference tools (xz -9, zstd -19, etc.)
and commit them.

## Acceptance criteria

- [ ] Corpus lands under `tests/corpus/`.
- [ ] `tests/corpus/README.md` documents each fixture.
- [ ] Each codec's test suite loads + verifies the corpus.
- [ ] Generation script in `tests/corpus/generate.sh` for
  reproducibility.

## Priority

P2 — improves test coverage but not a correctness gap.
