# 276 — Codec Determinism Audit Across Platforms

- **Priority:** P1 (LimniFS hard requirement)
- **Crate:** workspace
- **Depends on:** [147](147-determinism-cross-platform-audit.md)
- **Estimated effort:** 2 days

## Problem

LimniFS requires byte-identical compression output across platforms
for content-addressed storage. We have in-process determinism tests
but no cross-platform verification.

Drift sources that could break cross-platform determinism:
- Different libc behavior (memcpy alignment, etc.)
- Different f32 rounding (x87 SSE vs ARM NEON)
- Different thread pool scheduling
- Different allocator behavior

## Design

### Test fixtures

Pick 10 canonical inputs:
- Empty
- Single byte
- 1 KiB text
- 64 KiB CSV
- 1 MiB binary
- etc.

Compress each at 3 levels per codec. Record SHA-256 of compressed
output.

### Cross-platform workflow

```yaml
# .github/workflows/cross-platform-determinism.yml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
    rust: [stable, beta]
jobs:
  checksum:
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - run: cargo test --release --test determinism_checksums
      - uses: actions/upload-artifact@v4
        with:
          name: checksums-${{ matrix.os }}-${{ matrix.rust }}
          path: target/determinism_checksums.json
```

### Checksum comparison

A final job downloads all artifacts and verifies they're byte-identical.
If any platform produces a different checksum, the workflow fails.

## Acceptance criteria

- [ ] `determinism_checksums.json` committed with Linux/macOS/Windows
      numbers.
- [ ] CI workflow compares across all 3 platforms.
- [ ] Any drift blocks merge.
- [ ] Documentation explains the determinism contract.

## Why this matters

LimniFS dedup collapses if Linux compression differs from macOS.
Cross-platform verification makes the contract testable, not
aspirational.
