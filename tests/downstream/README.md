# Downstream conformance gate (LimniFS)

Runs the **LimniFS test suite** against the current omnizip-rs
workspace, in our CI, on every PR and push to main.

## Why

Every LimniFS validation pass has caught omnizip regressions our own
suites missed:

- **#388** — `MATCH_LEN_CAP` 1,951 → 65,536 hung their windows-latest
  CI for 23+ minutes on repetitive structured text (the in-code
  claim "measured: 65536 is safe" did not generalize across machines
  or content).
- **#408** — an effectively-uncapped default (16,779,211) repeated
  the same class with a larger ceiling.

Their whole-file drop tests exercise content classes and timing
envelopes our unit tests don't. Instead of discovering that in the
next downstream "Pass N" summary, this harness makes their suite our
gate: it fails the PR that would have broken them.

## How it works

`run.sh`:

1. Clones `limnifs/limnifs` at the commit pinned in
   [`limnifs-ref.txt`](limnifs-ref.txt) (same convention as the Ruby
   differential harness's `ruby-ref.txt`).
2. Writes `[patch.crates-io]` entries into the clone's
   `.cargo/config.toml` mapping every `omnizip-*` crate to this
   workspace — the clone's own version pins are caret-style, so the
   evergreen workspace satisfies them without editing downstream
   files.
3. Runs `cargo test --release --workspace` in the clone.

The GitHub job lives in
[`.github/workflows/downstream.yml`](../../.github/workflows/downstream.yml)
(45-minute timeout — the pathological-hang class this gate exists to
catch shows up as a timeout, which is a failure).

## Bumping the pin

Update `limnifs-ref.txt` to the downstream commit you want to conform
to. Bump it deliberately, when LimniFS lands new tests worth gating
on (especially regression tests for newly-filed omnizip issues) —
new downstream coverage should flow into this gate promptly.

## In-tree companions

Codec-level fixtures for the known pathological classes live in the
crates themselves (fast, always run):

- `omnizip-brotli` — `match_len_cap_default_is_bounded`,
  `repetitive_structured_text_q11_completes_and_round_trips` (the
  #388/#408 content class)
