# Spec Coverage — omnizip-zstd

**Spec:** RFC 8878 (Zstandard), https://datatracker.ietf.org/doc/html/rfc8878
**Last updated:** 2026-08-03

## Coverage matrix

| Section | Clause | Description | Test file | Status |
|---------|--------|-------------|-----------|--------|
| §3.1.1 | Magic number | 0xFD2FB528 (4 bytes LE) | zstd/src/constants.rs | ✅ |
| §3.1.1 | Frame header | Descriptor + frame content size + dict ID + window | zstd/src/frame.rs | ✅ |
| §3.1.2 | Block header | Last block + block type + block size | zstd/src/frame.rs | ✅ |
| §3.1.2 | Raw block (type 0) | Uncompressed data | zstd/src/decoder.rs | ✅ |
| §3.1.2 | RLE block (type 1) | Single byte repeated | zstd/src/decoder.rs | ✅ |
| §3.1.2 | Compressed block (type 2) | Literals + sequences | zstd/src/decoder.rs | ✅ |
| §3.1.3 | Frame checksum | XXH64 truncated to 32 bits | zstd/src/xxhash.rs | ✅ |
| §3.2 | Literals section | Raw/RLE/Compressed/Treeless | zstd/src/literals.rs | ✅ |
| §3.2 | Huffman decode | Direct weights + FSE-compressed weights | zstd/src/huffman.rs | ✅ |
| §3.2 | FSE decode | NCount table + bitstream | zstd/src/fse.rs | ✅ |
| §3.3 | Sequences section | Literal lengths + offsets + match lengths | zstd/src/sequences.rs | ✅ |
| §3.3 | Sequence execution | Copy literals + match from history | zstd/src/sequences.rs | ✅ |
| §4 | Dictionary format | Magic + dict ID + entropy tables + content | zstd/src/dict.rs | ✅ |
| — | Encoder | Frame/block/literals/sequences/FSE/Huffman | zstd/src/encoder/ | ✅ |
| — | Dictionary trainer | Frequency + FastCover algorithms | zstd/src/dict_trainer.rs | ✅ |

## Gaps

1. **Multi-byte FSE (TODO 84)**: The single-symbol-per-step FSE
   decoder could process 2-4 bytes per step via precomputed tables
   (ACM 2024 paper). Not implemented. **Priority: medium**.

2. **Window size enforcement**: The decoder doesn't strictly enforce
   the window size from the frame header. Could cause issues on
   memory-constrained devices. **Priority: low**.

3. **Skippable frames**: Frame magic 0x184D2A5n allows application-
   specific metadata. Not implemented. **Priority: very low**.
