# TODO 159: Standard filter trait composition

## Problem

Filters (BCJ, Delta, etc.) live in `omnizip-filters` but the
`Filter` trait doesn't compose with `Codec`. Callers must apply a
filter manually, then call `compress`, then reverse the filter on
decode.

## Proposed fix

```rust
pub trait Filter {
    fn id(&self) -> FilterId;
    fn name(&self) -> &'static str;
    fn forward(&self, input: &[u8]) -> Result<Vec<u8>, FilterError>;
    fn reverse(&self, input: &[u8]) -> Result<Vec<u8>, FilterError>;
}

pub struct FilteredCodec<C: Codec, F: Filter> {
    codec: C,
    filter: F,
}

impl<C: Codec, F: Filter> Codec for FilteredCodec<C, F> {
    fn compress(&self, input: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let filtered = self.filter.forward(input)?;
        self.codec.compress(&filtered, level)
    }
    // ...
}
```

Each codec + filter combination composes without code duplication.

## Acceptance criteria

- [ ] `Filter` trait in `omnizip-codecs`.
- [ ] `FilteredCodec` adapter lands.
- [ ] LZMA + BCJ-x86 filter exposed as a single codec.
- [ ] Round-trip parity with XZ's `--x86` filter.

## Priority

P2 — pure ergonomics improvement.
