# 259 — Error Type Unification

- **Status:** PARTIAL — `OmnizipError` gets helper constructors
  (`encode_failed`, `decode_failed`, etc.) and `codec_id()` accessor.
  Per-codec structured sub-errors (BrotliError, LzmaError, ZstdError)
  still pending.
- **Priority:** P3 (DX: caller ergonomics)
- **Crate:** `omnizip-codecs` (OmnizipError)
- **Depends on:** none
- **Estimated effort:** 1 day

## Problem

`OmnizipError` is a single enum with variants for all codecs. Adding
a new codec means adding new variants. Callers can't easily match
on "errors from codec X" vs "errors from codec Y".

Current variants (partial list):
- LevelOutOfRange
- Unsupported
- EncodeFailed
- DecodeFailed
- LengthMismatch
- InvalidInput
- IoError
- ... many more

Per-codec-specific errors are crammed into `EncodeFailed(String)` /
`DecodeFailed(String)`, losing structure.

## Design

### Codec-specific error subtypes

```rust
#[derive(Debug, thiserror::Error)]
pub enum OmnizipError {
    #[error("compression level out of range: {level}")]
    LevelOutOfRange { level: u8 },

    #[error("codec unsupported: {0}")]
    Unsupported(&'static str),

    #[error(transparent)]
    Brotli(#[from] BrotliError),

    #[error(transparent)]
    Lzma(#[from] LzmaError),

    #[error(transparent)]
    Zstd(#[from] ZstdError),

    // ... per codec

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum BrotliError {
    #[error("invalid metablock header: {0}")]
    InvalidMetablockHeader(&'static str),

    #[error("invalid literal context: {0}")]
    InvalidLiteralContext(u8),

    #[error("invalid distance: {0}")]
    InvalidDistance(u32),

    #[error("dictionary lookup failed")]
    DictionaryLookupFailed,

    #[error("wire format: {0}")]
    WireFormat(String),
}
```

### Caller pattern

```rust
match codec.compress(data, level) {
    Ok(out) => { /* use out */ },
    Err(OmnizipError::LevelOutOfRange { level }) => { /* handle */ },
    Err(OmnizipError::Brotli(BrotliError::InvalidDistance(d))) => { /* handle */ },
    Err(e) => Err(e)?,
}
```

Codecs that don't have detailed error types still use the generic
`EncodeFailed` / `DecodeFailed`.

### Migration path

- Per-codec error enums start empty (just `Other(String)`).
- As bugs surface, replace `Other(String)` with structured variants.
- Old `EncodeFailed(String)` / `DecodeFailed(String)` remain for
  backward compat.

## Acceptance criteria

- [ ] `OmnizipError` delegates to per-codec error types via `#[from]`.
- [ ] Brotli, LZMA, ZSTD have structured error enums (≥5 variants each).
- [ ] Other codecs use generic `EncodeFailed` (no regression).
- [ ] All codecs' `compress`/`decompress` return `Result<_, OmnizipError>`.
- [ ] Examples in docs show structured error matching.

## Why this matters

Errors are part of the API. Today, callers do `if err.to_string().contains("invalid distance")` to handle a specific case. That's
brittle and slow. Structured errors make the API self-documenting.
