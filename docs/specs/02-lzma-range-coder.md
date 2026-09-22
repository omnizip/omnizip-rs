# 02 — LZMA range coder

The range coder is the entropy coding core of LZMA. Every bit in the
compressed stream — literal bits, match/literal decisions, length/distance
values — is encoded through the range coder's probability-driven binary
arithmetic coding.

## State

The range coder maintains two pieces of state:

```rust
struct RangeDecoder {
    range: u32,   // starts at 0xFFFF_FFFF; narrows as bits are decoded
    code: u32,    // the current code value, filled from input bytes
}
```

```rust
struct RangeEncoder {
    range: u32,       // starts at 0xFFFF_FFFF
    low: u64,         // the accumulated output value
    cache: u8,        // carry-delay byte
    cache_size: u64,  // number of FF bytes pending carry resolution
}
```

## Constants

| Constant | Value | Meaning |
|---|---|---|
| `TOP` | `0x0100_0000` | Renormalisation threshold: when `range < TOP`, shift a byte |
| `BIT_MODEL_TOTAL` | `0x800` (2048) | Total probability range for each bit model (2^11) |
| `MOVE_BITS` | `5` | Adaptation speed: probability shifts by `total >> MOVE_BITS` on update |
| `MOVE_HALF` | `BIT_MODEL_TOTAL >> 1` | Initial probability (50%) |
| `NUM_BIT_MODEL_TOTAL_BITS` | `11` | `log2(BIT_MODEL_TOTAL)` |

## Decode algorithm

### Initialisation

Read the first byte (MUST be 0x00 for LZMA1). Then read 4 bytes
big-endian into `code`. Set `range = 0xFFFF_FFFF`.

### Decode one bit (given a probability `prob`)

```text
1. bound = range >> 11                         // range / BIT_MODEL_TOTAL
2. if code < bound + (range - bound) * prob:
     // This branch represents the modelled outcome
3.   range = bound + (range - bound) * prob     // narrow the range
4.   prob += (BIT_MODEL_TOTAL - prob) >> MOVE_BITS  // adapt UP
5.   bit = 1
   else:
3.   range -= bound + (range - bound) * prob     // narrow the other way
4.   code  -= bound + (range - bound) * prob
5.   prob -= prob >> MOVE_BITS                   // adapt DOWN
6.   bit = 0
7. if range < TOP:                               // renormalise
8.   range <<= 8
9.   code = (code << 8) | next_input_byte()
```

**Key insight:** the actual LZMA implementation uses a simpler formulation
that avoids the multiply. The `bound` is `range >> 11`, and the branch
compares `code` against `bound` scaled by the probability. This is
mathematically equivalent but faster.

### Simplified (actual) decode

```text
bound = (range >> NUM_BIT_MODEL_TOTAL_BITS) * prob
if code < bound:
    range = bound
    prob += (BIT_MODEL_TOTAL - prob) >> MOVE_BITS
    bit = 0
else:
    range -= bound
    code -= bound
    prob -= prob >> MOVE_BITS
    bit = 1
// renormalise
if range < TOP:
    range <<= 8
    code = (code << 8) | read_byte()
```

### Decode direct bits (no probability model)

Used for the raw bits of distances and lengths. No probability
adaptation — uniform distribution.

```text
for each direct bit:
    range >>= 1
    code -= range
    t = 0 - (code >> 31)            // 0xFFFF_FFFF if code went negative
    code += range & t
    bit = (t + 1)                   // 1 if negative, 0 if not
    if range < TOP:
        range <<= 8
        code = (code << 8) | read_byte()
```

### Decode a bit tree

Decode `num_bits` bits MSB-first using an array of `1 << num_bits`
probability slots. Used for length and distance slot coding.

```text
m = 1
for i in 0..num_bits:
    m = (m << 1) | decode_bit(&probs[m])
symbol = m - (1 << num_bits)     // the decoded value
```

### Reverse bit tree

Same as bit tree but bits are assembled LSB-first (reversed order). Used
for distance alignment and some length codes.

```text
m = 1
for i in 0..num_bits:
    bit = decode_bit(&probs[m])
    m = (m << 1) | bit
    symbol |= bit << i            // reversed: bit goes to position i
```

## Encode algorithm (mirror of decode)

The encoder mirrors the decoder exactly. Every `decode_bit(prob)` on the
decoder side has a corresponding `encode_bit(prob, bit)` on the encoder
side that produces the same bytes. This is the fundamental guarantee that
makes LZMA deterministic: same input + same parameters ⇒ same output.

### Carry handling

The encoder must handle carries: when `low` overflows past 32 bits, a
carry propagates back to previously written bytes. The `cache` and
`cache_size` fields delay byte emission until carry resolution is
certain.

## Determinism

The range coder is fully deterministic. There is no randomness, no
thread-dependent behaviour, no floating-point. Same input bytes + same
initial probabilities + same parameter set ⇒ byte-identical output,
always, on every machine.

This is a hard requirement for content-addressed storage: if two
encoders produce different bytes for the same input, `DropId =
BLAKE3(plaintext)` deduplication breaks at the representation layer.

## Cross-references

- Ruby: `omnizip/lib/omnizip/algorithms/lzma/range_decoder.rb` (274 LOC)
- Ruby: `omnizip/lib/omnizip/algorithms/lzma/range_encoder.rb` (202 LOC)
- C: `xz/src/liblzma/rangecoder/range_decoder.h`
- C: `xz/src/liblzma/rangecoder/range_encoder.h`
- Spec: `lzma-specification.txt` §2 (Igor Pavlov)
- Rust port: `omnizip-lzma/src/range_coder/decoder.rs` (pending)
