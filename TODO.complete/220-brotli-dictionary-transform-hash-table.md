# 220 — Brotli Dictionary Transform Hash Table

- **Status:** DONE (all 121 RFC 7932 transforms; hash table in
  `encoder/dict_hash.rs`)
- **Priority:** P1 (biggest remaining ratio win for text data)
- **Crate:** `omnizip-brotli`
- **Depends on:** [202](202-brotli-dictionary-transforms.md) (identity-only), this extends it
- **Estimated effort:** 2 days

## Goal

Pre-compute a hash table of all dictionary words with ALL 121 RFC 7932
transforms, enabling O(1) per-position lookup. Currently, only identity
transform is supported (TODO 202 identity-only).

## Background

The C reference uses a hash table of the first 4 bytes of every
transformed dictionary word. At encode time, it hashes the input's 4
bytes and looks up only matching entries — O(1) per position.

The current implementation scans word lengths 4-8 with a first-byte
check, finding identity matches only. UPPERCASE_FIRST and
UPPERCASE_ALL transforms (which map "name" → "Name", "name" → "NAME")
are not supported.

## Design

```rust
pub struct DictHashTable {
    head: Vec<u32>,       // hash → first entry index
    next: Vec<u32>,       // chain within same hash
    entries: Vec<DictEntry>,
}

struct DictEntry {
    word_offset: u32,     // offset in DICTIONARY_DATA
    word_length: u8,      // original word length (4..=24)
    transform_idx: u8,    // index into TRANSFORM_DATA (0..121)
    copy_length: u8,      // transformed word length
}
```

### Build (at module init via `std::sync::OnceLock`)

1. For each word length 4..=24:
   - For each word at that length (word_idx = 0..num_words):
     - For each transform_idx in 0..121:
       - Apply `transform_dictionary_word` to get the transformed bytes
       - If transformed length >= 4:
         - Hash first 4 bytes
         - Insert (word_offset, word_length, transform_idx, copy_length)

### Lookup

1. Hash `input[pos..pos+4]`
2. Walk chain for this hash
3. For each entry: reconstruct the transformed word and compare
4. Return longest match as (distance, copy_length)
   where `distance = max_distance + 1 + (word_idx | (transform_idx << shift))`

## Acceptance criteria

- [ ] All 121 transforms supported
- [ ] O(1) average lookup per position
- [ ] Identity matches produce same distance codes as current code
- [ ] Transform matches round-trip through decoder
- [ ] Ratio improvement >= 2% on English text fixtures
- [ ] No speed regression (hash table build is one-time)
