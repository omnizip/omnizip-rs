# TODO 143: Error type unification

## Problem

Each codec has its own error enum (`LzmaError`, `ZstdError`,
`FlacError`, etc.). The shared `OmnizipError` is the only thing
callers see, but inside each codec the conversion is ad-hoc:

```rust
// LZMA
map_decode_error(e: LzmaError) -> OmnizipError { ... }

// ZSTD
match e {
    ZstdError::Unsupported { reason } => ...,
    other => ...,
}
```

## Proposed fix

Two options:

1. **Keep per-codec errors**, share a `From<CodecError> for OmnizipError`
   blanket impl. Each codec keeps its strongly-typed error enum.

2. **Unified codec error**: replace per-codec enums with a single
   `CodecError` enum in `omnizip-codecs`. Codecs use it directly.

Option 1 keeps type safety at codec boundaries. Option 2 simplifies
the trait at the cost of weaker per-codec typing. **Recommend option
1.**

## Acceptance criteria

- [ ] `From<*Error> for OmnizipError` blanket impl lands in
  `omnizip-codecs`.
- [ ] All codecs migrate their `map_*_error` helpers to use `From`.
- [ ] No behaviour change for callers.

## Priority

P2 — pure ergonomics.
