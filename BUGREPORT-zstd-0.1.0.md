# omnizip-zstd 0.1.0 Bug Report

**Date:** 2026-08-01
**Source:** Published crate `omnizip-zstd 0.1.0` on crates.io
**Validated by:** Integration into LimniFS (`limnifs/limnifs`), 495-test workspace
**Reference:** `facebook/zstd` at `~/src/external/zstd/` (C reference implementation)

## Summary

Three bugs in the ZSTD literals section decoder prevent the crate from
handling Compressed and Treeless literals blocks. This makes it unable
to decode the vast majority of real-world ZSTD frames, since the
reference `zstd` encoder almost always emits Huffman-compressed literals
for any non-trivial input.

The differential parity tests pass because the golden fixtures in
`tests/fixtures/zstd/` happen to use only Raw and RLE blocks (small,
simple inputs where the encoder decides compression isn't worthwhile).
Real-world frames produced by `zstd -1` or `ruzstd::encoding` use
Compressed blocks with Huffman-coded literals, which hit the stub.

---

## BUG 1: Literals block_type reads bits 6-7 instead of bits 0-1

**Severity:** CRITICAL — misidentifies block type for most frames
**Location:** `src/literals/mod.rs` line 75

### Current (wrong)

```rust
let block_type = (header0 >> 6) & 0x03;
```

### C reference (correct)

```c
// zstd_decompress_block.c
SymbolEncodingType_e const litEncType = (symbolEncodingType_e)(istart[0] & 3);
```

### Fix

```rust
let block_type = header0 & 0x03;
```

### Impact

For byte `0x02` (bits 0-1 = 0b10 = Compressed):
- Current code: `(0x02 >> 6) & 3 = 0` → Raw (WRONG)
- C reference: `0x02 & 3 = 2` → Compressed (CORRECT)

The decoder treats Compressed blocks as Raw, producing garbage output.
This is why `zstd_round_trips` and `zstd_compresses_binary_data` fail
when omnizip-zstd is used as the decoder: `ruzstd` encodes with
Compressed literals, but omnizip-zstd sees them as Raw.

The golden fixtures pass by coincidence: their literals headers have
byte values where bits 0-1 happen to equal bits 6-7 (e.g. `0x10`:
both give 0 = Raw).

---

## BUG 2: Raw/RLE size_format uses bit 0 instead of bits 3-2

**Severity:** HIGH — wrong literal sizes for many Raw/RLE blocks
**Location:** `src/literals/mod.rs` lines 101-113

### Current (wrong)

```rust
fn decode_size_format_raw_rle(header0: u8, input: &[u8]) -> Result<(u32, usize), ZstdError> {
    if header0 & 1 == 0 {                              // bit 0
        Ok((u32::from(header0 >> 3), 1))               // size = bits 7-3
    } else {
        let lhc = u16::from_le_bytes([input[0], input[1]]);
        Ok((u32::from((lhc >> 4) & 0x0FFF), 2))        // size = bits 15-4
    }
}
```

### C reference (correct)

```c
// zstd_decompress_block.c, case set_basic (Raw):
U32 const lhlCode = ((istart[0]) >> 2) & 3;
switch(lhlCode)
{
case 0: case 2: default:
    lhSize = 1;
    litSize = istart[0] >> 3;
    break;
case 1:
    lhSize = 2;
    litSize = MEM_readLE16(istart) >> 4;
    break;
case 3:
    lhSize = 3;
    litSize = MEM_readLE24(istart) >> 4;
    break;
}
```

### Fix

```rust
fn decode_size_format_raw_rle(header0: u8, input: &[u8]) -> Result<(u32, usize), ZstdError> {
    let lhl_code = (header0 >> 2) & 3;
    match lhl_code {
        0 | 2 => Ok((u32::from(header0 >> 3), 1)),
        1 => {
            if input.len() < 2 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 2-byte Raw/RLE literals header".into(),
                });
            }
            let lhc = u16::from_le_bytes([input[0], input[1]]);
            Ok((u32::from(lhc >> 4), 2))
        }
        3 => {
            if input.len() < 3 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated 3-byte Raw/RLE literals header".into(),
                });
            }
            let lhc = u32::from_le_bytes([input[0], input[1], input[2], 0]);
            Ok((u32::from(lhc >> 4), 3))
        }
        _ => unreachable!("lhl_code is masked to 2 bits"),
    }
}
```

### Impact

The current code uses bit 0 (`header0 & 1`) to decide 1-byte vs 2-byte
header. The C reference uses bits 3-2 (`(header0 >> 2) & 3`) as a
2-bit `lhlCode` with 4 cases (0/2 → 1-byte, 1 → 2-byte, 3 → 3-byte).

For byte `0x09` (bit 0 = 1, lhlCode = (0x09 >> 2) & 3 = 2):
- Current: 2-byte header, litSize = (0x0009 >> 4) & 0xFFF = 0 (WRONG)
- C ref: 1-byte header, litSize = 0x09 >> 3 = 1 (CORRECT)

---

## BUG 3: Compressed/Treeless literals not implemented

**Severity:** CRITICAL — blocks all real-world ZSTD decode
**Location:** `src/literals/mod.rs` lines 158-167

### Current (stub)

```rust
fn decode_compressed<'t>(
    _input: &'t [u8],
    _previous_table: Option<&HuffmanTable>,
) -> Result<LiteralsSection<'t>, ZstdError> {
    // TODO: full implementation requires the Huffman-table reader
    // (FSE-compressed weights path). Tracked separately.
    Err(ZstdError::Unsupported {
        reason: "compressed / treeless literals not yet ported".into(),
    })
}
```

### C reference

```c
// zstd_decompress_block.c, case set_compressed:
U32 const lhlCode = (istart[0] >> 2) & 3;
U32 const lhc = MEM_readLE32(istart);
switch(lhlCode) {
case 0: case 1:
    singleStream = !lhlCode;
    lhSize = 3;
    litSize  = (lhc >> 4) & 0x3FF;       // 10 bits
    litCSize = (lhc >> 14) & 0x3FF;      // 10 bits
    break;
case 2:
    lhSize = 4;
    litSize  = (lhc >> 4) & 0x3FFF;      // 14 bits
    litCSize = lhc >> 18;                // 14 bits
    break;
case 3:
    lhSize = 5;
    litSize  = (lhc >> 4) & 0x3FFFF;     // 18 bits
    litCSize = lhc >> 22;               // 18 bits
    break;
}
// Then: read Huffman table from istart[lhSize..lhSize+litCSize]
// Decode using HUF_decompress1X_usingDTable (single stream)
//   or HUF_decompress4X_usingDTable (4 streams)
```

### What needs to be implemented

1. **Header parsing:** Read `lhlCode = (byte0 >> 2) & 3`, extract
   `litSize` and `litCSize` per the switch table above.
2. **Huffman table reader:** If the block is `set_compressed` (not
   `set_repeat`), read the Huffman tree from the compressed data. The
   tree is encoded as FSE-compressed weights (see RFC 8878 §4.2.1).
3. **Huffman decode:** Use the table to decompress `litCSize` bytes of
   Huffman-coded data into `litSize` bytes of literals. For
   `singleStream=1` (lhlCode 0): one forward bitstream. For
   `singleStream=0` (lhlCode 1,2,3): four parallel forward bitstreams.
4. **Table reuse:** Store the Huffman table for the next `Treeless`
   block in the same frame.

### Impact

Any ZSTD frame with Compressed or Treeless literals returns
`Unsupported`. The `omnizip_zstd::ZstdError::Unsupported` error fires
for:
- All frames produced by `zstd -1` through `zstd -22` on non-trivial input
- All frames produced by `ruzstd::encoding::compress_to_vec`
- Every golden fixture that uses Compressed literals (none currently
  in the test suite, which is why parity tests pass)

---

## Reproduction

```bash
# Clone limnifs and switch ZSTD decode to omnizip-zstd:
cd limnifs/limnifs
# Edit limnifs-core/src/codec/zstd.rs: replace ruzstd decode with:
#   omnizip_zstd::decompress(compressed, expected_len)
cargo test -p limnifs-core --lib codec::tests::zstd_round_trips
# → FAILED: "unsupported: compressed / treeless literals not yet ported"
```

---

## What works (validated)

These paths are correct in the published 0.1.0:

- Frame header parsing (magic, descriptor, FCS, window, dictionary)
- Block header parsing (type, size, last-block flag)
- Raw and RLE block decode
- FSE bitstream reader (reverse direction, LSB-first)
- FSE table construction (predefined + RLE modes)
- Sequence section decode (LL, OF, ML symbol + extra bits)
- Sequence execution (literal copy + match copy + repeat offset rotation)
- Offset code tables (OF_BASE / OF_BITS — fixed during validation)
- FSE decode ordering (extra bits before state transitions — fixed during validation)
- All golden fixtures from `facebook/zstd/tests/golden-decompression/`


---

## Resolution (2026-08-22, omnizip-zstd post-0.16.77)

- **BUG 1 (block_type bits)** and **BUG 2 (size_format)**: fixed earlier —
  the current code reads `header0 & 0x03` and uses the `lhlCode` switch
  exactly as specified above.
- **BUG 3 (compressed literals)**: fully implemented since (FSE-compressed
  weights + single/four-stream Huffman decode).
- **Follow-on bug found while validating this report's repro** (the
  checksum-mismatch symptom it predicted): the offset-code tables
  `OF_BASE`/`OF_BITS` did not match the C reference
  (`zstd_decompress_internal.h`). The wrong table made every
  FSE-compressed offset decode as a repeat code — a `zstd -1` CLI frame of
  a 1 MB CSV decoded to 962 KB of garbage (94% of bytes wrong) while our
  own round-trips kept passing (both sides shared the wrong table).
  Fixed with the reference values (`OF_BASE = [0,1,1,5,13,29,...]`,
  `OF_BITS = [0,1,2,...,31]`); verified against `zstd -1` CLI frames with
  and without content checksum, and covered by the embedded-frame
  regression test `decodes_reference_cli_frames`.
