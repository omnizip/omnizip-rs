# 202 — Brotli Dictionary Transforms

- **Priority:** P2 (2% ratio win, well-defined)
- **Crate:** `omnizip-brotli`
- **Depends on:** none
- **Estimated effort:** 3 days

## Goal

Enable all 121 RFC 7932 §10.4 dictionary transforms in
`find_dictionary_match`. Currently only the **identity** transform
(transform_idx=0) is used, meaning dictionary words are matched
verbatim. The other 120 transforms (uppercase first/all, shifts,
cutting) catch many more real-world matches.

## Background

RFC 7932 §10.3–§10.4 defines 121 transforms over the static
dictionary:

| Transform | Effect | Example |
|-----------|--------|---------|
| 0 | Identity | "hello" → "hello" |
| 1 | FermentFirst (uppercase first letter) | "hello" → "Hello" |
| 2 | FermentAll (uppercase all) | "hello" → "HELLO" |
| 3–120 | Shifts, cuts, and combinations | "hello" → "ello", etc. |

The transform is selected by the high bits of the dictionary address:
```
address = distance - max_distance - 1
word_idx = address & ((1 << shift) - 1)
transform_idx = address >> shift
```

The decoder already applies all 121 transforms via
`transform_dictionary_word` in `dictionary.rs`.

## Scope

1. **Transform-aware matching** (2 days): in `find_dictionary_match`,
   for each dictionary word and each transform, apply the transform
   and compare with the input. Return the (distance, length) for the
   best match.

2. **Distance encoding** (1 day): update the distance computation to
   include `transform_idx << shift` in the address.

## Acceptance criteria

- [ ] At least 5 transforms used on typical text input
- [ ] Round-trip correctness preserved
- [ ] Ratio improvement ≥ 1% on mixed-case text
- [ ] No regression on binary input
- [ ] `brotli -d` accepts output

## Implementation plan

### Modified: `dictionary.rs:find_dictionary_match`

```rust
pub fn find_dictionary_match(input: &[u8], pos: usize, max_distance: u32) -> Option<(u32, u32)> {
    // For each word length 4..=8:
    for len in 4u32..=8u32 {
        let shift = SIZE_BITS_BY_LENGTH[len as usize];
        let num_words = 1usize << shift;
        let offset_base = OFFSETS_BY_LENGTH[len as usize];

        for word_idx in 0..num_words {
            let dict_offset = offset_base + word_idx * len as usize;
            let word = &DICTIONARY_DATA[dict_offset..dict_offset + len as usize];

            // Try each transform
            for transform_idx in 0..NUM_TRANSFORMS {
                let mut transformed = Vec::new();
                let n = transform_dictionary_word(&mut transformed, word, transform_idx);
                if n == 0 { continue; }

                if pos + n <= input.len() && &input[pos..pos + n] == &transformed[..n] {
                    let address = (transform_idx << shift) | word_idx;
                    let distance = max_distance + 1 + address as u32;
                    return Some((distance, n as u32));
                }
            }
        }
    }
    None
}
```

### Performance optimization

The naive O(122784 × 121) search per position is too slow. Use a hash
table of transformed dictionary words for O(1) lookup:

1. Pre-compute all transformed words at init time
2. Build a hash map: `HashMap<[u8; 4], Vec<(word_idx, transform_idx)>>`
3. Lookup by the first 4 bytes of the input at `pos`

## Test plan

- Unit test: each transform produces the expected output
- Unit test: "Hello" at position 0 matches via FermentFirst transform
- Integration: text with mixed-case words compresses better
- Integration: round-trip at all quality levels

## References

- RFC 7932 §10.3–§10.4
- `dictionary.rs:TRANSFORM_DATA` (121 transforms, already implemented)
- `dictionary.rs:transform_dictionary_word` (already implemented)
- Our decoder: `decoder_full.rs:dictionary_lookup` (already handles
  transforms)
