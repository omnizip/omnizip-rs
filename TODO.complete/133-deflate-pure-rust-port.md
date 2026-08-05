# TODO 133: DEFLATE pure-Rust port (replace `miniz_oxide` wrapper)

## Problem

`omnizip-deflate` (318 LOC) wraps `miniz_oxide` entirely.
`omnizip-libdeflate` is a separate codec with its own in-house
encoder (stored + fixed + dynamic Huffman) but the `deflate` crate
remains a wrapper.

## Proposed fix

Two options:

1. **Delete `omnizip-deflate`** entirely. `omnizip-libdeflate`
   covers the same wire format (RFC 1951) with a pure-Rust encoder
   + decoder. Codecs that wrap `DeflateCodec` (brotli's deflate
   fallback, etc.) migrate to `LibdeflateCodec`.

2. **Reimplement `omnizip-deflate`** from spec, sharing code with
   `omnizip-libdeflate`. The two crates would expose different codec
   IDs but share implementation modules.

Option 1 is simpler — fewer crates, less duplication.

## Acceptance criteria

- [ ] Decision made and documented.
- [ ] If option 1: `omnizip-deflate` deleted, callers migrated.
- [ ] If option 2: pure-Rust encoder + decoder in `omnizip-deflate`,
  no `miniz_oxide` dependency.
- [ ] All codec tests pass.

## Priority

P1 — eliminates a long-standing external dependency.
