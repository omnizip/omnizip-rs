# TODO 151: Brotli Phase C — encoder skeleton + dictionary

## Problem

Phases A and B of the Brotli pure-Rust port (TODOs 117, 130) cover
the decoder side through Huffman + context modes. Phase C is the
encoder side, which is the longest stretch.

## Scope

Phase C is broken into three sub-phases:

### Phase C.1 — Decoder completion (1-2 weeks)

Finish the in-house decoder:
- Block-type jump table execution (RFC 7932 §9.3).
- Distance code computation (direct + complex).
- Literal context-mode lookup (Lsb6 / Msb6 / Utf8 / Signed).
- Insert-and-copy command execution.
- Static dictionary lookup with 121 transforms (§10).

Acceptance:
- Decoder round-trips every fixture in the Brotli reference test
  suite.
- `BrotliCodec::decompress` switches from upstream `brotli` crate
  to the in-house decoder.

### Phase C.2 — Stored-block encoder (1 week)

Mirror `omnizip-libdeflate` Phase 1:
- Emit uncompressed metablocks (UNCOMPRESSED block type).
- Frame header + ISLAST + MLEN + reserved bit.
- Round-trip via own decoder + `brotli -d`.

### Phase C.3 — Full encoder (4-6 weeks)

Mirror `omnizip-lzma` / `omnizip-zstd` structure:
- LZ77 with hash-chain match finder (use shared
  `HashChainMatchFinder`).
- Static-dictionary lookup with transforms.
- Huffman table builder (canonical codes via package-merge).
- Context-mode literal encoding.
- Quality levels 0-11 mapped to `CompressionLevel`.

## Acceptance criteria (overall)

- [ ] C.1: decoder round-trips reference fixtures.
- [ ] C.2: stored-block encoder round-trips.
- [ ] C.3: full encoder at quality 11 within 5% of `brotli -q 11`
  on Enwik8.
- [ ] No `brotli` crate dependency in `Cargo.toml`.
- [ ] `#![forbid(unsafe_code)]` preserved.

## Priority

P0 — workspace convention: every codec pure-Rust from spec. Brotli
is the last remaining wrapper.
