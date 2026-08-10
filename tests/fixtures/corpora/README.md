# Test corpora

Real-world compression benchmarks live here. All files are
**gitignored** — too large for the repo. Run `setup.sh` to
download on demand.

## Available corpora

| Directory | Source | Size | License |
|---|---|---|---|
| `silesia/` | [Silesia corpus](https://sun.aei.polsl.pl/~sdeor/index.php?page=silesia) | ~200 MB | research/educational |
| `enwik8/` | [Matt Mahoney's site](https://mattmahoney.net/dc/textdata.html) | 100 MB | GFDL/GPL |
| `calgary/` | [Calgary corpus](https://www.data-compression.info/Corpora/CalgaryCorpus/) | ~3 MB | public domain |
| `canterbury/` | [Canterbury corpus](https://corpus.canterbury.ac.nz/) | ~3 MB | public domain |
| `limnifs/` | Provided by LimniFS user | varies | (user) |

## Setup

```bash
./setup.sh                # download all
./setup.sh silesia        # download one
./setup.sh --list         # list what's available
```

Downloads require `curl` and `tar`. CI runs corpora benchmarks only
when `OMNIZIP_RUN_CORPORA=1` is set.

## Why real corpora?

Synthetic test inputs (random bytes, repeated phrases) catch round-
trip bugs but not ratio regressions on the data shapes users actually
have. Silesia is the de-facto compression benchmark; published
numbers from Brotli/ZSTD/LZMA papers use it.

Without these corpora, "10% ratio improvement" is a claim about
synthetic data that may not generalize. See ADR-0005 (differential
parity) and TODO 247 for the rationale.
