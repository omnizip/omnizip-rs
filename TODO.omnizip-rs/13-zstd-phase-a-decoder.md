# 13 — ZSTD Phase A: decoder + frame parse

- **Priority:** P0
- **Depends on:** [01](01-codec-trait-registry.md), [02](02-cross-language-differential-harness.md)
- **Estimated effort:** 1–2 weeks
- **Crate:** `omnizip-zstd`

## Goal

Port the ZSTD decoder: frame header parse, block decode (raw / RLE /
compressed), FSE decode, Huffman decode, sequence execution. Rust reads
every `.zst` fixture the Ruby decoder reads.

## Ruby → Rust module map (3,150 LOC across 14 Ruby files)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `zstandard/constants.rs` | `constants.rs` | 141 |
| `zstandard/frame/frame.rb` | `frame/mod.rs` | ~100 |
| `zstandard/frame/header.rb` | `frame/header.rs` | 220 |
| `zstandard/frame/block.rb` | `frame/block.rs` | 126 |
| `zstandard/fse/fse.rb` | `fse/mod.rs` | 34 |
| `zstandard/fse/bitstream.rb` | `fse/bitstream.rs` | 186 |
| `zstandard/fse/table.rs` | `fse/table.rs` | 266 |
| `zstandard/huffman.rb` | `huffman/mod.rs` | 269 |
| `zstandard/literals.rb` | `literals/mod.rs` | 174 |
| `zstandard/sequences.rb` | `sequences.rs` | 342 |
| `zstandard/decoder.rb` | `decoder.rs` | 225 |

## Phase A scope

1. **Constants & frame** (3 days): `constants.rs`, `frame/{mod,header,block}.rs`.
   Parse the ZSTD frame header descriptor, frame content size, dictionary id,
   single-segment flag, block headers.
2. **FSE** (4 days): `fse/{mod,bitstream,table}.rs`. Finite State Entropy
   is ZSTD's entropy coder. Port the bit-stream reader, the decoding table
   builder, and the state machine. Critical path.
3. **Huffman** (3 days): `huffman/mod.rs`. Decode Huffman-compressed literals.
4. **Literals & sequences** (3 days): `literals/mod.rs`, `sequences.rs`.
   Parse literal blocks (raw/RLE/compressed/treeless) and execute sequences
   (match copy with offsets).
5. **Top-level decoder** (2 days): `decoder.rs`. Wire frame → blocks →
   literals/sequences → output.

## Acceptance

- **Differential gate:** every `.zst` fixture in `tests/differential/fixtures/`
  decodes byte-identically between Ruby and Rust.
- **C reference gate:** every `.zst` produced by reference `zstd` at levels
  1–22 round-trips through our decoder.
- Decode throughput ≥ 100 MB/s on Apple M1 single core (Rust should be 2–3x
  faster than C `zstd -d` due to fewer abstraction layers — the Ruby is ~100
  KB/s, so Rust is ~1000x faster than Ruby).
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- ZSTD's FSE is the trickiest part. The Ruby port in `fse/` is already
  correct — translate it faithfully.
- The decoder must handle every frame version (v0.1 through v0.5+). The
  Ruby handles v0.4+; we match.
- Sequence execution uses repeated offsets (3 most-recent match distances).
  The state machine is small; port it as an explicit `match` over the
  rep-update codes.
