# TODO 149: Documentation — per-codec READMEs

## Problem

Most crates have only a `src/lib.rs` doc comment. No top-level
README explaining:

- What the codec does.
- What wire format it implements.
- What's supported / unsupported.
- How to use it (minimal example).
- Performance characteristics.

## Proposed fix

Each codec crate gets a `README.md` with a consistent template:

```markdown
# omnizip-{codec}

One-paragraph description.

## Wire format

Reference to spec. List of supported features.

## Quick start

```rust
use omnizip_{codec}::{Codec};
use omnizip_codecs::{Codec as _, CompressionLevel};

let codec = {Codec}::new();
let input = b"...";
let compressed = codec.compress(input, CompressionLevel::default()).unwrap();
let decompressed = codec.decompress(&compressed, input.len() as u32).unwrap();
assert_eq!(decompressed, input);
```

## Performance

Throughput numbers on representative inputs. Tuning knobs.

## Limitations

What's not supported. Why.
```

## Acceptance criteria

- [ ] README.md lands for every codec crate.
- [ ] `cargo doc` generates clean per-crate docs.
- [ ] Workspace README links to each crate's README.

## Priority

P2.
