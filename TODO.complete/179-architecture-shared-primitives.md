# 179: Architecture — Shared Primitives Consolidation

## Priority: P2 (DRY + maintainability)

## Status: documented — primitives exist but are under-adopted.

## Context

The workspace has 19 codec crates. Shared primitives live in
`omnizip-codecs/`. The following shared modules exist:

| Module                        | LOC  | Adopted by                        |
|-------------------------------|------|-----------------------------------|
| `checksum.rs` (CRC32, etc.)   | 200  | LZMA, BZip2 (via re-export)       |
| `xxhash.rs`                   | 250  | ZSTD                              |
| `matchfinder.rs`              | 384  | **NONE** (doc says shared, 0 use) |
| `arith.rs`                    | 150  | PPMd (partial)                    |
| `hash.rs`                     | 80   | ZSTD (partial)                    |

## DRY violations

### 1. Match finder duplication (highest impact)

Three match finders exist:

| Location                        | LOC  | API                        |
|---------------------------------|------|----------------------------|
| `omnizip-codecs/matchfinder.rs` | 384  | `HashChainMatchFinder`     |
| `omnizip-lzma/encoder/match_finder.rs` | 476  | `MatchFinder`       |
| `omnizip-zstd/encoder/match_finder.rs` | 1230 | `MatchState` + `SeqStore` |

The LZMA and shared APIs are nearly identical. The ZSTD API is more
specialized (ZSTD sequences with rep offsets).

**Fix**:
1. LZMA: replace `MatchFinder` with `HashChainMatchFinder` from
   `omnizip-codecs`. Add a thin adapter if needed.
2. ZSTD: extract the hash-chain core into a shared adapter, keep the
   ZSTD-specific sequence store in the ZSTD crate.
3. LZ4 HC and libdeflate: also migrate to the shared finder.

**Expected DRY gain**: ~800 LOC removed across the workspace.

### 2. Bit reader/writer duplication

Every codec has its own bit reader/writer:

| Codec   | BitReader location                    |
|---------|---------------------------------------|
| LZMA    | `range_coder/encoder.rs`, `decoder.rs`|
| ZSTD    | `fse/bitstream.rs`                    |
| Brotli  | `decoder.rs` (inline)                 |
| BZip2   | `bz2/bitwriter.rs`                    |
| FLAC    | `bitreader.rs`, `encoder/bitwriter.rs`|

**Fix**: Create `omnizip-codecs::bitstream` with:
- `BitReaderBE` (MSB-first, for FLAC/Brotli)
- `BitReaderLE` (LSB-first, for ZSTD/FSE)
- `BitWriterBE`, `BitWriterLE`

Each codec adapts to the shared reader/writer via newtype wrappers if
needed (e.g., LZMA's range coder has special normalization logic).

**Expected DRY gain**: ~400 LOC removed.

### 3. Huffman encoder/decoder duplication

Huffman coding is reimplemented in ZSTD, Brotli, BZip2, DEFLATE:

| Codec   | Huffman location                     |
|---------|--------------------------------------|
| ZSTD    | `huffman/encoder.rs`, `decoder`      |
| Brotli  | `decoder.rs` (inline table)          |
| BZip2   | `bz2/huffman.rs`                     |
| DEFLATE | (wraps miniz_oxide)                  |

**Fix**: Create `omnizip-codecs::huffman` with:
- `HuffmanTree::build(symbol_counts) -> HuffmanTree`
- `HuffmanTree::encode(symbol, &mut BitWriter)`
- `HuffmanTree::decode(&mut BitReader) -> symbol`
- Length-limited variant (package-merge)

Each codec specifies its max code length and bit order.

**Expected DRY gain**: ~600 LOC removed.

## OCP improvements

### Data-driven codec registration

Currently, adding a codec requires:
1. Creating a crate
2. Implementing `Codec` trait
3. Adding to `Cargo.toml` workspace members
4. Adding to the codec list in each test/example

**Fix**: Use a registry pattern where codecs self-register via
inventory or linkme crates. Consumers only depend on the crates they
use; the registry auto-discovers.

### Level-based strategy dispatch

Each codec's `compress()` manually dispatches on level:
```rust
match quality {
    0..=1 => fast_encoder::vendored_compress(plaintext),
    _ => compress_fragment::compress(plaintext),
}
```

**Fix**: Replace with a strategy table:
```rust
const STRATEGIES: &[(CompressionLevel, Strategy)] = &[
    (1.into(), Strategy::Fast),
    (6.into(), Strategy::HashChain { chain: 128, nice: 128 }),
    (9.into(), Strategy::BinaryTree { chain: 1024, nice: 273 }),
];
```

This makes it easy to add new levels without changing dispatch code.

## Acceptance criteria

- [ ] LZMA matchfinder uses `omnizip-codecs::HashChainMatchFinder`
- [ ] Shared `BitReader`/`BitWriter` adopted by at least 3 codecs
- [ ] Shared Huffman module adopted by at least 2 codecs
- [ ] Strategy-table dispatch in LZMA, ZSTD, Brotli
- [ ] Zero DRY violations in `cargo deps` audit
