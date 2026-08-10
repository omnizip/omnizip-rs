# ADR-0003: Codec trait + CodecRegistry (OCP)

- **Status:** accepted
- **Date:** 2026-07-15
- **Deciders:** Ronald Tse, omnizip-rs maintainers

## Context

omnizip-rs must support 15+ compression codecs and may add more
in the future (e.g., Lempel-Ziv-Hwang, density, Zling).

The dispatch design determines:

- **How callers select a codec** — by id? by name? by feature flag?
- **Where codec-dispatch branches live** — scattered at call sites,
  or centralized in one registry?
- **What adding a new codec costs** — N lines of dispatch code
  changed, or zero?

Alternatives:

1. **`enum CodecKind` + `match` at every call site** — simplest
   but requires editing every call site when a codec is added.
   Violates OCP.
2. **Function table (array indexed by codec id)** — fast but
   requires pre-declaring all codecs in one place.
3. **Trait + Registry** — codecs register themselves; dispatch
   looks them up by id. OCP-friendly.

## Decision

**`Codec` trait + `CodecRegistry`** in `omnizip-codecs`:

```rust
pub trait Codec: Send + Sync {
    fn id(&self) -> CodecId;
    fn name(&self) -> &'static str;
    fn compress(&self, input: &[u8], level: CompressionLevel)
        -> Result<Vec<u8>, OmnizipError>;
    fn decompress(&self, input: &[u8], expected_len: u32)
        -> Result<Vec<u8>, OmnizipError>;
}

pub struct CodecRegistry { /* ... */ }

impl CodecRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, codec: Box<dyn Codec>);
    pub fn get(&self, id: CodecId) -> Option<&dyn Codec>;
}
```

Callers register the codecs they need; dispatch is `registry.get(id).compress(...)`.

## Consequences

**Positive**:
- **Open/Closed Principle** — adding a codec = creating a new crate
  that implements `Codec` + one `register()` call. Zero changes to
  dispatch code.
- **Decoupling** — application code depends only on `omnizip-codecs`,
  not on individual codec crates. Codecs are dependencies of the
  registry, not the application.
- **Testability** — mock codecs implement the trait; tests don't
  need the real codec to test dispatch logic.
- **Feature-gating** — codecs can be optional; the registry starts
  empty and consumers register only what they need.

**Negative**:
- **Dynamic dispatch overhead** — one vtable lookup per call.
  Negligible compared to compression time.
- **Heap allocation per codec** — each registered codec is a
  `Box<dyn Codec>`. Acceptable; codecs are long-lived.
- **No compile-time exhaustiveness** — `match` on `CodecId` can't
  prove all variants are handled. Acceptable; runtime `Option`
  handles the missing case.

**Neutral**:
- Matches the [Ruby omnizip `Codecs` registry](https://github.com/omnizip/omnizip).

## References

- [Open/Closed Principle](https://en.wikipedia.org/wiki/Open%E2%80%93closed_principle)
- [Rust traits](https://doc.rust-lang.org/book/ch10-02-traits.html)
- [`CodecId` source](../../omnizip-codecs/src/codec.rs)
