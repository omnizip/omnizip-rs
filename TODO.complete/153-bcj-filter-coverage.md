# TODO 153: BCJ filter coverage — ARM/ARM64/IA64/SPARC/PPC

## Problem

`omnizip-filters` implements BCJ-x86 only. LimniFS stores binaries
from multiple architectures; without BCJ filters, LZMA ratio on
ARM64 ELF binaries is 5-10% worse than necessary.

## Scope

Per the XZ utils spec:

- **BCJ-ARM**: ARM little-endian call/branch instructions.
- **BCJ-ARM64**: AArch64 branch instructions.
- **BCJ-IA64**: Itanium bundle templates.
- **BCJ-SPARC**: SPARC v9 call/branch.
- **BCJ-PPC**: PowerPC big-endian branch.

Each is a ~100 LOC filter that scans for branch instructions and
replaces the absolute target with a relative one.

## Acceptance criteria

- [ ] All 5 filters land in `omnizip-filters`.
- [ ] Each round-trips: filter then unfilter gives original bytes.
- [ ] LZMA compression ratio on the filtered binary improves ≥ 3%
  vs unfiltered on each architecture's test fixture.

## Priority

P2 — only matters for users compressing non-x86 binaries.
