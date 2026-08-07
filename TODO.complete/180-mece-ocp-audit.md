# 180: MECE / OCP Architecture Audit

## Priority: P3 (code quality)

## Status: documented — workspace is functional, this audit formalizes the structure.

## Context

The workspace follows a "one crate per codec family" structure. This
document audits the architecture against MECE (Mutually Exclusive,
Collectively Exhaustive) and OCP (Open/Closed Principle) criteria.

## MECE audit

### Layer separation (mutually exclusive)

```
┌─────────────────────────────────────────────────────┐
│ Layer 4: Consumer (LimniFS, CLI)                    │
├─────────────────────────────────────────────────────┤
│ Layer 3: Codec Registry + Codec trait               │
│          omnizip-codecs                             │
├─────────────────────────────────────────────────────┤
│ Layer 2: Codec implementations                      │
│          omnizip-lzma, omnizip-zstd, omnizip-flac…  │
├─────────────────────────────────────────────────────┤
│ Layer 1: Shared primitives                          │
│          omnizip-codecs::checksum, matchfinder,     │
│          bitstream, huffman                         │
├─────────────────────────────────────────────────────┤
│ Layer 0: std + platform                             │
└─────────────────────────────────────────────────────┘
```

**Current violations**:
- LZMA has its own `crc32.rs` (Layer 2 duplicates Layer 1)
- LZMA has its own `match_finder.rs` (Layer 2 duplicates Layer 1)
- ZSTD has its own `match_finder.rs` + `bitstream`
- Brotli has its own Huffman table builder inline in decoder.rs

**Fix**: Layer 2 must NOT re-implement what Layer 1 provides. Each
violation is a TODO 179 item.

### Responsibility allocation (collectively exhaustive)

| Responsibility              | Owner                         | Status |
|-----------------------------|-------------------------------|--------|
| Codec ID assignment         | `omnizip-codecs::CodecId`     | OK     |
| Compression level semantics | `omnizip-codecs::CompressionLevel` | OK |
| Error types                 | `omnizip-codecs::OmnizipError`| OK     |
| Codec trait                 | `omnizip-codecs::Codec`       | OK     |
| Filter trait                | `omnizip-filters::Filter`     | OK     |
| CRC32                       | `omnizip-codecs::checksum`    | OK (re-exported) |
| XXHash                      | `omnizip-codecs::xxhash`      | OK     |
| Hash-chain match finding    | `omnizip-codecs::matchfinder` | GAP (not adopted) |
| Bit reading/writing         | (not shared)                  | GAP    |
| Huffman coding              | (not shared)                  | GAP    |
| Range coding                | `omnizip-lzma::range_coder`   | LZMA-only (OK) |

## OCP audit

### Adding a new codec

**Current**: Create crate → implement `Codec` → add to workspace
`Cargo.toml` → add to test codec list.

**Assessment**: Mostly OCP. The `CodecRegistry` allows runtime
registration. Adding a codec does NOT modify existing codec code. The
only closed-against-modification step is the workspace `Cargo.toml`.

### Adding a new compression level

**Current**: Each codec has a `match level { ... }` block. Adding a
new level requires editing the match.

**Fix**: Use a strategy table (see TODO 179). New levels are added by
extending the table, not editing dispatch code.

### Adding a new filter

**Current**: Implement `Filter` trait → add to `omnizip-filters` →
wire into codec via `FilterChain`.

**Assessment**: OCP. Filters compose without modifying codec code.

### Adding a new checksum

**Current**: Add function to `omnizip-codecs::checksum`.

**Assessment**: OCP. No existing code changes when adding a new
checksum algorithm.

## Code quality metrics

| Metric                 | Target | Current |
|------------------------|--------|---------|
| `#![forbid(unsafe_code)]` coverage | 100% | 100% |
| External C deps        | 0      | 0       |
| Workspace warnings     | 0      | 0       |
| Clippy pedantic        | warn   | warn    |
| Test count             | 1000+  | ~900    |
| Determinism (BLAKE3)   | 100%   | 100%    |

## Semantic naming audit

All types are named after their domain concepts:

| Type             | Domain concept                    |
|------------------|-----------------------------------|
| `Codec`          | A compression algorithm           |
| `Filter`         | A preprocessing transform         |
| `CodecId`        | Unique identifier for a codec     |
| `CompressionLevel` | Quality/effort setting          |
| `MatchFinder`    | LZ77 match search engine          |
| `RangeEncoder`   | LZMA's arithmetic coder           |
| `FSEEncoder`     | ZSTD's finite-state entropy coder |
| `HuffmanTree`    | Canonical Huffman code table      |

No abbreviations, no implementation-detail names in public API.

## Acceptance criteria

- [x] Layer separation documented
- [x] Responsibility table complete
- [x] OCP assessment for each extension axis
- [x] Semantic naming audit
- [ ] Layer 2 → Layer 1 DRY violations resolved (TODO 179)
- [ ] Strategy-table dispatch in all codecs (TODO 179)
