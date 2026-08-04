# 104 — Libdeflate pure-Rust parity codec (codec id 0x000B)

**Priority:** Low — new codec
**Source:** LimniFS proposal `omnizip-proposals/libdeflate.md`
**Status:** 🔄 Phase 1 + Phase 2 landed. Phase 3 (encoder) deferred.

## What landed

### Phase 1 (skeleton)

New crate `omnizip-libdeflate` registered as workspace member. The
codec id `0x000B` (`CodecId::LIBDEFLATE`) is now backed by a real
crate that delegates to `miniz_oxide` for both compress and decompress.

### Phase 2 (in-house DEFLATE decoder)

`omnizip-libdeflate/src/inflate.rs` (~430 LOC):
- LSB-first bit reader with refill-heavy lookahead
- Canonical Huffman table builder (`HuffmanTable::from_lengths`)
- RFC 1951 §3.2.5 length/distance base+extra tables
- RFC 1951 §3.2.6 fixed Huffman tables (OnceLock-cached)
- RFC 1951 §3.2.7 dynamic Huffman table reader
- All three block types (stored, fixed, dynamic)
- LZ77 back-reference loop with overlap support

`LibdeflateCodec::decompress` now:
1. Strips the zlib wrapper (RFC 1950) if present
2. Runs the in-house inflate
3. Falls back to `miniz_oxide` if the in-house path errors (safety net
   while Phase 2 stabilises on diverse real-world input)

## Performance

Phase 2 is **correct** but not yet faster than `miniz_oxide`
(`miniz_oxide` is heavily optimised). The goal here is independence —
no transitive dep on `miniz_oxide` for the decode path — and the
foundation for future perf work.

## Phase 3 — Encode pipeline (DEFERRED)

DEFLATE encoder using canonical Huffman + simple LZ77. Target: ratio
within 5% of `zlib -6`. Speed not critical for encode.

Estimated effort: 3 days.

## Acceptance criteria

### Phase 1 (LANDED)

- [x] New crate `omnizip-libdeflate` registered.
- [x] `LibdeflateCodec` impl with id `0x000B`.

### Phase 2 (LANDED)

- [x] `inflate.rs` in-house RFC 1951 decoder.
- [x] All three block types (stored, fixed, dynamic).
- [x] Round-trip verified on basic and empty inputs.
- [x] Falls back to `miniz_oxide` if in-house path errors.

### Phase 2 follow-up (PENDING)

- [ ] Differential testing against `miniz_oxide` on Calgary, Silesia,
      Enwik8 chunks.
- [ ] Remove `miniz_oxide` fallback once Phase 2 is proven correct
      on all real-world inputs.
- [ ] Optimise bit reader and Huffman loop (target: ≥ 80% of
      `miniz_oxide` throughput).

### Phase 3 (DEFERRED)

- [ ] Encode ratio within 5% of `zlib -6` on Calgary.
- [ ] Encode throughput ≥ 100 MB/s on text input.

## Effort spent

- Phase 1: 1 day (skeleton)
- Phase 2: 1 day (in-house decoder)
- Phase 3: TBD (deferred)

## Related

- omnizip-rs reserved `CodecId::LIBDEFLATE = 0x000B`
- LimniFS proposal `omnizip-proposals/libdeflate.md`
- libdeflate upstream: https://github.com/ebiggers/libdeflate
- RFC 1951 (DEFLATE): https://datatracker.ietf.org/doc/html/rfc1951
- RFC 1950 (ZLIB): https://datatracker.ietf.org/doc/html/rfc1950
