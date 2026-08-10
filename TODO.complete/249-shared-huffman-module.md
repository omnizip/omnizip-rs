# 249 — Shared Huffman Module Unification (Architectural: DRY)

- **Priority:** P2 (DRY — eliminates ~3,000 LOC of duplication)
- **Crate:** `omnizip-codecs/src/huffman.rs` (extend existing)
- **Depends on:** none
- **Estimated effort:** 4 days

## Problem

Four codecs (Brotli, LZMA, ZSTD, DEFLATE) each ship their own
Huffman code:

| Crate | File | LOC | Variant |
|---|---|---|---|
| omnizip-brotli | `huffman.rs` | ~400 | Canonical, max 15-bit |
| omnizip-lzma | `huffman.rs` | ~350 | Canonical, max 15-bit (overlaps) |
| omnizip-zstd | `huffman/encoder.rs` + `huffman/decoder.rs` | ~600 | Length-limited package-merge |
| omnizip-libdeflate | `huffman.rs` | ~450 | Dynamic Huffman (DEFLATE spec) |
| omnizip-codecs | `huffman.rs` | ~300 (existing shared) | Generic canonical |

Each has subtly different:
- Tree-building algorithm (package-merge vs. simple sort)
- Length-limiting constraints (15 bits vs. 11 vs. 9)
- Code-assignment order (radix sort vs. canonical)
- Decode strategy (table-based vs. bit-walk)
- Allocation patterns (reuse vs. fresh)

This is the largest DRY violation in the workspace. Bugs found in
one variant (e.g., the package-merge length-limit edge case) must
be fixed in each copy.

## Design

### Strategy: `HuffmanCodec` trait + `HuffmanParams`

Different codecs have different constraints, so we can't have ONE
`HuffmanTree`. Instead, factor out the COMMON operations behind a
trait + parameterization.

```rust
/// Per-codec Huffman configuration.
#[derive(Debug, Clone, Copy)]
pub struct HuffmanParams {
    /// Maximum allowed code length (RFC 7932: 15, RFC 1951: 15,
    /// ZSTD FSE: 11, etc.).
    pub max_code_length: u8,
    /// Whether to apply length-limiting (package-merge) when the
    /// naive tree exceeds `max_code_length`.
    pub length_limited: bool,
    /// Whether codes are canonical (RFC 1951, RFC 7932) or
    /// assigned by tree traversal (LZMA range coder).
    pub canonical: bool,
}

/// Tree-building strategies. Codecs pick one via `HuffmanParams`.
pub enum BuildStrategy {
    /// Sort by frequency, assign lengths greedily.
    Naive,
    /// Package-merge for length-limited optimal codes.
    PackageMerge,
    /// Use fixed code lengths (e.g., deflate's stored block).
    FixedLength(u8),
}

/// Build a canonical Huffman code from symbol frequencies.
///
/// Returns `Vec<(code: u16, length: u8)>` per symbol, or `None` if
/// the alphabet is empty.
pub fn build_canonical(
    freqs: &[u32],
    params: HuffmanParams,
    strategy: BuildStrategy,
) -> Option<Vec<(u16, u8)>>;
```

### Decoder trait

```rust
/// Per-codec Huffman decoder strategy.
pub trait HuffmanDecoder {
    /// Read one symbol from the bit stream.
    fn read_symbol(&mut self, br: &mut BitReader) -> Option<u16>;

    /// Build a decoder from canonical code lengths.
    fn from_lengths(lengths: &[u8]) -> Self;
}
```

Two implementations:
- `TableDecoder` — fast O(1) lookup, used by Brotli/DEFLATE.
- `BitWalkDecoder` — slower, used by LZMA's range coder (different
  bit reader interface).

### Migration plan

Per-codec, in priority order:

1. **omnizip-brotli/huffman.rs** → use shared `build_canonical` with
   `HuffmanParams { max_code_length: 15, length_limited: false,
   canonical: true }`.
2. **omnizip-lzma/huffman.rs** → same.
3. **omnizip-libdeflate/huffman.rs** → same.
4. **omnizip-zstd/huffman/** → use shared `build_canonical` with
   `length_limited: true, max_code_length: 11` + package-merge.

Each migration:
- Replace the per-codec tree builder with the shared one.
- Verify all tests pass with byte-identical output.
- Delete the per-codec tree builder.

## Acceptance criteria

- [ ] Shared `build_canonical` and `HuffmanDecoder` in
      `omnizip-codecs/src/huffman.rs`.
- [ ] Brotli migrated; per-codec huffman.rs removed.
- [ ] LZMA migrated.
- [ ] libdeflate migrated.
- [ ] ZSTD migrated (with package-merge strategy).
- [ ] All workspace tests pass byte-identical.
- [ ] LOC reduction: ~2,000+ lines removed across codecs.

## Why this matters

The Huffman code in each codec is functionally identical with
parameter-level differences. Centralizing it:
- Eliminates 4× bug-fix surface.
- Lets us add one optimization (e.g., SIMD decode) and have it
  benefit all codecs.
- Makes the per-codec code shorter and easier to read.
- Surfaces the actually interesting per-codec logic (which symbols,
  which encoding scheme) by removing the generic Huffman noise.
