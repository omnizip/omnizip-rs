# 89 — Spec coverage analysis

**Priority:** Medium
**Source:** CLAUDE.md ("Spec-first" invariant)

## Context

omnizip-rs ports from Ruby reference + various format specs (RFC 1950/
1951/1952, RFC 7932, LZMA spec, ZSTD RFC 8878, FLAC spec, etc.).
Currently it's hard to tell which spec clauses are covered by tests
and which aren't.

## Goal

For each codec, produce a coverage matrix mapping spec clauses →
tests. Example for DEFLATE (RFC 1951):

| Clause | Description                    | Test file                      | Status |
|--------|--------------------------------|--------------------------------|--------|
| §3.1   | Block types 00/01/10          | deflate/tests/block_types.rs   | ✅     |
| §3.2.5 | Huffman code length tables    | deflate/tests/huffman.rs       | ✅     |
| §3.2.6 | Static Huffman table          | (none)                         | ❌     |
| §3.3   | Back-pointer length/distance  | deflate/tests/match.rs         | ✅     |

This is tedious to produce but extremely valuable:
- Reveals spec clauses we never test
- Documents which codec handles which spec
- New contributors can find untested areas easily

## Approach

1. For each codec, find the authoritative spec (RFC, paper, etc.).
2. List spec clauses at section granularity.
3. Grep test files for references to clause numbers.
4. Generate `docs/spec-coverage/{codec}.md` per codec.
5. Aggregate into `docs/spec-coverage/SUMMARY.md`.

## Acceptance criteria

- [ ] `docs/spec-coverage/` directory created.
- [ ] Coverage matrix for each of: DEFLATE, Brotli, ZSTD, LZMA, XZ,
      BZip2, FLAC, Snappy, LZ4.
- [ ] Each matrix lists ≥10 spec clauses with status.
- [ ] At least 5 gaps identified and filed as new test TODOs.
- [ ] Documentation in `docs/spec-coverage/README.md`.

## Files

- `docs/spec-coverage/` — new directory
- `docs/spec-coverage/README.md` — methodology
- `docs/spec-coverage/{codec}.md` — per-codec matrix
