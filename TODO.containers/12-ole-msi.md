# 12 — OLE compound documents + MSI (read)

- **Priority:** P2
- **Depends on:** [01](01-archive-core.md); MSI extraction needs CAB (Cabriolet — see note)
- **Estimated effort:** 3 weeks
- **Crate:** `omnizip-ole`

## Goal

OLE2 compound files (.doc/.xls/.ppt/.msi): FAT/minifat sector chains,
directory entries, stream read/write. On top of it, MSI read: string pool,
table parser (File/Component/Directory/Media), directory resolver, embedded
cabinet extraction.

## Note: the CAB dependency

MSI payloads live in embedded CABs, which the Ruby ecosystem assigns to
**Cabriolet**, not omnizip. Options:
(a) port the needed CAB-read subset here (duplication with a future
    cabriolet-rs),
(b) start a sibling `cabriolet-rs` crate and depend on it.

**Recommendation:** (b) — same family rules, and Excavate-rs will need full
CAB anyway. Record the decision in this file when made.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/ole/` (sectors, directory, streams) | `ole/{sector,directory,stream}.rs` | 2,129 |
| `formats/msi/` (string pool, tables, resolver, extractor) | `msi/*.rs` | 1,451 |

## Acceptance

- [ ] OLE read/write: stream round-trip; 7-Zip lists our compound files
- [ ] MSI: extract files from real .msi fixtures byte-exactly (the Excavate
      test corpus is the oracle)
