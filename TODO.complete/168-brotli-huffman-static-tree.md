# TODO 168: Brotli Huffman-coded encoder — static tree path

## Problem

PR #119 landed Phase D of the Brotli pure-Rust port: a working
uncompressed-metablock encoder + decoder. The encoder produces
**zero compression** — output is `input.len() + ~5 bytes overhead`.

To deliver actual compression (the user's stated goal: "fully finish
all remaining work, including brotli implementation"), we need
Huffman-coded metablocks. The full path (context modes + block-type
splitting + dictionary transforms) is multi-week work, but there's
a tractable **static-tree shortcut** that delivers ~60-80% of the
upstream `brotli -q 0` compression ratio with ~500 LOC.

## Scope

### Phase C.3a — Static-tree encoder (this TODO)

Port `store_meta_block_fast` from the upstream brotli crate
(`~/src/external/brotli/src/enc/brotli_bit_stream.rs:2614`). The
key insight: brotli has predefined ("static") Huffman tables for
the insert-copy and distance code alphabets. They're emitted as a
fixed bit pattern (no tree encoding overhead).

- **Literals**: Huffman-coded with a custom tree built from the
  input's byte histogram. Encoding via `BrotliStoreHuffmanTree`
  (complex form per RFC 7932 §9.5.2).
- **Commands**: Static Huffman tree (56+3 = 59 bits fixed pattern
  from `StoreStaticCommandHuffmanTree`).
- **Distances**: Static Huffman tree (28 bits fixed pattern from
  `StoreStaticDistanceHuffmanTree`).

This path requires LZ77 matches because every brotli insert-copy
command has a minimum `copy_len = 2`. For inputs shorter than 2
bytes or with no repeating patterns, fall back to the existing
uncompressed path.

### Phase C.3b — Full Huffman encoder (separate TODO)

Per-category Huffman trees, context modes, block-type splitting,
dictionary transforms. Compare against `brotli -q 11` for ratio.

## Implementation plan

### Files to add/modify

```
omnizip-brotli/src/
├── encoder.rs           (modify: add `encode_huffman` entry point)
├── huffman_tree.rs      (NEW: BrotliStoreHuffmanTree port)
├── static_codes.rs      (NEW: kStaticCommandCodeDepth/Bits + kStaticDistanceCodeDepth/Bits)
├── commands.rs          (NEW: insert-copy command generation from LZ77 matches)
└── encoder_error.rs     (modify: add new error variants)
```

### Step 1: Port static command + distance code tables

From `~/src/external/brotli/src/enc/backward_references/hq.rs:35`:
```rust
pub static kStaticCommandCodeDepth: [u8; 64] = [
    4, 4, 5, 6, 4, 4, 4, 5, 6, 6, 6, 6, 6, 6, 6, 7,
    // ... 64 entries
];
pub static kStaticCommandCodeBits: [u16; 64] = [
    0, 0, 8, 9, 3, 35, 7, 71, 39, 103, 23, 47, 175, 111, 239, 31,
    // ...
];
```

These are the canonical Huffman codes for the 64 most-common
insert-copy commands. The encoder restricts itself to these
commands when using static trees.

### Step 2: Port `BrotliStoreHuffmanTree`

From `~/src/external/brotli/src/enc/brotli_bit_stream.rs:835`. The
function:

1. Calls `BrotliWriteHuffmanTree` to RLE the per-symbol depths.
2. Builds a histogram of the RLE symbols (18-symbol alphabet).
3. Builds a Huffman tree for the RLE alphabet (5-bit max).
4. Emits the RLE Huffman tree structure
   (`BrotliStoreHuffmanTreeOfHuffmanTreeToBitMask`).
5. Emits the RLE'd depths using the RLE Huffman codes
   (`BrotliStoreHuffmanTreeToBitMask`).

The RLE alphabet (RFC 7932 §9.5.2):
- Symbols 0-15: literal code length
- Symbol 16: repeat previous length 2-6 times (2 extra bits)
- Symbol 17: zero-run 3-10 (3 extra bits)
- Symbol 18: zero-run 11-138 (7 extra bits)

### Step 3: Generate insert-copy commands from LZ77 matches

Use `omnizip_codecs::matchfinder::HashChainMatchFinder` to find
matches. Convert matches + literals into commands:

```rust
struct Command {
    insert_len: u32,   // literals before this command
    copy_len: u32,    // minimum 2
    distance: u32,    // 1-based
}
```

The command alphabet (RFC 7932 §10.3) encodes (insert_len, copy_len)
pairs into 704 symbols. With static trees, only the first 64 are
usable. The lookup table `kCmdLut` (from
`~/src/external/brotli-decompressor/src/prefix.rs:124`) maps each
symbol to its `(insert_len_offset, copy_len_offset, extra_bits)`
values.

### Step 4: Emit the metablock

Bit layout after the standard metablock header (with IS_UNCOMPRESSED=0):

```
13 zero bits (block-type + distance header prelude)
HSKIP = 0 (2 bits)  -- complex Huffman table for literals
[code-length code lengths] (variable, RLE-form)
[per-symbol code lengths] (variable, RLE-form)
Static command Huffman tree (59 bits)
Static distance Huffman tree (28 bits)
[Huffman-coded literals + commands + distances]
```

The 13 zero bits cover (per `BrotliWriteBits(13, 0, ...)` in
upstream `store_meta_block_fast`):
- NBLTYPESL=1 (2 bits: 00)
- ContextMode=0=Lsb6 (2 bits: 00)
- NBLTYPESI=1 (2 bits: 00)
- NBLTYPESD=1 (2 bits: 00)
- NPOSTFIX=0 (2 bits: 00)
- NDIRECT high bit (1 bit: 0) + low nibble (4 bits: 0000) — wait that's 15 bits total, not 13.

**Open question**: re-verify the 13-bit count by tracing what the
decoder actually reads. The discrepancy may be because some bits
are combined or skipped in the trivial path.

### Step 5: Wire into `encode_huffman`

```rust
pub fn encode_huffman(input: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if input.len() < 2 {
        // Too small for matches; fall back to uncompressed.
        return encode_uncompressed(input);
    }
    let commands = build_commands(input);
    if commands.iter().all(|c| c.copy_len == 0) {
        // No matches found; fall back.
        return encode_uncompressed(input);
    }
    // ... Huffman-coded path
}
```

`BrotliCodec::compress` picks `encode_huffman` for inputs that
benefit; `encode_uncompressed` for tiny inputs.

## Acceptance criteria

- [ ] `encode_huffman` produces valid Brotli (decodes via `brotli -d`)
- [ ] Output is smaller than `encode_uncompressed` for text inputs
      ≥ 100 bytes
- [ ] Deterministic: same input always produces same output bytes
- [ ] Round-trips via our in-house decoder (extend `decode()` to
      handle Huffman-coded metablocks — currently returns error)
- [ ] Throughput ≥ 50 MB/s on text inputs (no LZ77 chain walking)

## Priority

P0 — direct follow-up to TODO 117 Phase D. Restores the actual
compression capability that the uncompressed stub removed.

## Porting notes

The upstream `store_meta_block_fast` is the right reference. Key
files in `~/src/external/brotli/src/enc/`:

- `brotli_bit_stream.rs:2349` — `store_meta_block_trivial`
  (single-tree, no static fallback)
- `brotli_bit_stream.rs:2614` — `store_meta_block_fast`
  (custom literal tree + static command/distance trees) ← port this
- `brotli_bit_stream.rs:835` — `BrotliStoreHuffmanTree`
- `brotli_bit_stream.rs:925` — `BrotliBuildAndStoreHuffmanTreeFast`
- `entropy_encode.rs:575` — Huffman tree builder (`BrotliCreateHuffmanTree`)

The upstream code is BSD-3-Clause licensed (compatible with our
MIT OR Apache-2.0). Preserve attribution in source headers per
`LICENSE-NOTICE.md`.

## Test fixtures

Validate against these inputs (cross-check via `brotli -d`):
- `"hello world hello world hello world"` — short repetition
- `"a".repeat(1024)` — single-character run
- Lorem ipsum 10KB — natural text
- Calgary corpus `bib` — academic text
- Silesia corpus `xml` — structured text

Compare output sizes against `brotli -q 0` and `brotli -q 6`. Our
static-tree encoder should produce output within 30% of `brotli -q 0`
on text inputs.

## Risks

- **Bit layout correctness**: brotli's bit-packing is intricate.
  Use `brotli -d` round-trip as the source of truth.
- **Decoder gap**: our in-house decoder doesn't yet handle
  Huffman-coded metablocks. Either extend it or rely on `brotli -d`
  for validation.
- **Command alphabet restriction**: with static trees, only 64 of
  704 command symbols are usable. Long insert lengths (>65kB)
  require the complex-form command tree.

## Dependency

This unblocks the broader Brotli parity story (TODO 117, 151) and
makes omnizip-brotli competitive with the upstream crate for the
first time since the dependency was introduced.