# 205 — Brotli Context Mode Selection

- **Priority:** P3 (2% ratio win for text, well-defined)
- **Crate:** `omnizip-brotli`
- **Depends on:** [200](200-brotli-context-modeling.md) (context modes
  are part of the context-modeling framework)
- **Estimated effort:** 2 days

## Goal

Select the best CONTEXT_MODE (LSB6, MSB6, UTF8, Signed) per metablock
based on input characteristics. Currently hardcoded to LSB6.

## Background

RFC 7932 §10.1 defines 4 context modes:

| Mode | Context function | Best for |
|------|-----------------|----------|
| LSB6 | `p1 & 0x3F` | Generic binary |
| MSB6 | `p1 >> 2` | Byte-aligned data |
| UTF8 | UTF-8-aware context | UTF-8 text |
| Signed | 2-byte signed context | Signed integer data |

The reference encoder uses UTF8 for text inputs (detected via UTF-8
validation), LSB6 for binary.

## Scope

1. **UTF-8 detection** (1 day): fast scan to check if input is valid
   UTF-8. If yes, use UTF8 mode.

2. **Mode selection** (1 day): heuristic based on content type.

## Acceptance criteria

- [ ] UTF8 mode used for valid UTF-8 text at quality ≥ 4
- [ ] LSB6 mode used for binary input
- [ ] Round-trip correctness preserved
- [ ] Ratio improvement ≥ 1% on UTF-8 text vs LSB6

## Implementation plan

### New function: `choose_context_mode`

```rust
fn choose_context_mode(input: &[u8]) -> ContextMode {
    // Fast UTF-8 check: scan for invalid sequences
    if is_likely_utf8(input) {
        ContextMode::Utf8
    } else {
        ContextMode::Lsb6
    }
}
```

### UTF-8 detection

A simplified check: if the input contains no bytes in 0x80..0xC0 that
aren't valid UTF-8 continuation bytes, it's likely UTF-8 text.

### Integration with context modeling (TODO 200)

The context mode is written in the metablock header and read by
the context computation function.

## Test plan

- Unit test: UTF-8 detection on ASCII, UTF-8 text, binary
- Unit test: each context mode produces expected context IDs
- Integration: text input uses UTF8 mode at quality ≥ 4
- Integration: `brotli -d` accepts output

## References

- RFC 7932 §10.1
- Upstream: `brotli/c/enc/encode.c:BrotliChooseContextMode`
