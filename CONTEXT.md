# CONTEXT.md — Domain Glossary

Domain vocabulary for omnizip-rs. Architecture reviews and new code
should use these names verbatim; the glossary is the single source of
truth for what a term means (per ADR-0010).

Codified decisions that constrain these concepts live in
[`docs/adr/`](docs/adr/); each entry below cross-references them.

---

## The two product layers

**Codec** — a compression algorithm that maps `plaintext -> compressed`
and back. One crate per algorithm family (ADR-0002), implemented in
pure safe Rust (ADR-0001), reached only through the `Codec` trait and
`CodecRegistry` (ADR-0003). Wire-format output must interop with the
reference tool (e.g. `bzip2 -d`, `lz4 -d`, `xz -d`).

**Container** — an archive format (zip, 7z, tar, …) that *packages*
members and delegates member payloads to codecs. Containers never
re-implement compression; they call codecs through the registry.
Container crates: `omnizip-zip`, `omnizip-sevenzip`, `omnizip-rar`
(RAR4 + RAR5), `omnizip-tar`, `omnizip-cpio`, `omnizip-xar`,
`omnizip-iso`, `omnizip-rpm`, `omnizip-par2`, `omnizip-ole`.
Single-member file wrappers (`gzip`, `bzip2_file`, `lzip`,
`lzma_alone`) live in `omnizip-archive-core`.

## Codec families and their wire formats

| Family | Crate | Wire format |
|---|---|---|
| Deflate | `omnizip-deflate`, `omnizip-deflate64`, `omnizip-libdeflate` | RFC 1951 stream; 64 KB window for deflate64 |
| Brotli | `omnizip-brotli` | RFC 7932 stream (from-spec encoder, ADR-0007) |
| BZip2 | `omnizip-bzip2` | `.bz2` (`BZh`): RLE1 → BWT → MTF → RLE2 → Huffman |
| LZMA | `omnizip-lzma` | LZMA1 / LZMA2 in `.xz`, `.lzma`, `.lz` |
| Zstandard | `omnizip-zstd` | Zstandard frame (block + FSE/Huffman) |
| LZ4 | `omnizip-lz4` | LZ4 block format + frame format (`lz4 -d` compatible) |
| Snappy | `omnizip-snappy` | Snappy raw + framed |
| PPMd | `omnizip-ppmd` | PPMd variant H/I |
| Filters | `omnizip-filters` | BCJ x86, delta (pre/post transforms, not codecs) |
| Research codecs | `omnizip-flac`, `omnizip-fsst`, `omnizip-blosc`, `omnizip-glza`, `omnizip-ricepp`, `omnizip-zpaq` | not in the Ruby omnizip; Rust-original |

## Core domain concepts

**Parity** — behavior equivalence against a *reference*: same decode
bytes for reference-produced files, and encoder output the reference
tool accepts. Verified by the differential harness (ADR-0005).
"Parity" never means byte-identical *encoder* output to the reference
— our encoders choose their own parse — except where a deliberate
line-by-line port makes it so (lz4 fast tier, bzip2 `sendMTFValues`).

**Ratio** — compressed size, ours ÷ reference CLI, at the same level.
Bar: never worse than ~1.02x on the sweep; cells above that get
root-caused, not tuned blind. See *sweep*.

**Sweep** — the 10-corpus broad sweep (`/tmp/sweep`: arial.ttf,
bin1, bin2, csv2m.bin, dbdump.txt, fits4m.bin, rand.bin, rfc.txt,
rustsrc.txt, words.txt) racing ours vs the reference CLI at matched
levels. The sweep is the ratio gate; single fixtures hide bugs that
"passing" suites miss.

**Tier** — a level band inside one codec family selecting a different
*parser*, not just parameters: e.g. LZMA fast parse (port of xz's
`lzma_encoder_optimum_fast`) at level 1 vs optimal parser at 2+;
LZ4 fast tier (`LZ4_compress_generic` port) vs HC tier (hash chain +
lazy). `capabilities()` and `default_{fast,balanced,max_ratio}_level`
define each codec's tiers.

**Match finder / hasher** — the engine locating backward references.
`HashChainMatchFinder` is shared in `omnizip-codecs` (ADR-0008);
codecs with reference-shaped loops keep private ones (lz4 hash5
table, brotli *banks*) when sharing would perturb parity.

**Bank** — Brotli's match-finder storage: `BankMatchFinder` in
`omnizip-codecs`, mirroring C brotli's H5/H6 hash banks — candidate
matches binned by length class into fixed-slot banks, newest-first.
The greedy tier (q4–9) runs a single shared bank; higher tiers split
banks per metablock. See ADR-0007.

**Parser** — the policy deciding *which* matches to emit: greedy,
lazy (one-byte lookahead, LZ4 HC / deflate), optimal-parse (dynamic
programming, LZMA levels 2+), or reference-shaped fast parse.

**Metablock / block / member** — the unit of re-sync in a stream:
brotli metablocks, zstd blocks, bzip2 blocks (RLE1-budgeted,
`nblockMAX`), xz blocks/LZMA2 chunks, gzip members. Block-splitting
policy is part of parity (bzip2's RLE1-output budget bug is the
cautionary tale).

**Interop gate** — a test piping our output through a reference CLI
(`bzip2 -dc`, `lz4 -d`) and asserting round-trip. Skips when the CLI
is absent; never a substitute for the in-house decoder.

**Determinism recording** — `tests/determinism/determinism_recorded.txt`
pins codec output hashes; drift fails CI. Determinism is a hard
requirement because LimniFS's `DropId = BLAKE3(plaintext)` breaks
dedup otherwise (ADR-0004).

**Differential harness** — the cross-language gate decoding Ruby +
Rust on the same fixtures and asserting byte-identical output
(ADR-0005; `tests/differential/`, Ruby ref pinned in
`ruby-ref.txt`).

**Release train** — patch-line workspace bumps (0.21.x) merged via
PR, then published by the trusted-publishing Release workflow;
rebase-merge only, never direct pushes (ADR-0006).

## Consumers and references

**LimniFS** — the downstream consumer; content-addressed FS whose
`DropId` is the BLAKE3 of plaintext. Its constraints (determinism,
streaming, memory budgets) are why the `Codec` trait looks the way it
does.

**Reference** — the authoritative implementation for a family:
Ruby omnizip (`../omnizip`) for algorithm structure, the C libraries
(`xz`, `zstd`, `lz4`, `brotli`, `bzip2`) for wire-format detail and
tuning. Ports verify against Ruby first, tune against C second.
