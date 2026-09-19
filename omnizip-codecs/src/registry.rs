//! Codec registry — runtime dispatch by [`CodecId`].
//!
//! Adding a codec = implementing [`Codec`](crate::Codec) + calling
//! [`CodecRegistry::register`]. No dispatch code changes. See the crate-
//! level docs for the determinism requirement.

use std::sync::OnceLock;

use crate::codec::{Codec, CodecId};
use crate::error::OmnizipError;
use crate::level::CompressionLevel;

/// Process-wide registry of codecs, keyed by id.
pub struct CodecRegistry {
    codecs: Vec<Box<dyn Codec>>,
}

impl CodecRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { codecs: Vec::new() }
    }

    /// Register a codec. Id collisions are a programming error.
    ///
    /// # Panics
    ///
    /// Panics if a codec with the same id is already registered.
    pub fn register(&mut self, codec: Box<dyn Codec>) {
        let id = codec.id();
        assert!(
            !self.codecs.iter().any(|c| c.id() == id),
            "codec id {id} already registered",
        );
        self.codecs.push(codec);
    }

    fn find(&self, id: CodecId) -> Option<&dyn Codec> {
        self.codecs.iter().find(|c| c.id() == id).map(Box::as_ref)
    }

    /// All registered implementations that read/write the wire
    /// format `id`, in registration order (deterministic — the Vec,
    /// never hash iteration). Index 0 is the house default.
    #[must_use]
    pub fn for_format(&self, id: CodecId) -> Vec<&dyn Codec> {
        self.codecs
            .iter()
            .filter(|c| c.wire_format() == id)
            .map(Box::as_ref)
            .collect()
    }

    /// Pick an implementation of wire format `id`.
    ///
    /// `Default` selects the first registered impl (the house
    /// default); `Named` selects by [`Codec::impl_name`].
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Unsupported`] when no impl of `id` is
    /// registered, or when `Named` matches no impl (the reason
    /// lists the available names).
    pub fn codec_for_format(
        &self,
        id: CodecId,
        preference: ImplPreference,
    ) -> Result<&dyn Codec, OmnizipError> {
        let impls = self.for_format(id);
        match preference {
            ImplPreference::Default => impls.first().copied().ok_or_else(|| self.no_format(id)),
            ImplPreference::Named(want) => impls
                .iter()
                .find(|c| c.impl_name() == want)
                .copied()
                .ok_or_else(|| {
                    let names = impls
                        .iter()
                        .map(|c| c.impl_name())
                        .collect::<Vec<_>>()
                        .join(", ");
                    OmnizipError::Unsupported {
                        codec: id,
                        reason: format!(
                            "no implementation named '{want}' for format {id} (available: {names})"
                        ),
                    }
                }),
        }
    }

    fn no_format(&self, id: CodecId) -> OmnizipError {
        OmnizipError::Unsupported {
            codec: id,
            reason: format!(
                "no codec registered for format {id} (registered: {registered})",
                registered = self.registered_names()
            ),
        }
    }

    fn registered_names(&self) -> String {
        self.codecs
            .iter()
            .map(|c| format!("{}={}", c.id(), c.name()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Dispatch compression to the codec identified by `id`.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Unsupported`] if no codec with `id` is
    /// registered.
    pub fn compress(
        &self,
        id: CodecId,
        plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        match self.find(id) {
            Some(codec) => codec.compress(plaintext, level),
            None => Err(OmnizipError::Unsupported {
                codec: id,
                reason: format!(
                    "no codec registered with id {id} (registered: {registered})",
                    registered = self.registered_names()
                ),
            }),
        }
    }

    /// Dispatch decompression to the codec identified by `id`.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Unsupported`] if no codec with `id` is
    /// registered, or [`OmnizipError::DecodeFailed`] on decoder failure.
    pub fn decompress(
        &self,
        id: CodecId,
        compressed: &[u8],
        expected_len: u32,
    ) -> Result<Vec<u8>, OmnizipError> {
        match self.find(id) {
            Some(codec) => codec.decompress(compressed, expected_len),
            None => Err(OmnizipError::Unsupported {
                codec: id,
                reason: format!(
                    "no codec registered with id {id} (registered: {registered})",
                    registered = self.registered_names()
                ),
            }),
        }
    }

    /// The default pure-Rust registry. Empty until codec crates are wired
    /// in (tasks 10–25); consumers add codecs via [`Self::register`].
    ///
    /// `LimniFS` constructs its own registry at `limnifs-core/src/codec/`
    /// that includes the codecs it needs. This `default_pure_rust` is
    /// for standalone omnizip-rs use (benchmarks, fuzz, the CLI).
    #[must_use]
    pub fn default_pure_rust() -> Self {
        Self::new()
    }
}

impl Default for CodecRegistry {
    fn default() -> Self {
        Self::default_pure_rust()
    }
}

impl std::fmt::Debug for CodecRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodecRegistry")
            .field("codecs", &self.registered_names())
            .finish()
    }
}

static DEFAULT_REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();

/// How a caller picks an implementation of a wire format — see
/// [`CodecRegistry::codec_for_format`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplPreference {
    /// The house default: the first registered impl of the format.
    Default,
    /// A specific implementation by [`Codec::impl_name`] (e.g.
    /// `"libdeflate"` for the DEFLATE format).
    Named(&'static str),
}

/// Returns the process-wide default registry, initialised lazily.
///
/// Used by omnizip-rs's standalone tools (bench, fuzz, CLI). `LimniFS`
/// constructs its own registry at `limnifs-core/src/codec/`.
#[allow(dead_code)]
pub fn default_registry() -> &'static CodecRegistry {
    DEFAULT_REGISTRY.get_or_init(CodecRegistry::default_pure_rust)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopCodec {
        id: CodecId,
    }

    impl Codec for NoopCodec {
        fn id(&self) -> CodecId {
            self.id
        }
        fn name(&self) -> &'static str {
            "noop"
        }
        fn compress(
            &self,
            plaintext: &[u8],
            _level: CompressionLevel,
        ) -> Result<Vec<u8>, OmnizipError> {
            Ok(plaintext.to_vec())
        }
        fn decompress(
            &self,
            compressed: &[u8],
            expected_len: u32,
        ) -> Result<Vec<u8>, OmnizipError> {
            let expected = usize::try_from(expected_len).unwrap_or(0);
            if compressed.len() != expected {
                return Err(OmnizipError::LengthMismatch {
                    codec: self.id,
                    expected: expected_len,
                    actual: compressed.len(),
                });
            }
            Ok(compressed.to_vec())
        }
    }

    const NOOP_ID: CodecId = CodecId::new(0xFFFE);

    #[test]
    fn register_and_dispatch() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: NOOP_ID }));
        let compressed = registry
            .compress(NOOP_ID, b"abc", CompressionLevel::default())
            .expect("noop compress");
        assert_eq!(compressed, b"abc");
        let decompressed = registry
            .decompress(NOOP_ID, b"abc", 3)
            .expect("noop decompress");
        assert_eq!(decompressed, b"abc");
    }

    #[test]
    #[should_panic(expected = "codec id 0xFFFE already registered")]
    fn duplicate_id_panics() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: NOOP_ID }));
        registry.register(Box::new(NoopCodec { id: NOOP_ID }));
    }

    #[test]
    fn missing_codec_returns_unsupported() {
        let registry = CodecRegistry::new();
        let err = registry
            .compress(CodecId::LZMA, b"abc", CompressionLevel::default())
            .unwrap_err();
        assert!(matches!(err, OmnizipError::Unsupported { .. }));
    }

    #[test]
    fn codec_id_displays_hex() {
        assert_eq!(CodecId::STORE.to_string(), "0x0000");
        assert_eq!(CodecId::LZMA.to_string(), "0x0003");
        assert_eq!(CodecId::new(0xABCD).to_string(), "0xABCD");
    }

    #[test]
    fn level_orders_and_displays() {
        assert!(CompressionLevel::fastest() < CompressionLevel::default());
        assert!(CompressionLevel::default() < CompressionLevel::best());
        assert_eq!(CompressionLevel::default().to_string(), "level-6");
    }

    /// Alternative implementation of a format: different id, same
    /// wire format, distinct `impl_name`.
    struct AltCodec {
        id: CodecId,
        format: CodecId,
    }

    impl Codec for AltCodec {
        fn id(&self) -> CodecId {
            self.id
        }
        fn name(&self) -> &'static str {
            "alt"
        }
        fn wire_format(&self) -> CodecId {
            self.format
        }
        fn impl_name(&self) -> &'static str {
            "alt"
        }
        fn compress(
            &self,
            plaintext: &[u8],
            _level: CompressionLevel,
        ) -> Result<Vec<u8>, OmnizipError> {
            Ok(plaintext.to_vec())
        }
        fn decompress(
            &self,
            compressed: &[u8],
            expected_len: u32,
        ) -> Result<Vec<u8>, OmnizipError> {
            let expected = usize::try_from(expected_len).unwrap_or(0);
            if compressed.len() != expected {
                return Err(OmnizipError::LengthMismatch {
                    codec: self.id,
                    expected: expected_len,
                    actual: compressed.len(),
                });
            }
            Ok(compressed.to_vec())
        }
    }

    const FORMAT_ID: CodecId = CodecId::new(0xFFFD);
    const ALT_ID: CodecId = CodecId::new(0xFFFC);

    #[test]
    fn wire_format_defaults_to_id() {
        let codec = NoopCodec { id: NOOP_ID };
        assert_eq!(codec.wire_format(), NOOP_ID);
        assert_eq!(codec.impl_name(), "reference");
    }

    #[test]
    fn for_format_returns_impls_in_registration_order() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: FORMAT_ID }));
        registry.register(Box::new(AltCodec {
            id: ALT_ID,
            format: FORMAT_ID,
        }));
        let impls = registry.for_format(FORMAT_ID);
        assert_eq!(impls.len(), 2);
        assert_eq!(impls[0].id(), FORMAT_ID);
        assert_eq!(impls[1].id(), ALT_ID);
        // Deterministic across queries (Vec order, not hash order).
        let again: Vec<CodecId> = registry
            .for_format(FORMAT_ID)
            .into_iter()
            .map(|c: &dyn Codec| c.id())
            .collect();
        let first: Vec<CodecId> = impls.iter().map(|c| c.id()).collect();
        assert_eq!(again, first);
    }

    #[test]
    fn default_preference_picks_first_registered() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: FORMAT_ID }));
        registry.register(Box::new(AltCodec {
            id: ALT_ID,
            format: FORMAT_ID,
        }));
        let picked = registry
            .codec_for_format(FORMAT_ID, ImplPreference::Default)
            .expect("default impl");
        assert_eq!(picked.id(), FORMAT_ID);
    }

    #[test]
    fn named_preference_selects_impl() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: FORMAT_ID }));
        registry.register(Box::new(AltCodec {
            id: ALT_ID,
            format: FORMAT_ID,
        }));
        let picked = registry
            .codec_for_format(FORMAT_ID, ImplPreference::Named("alt"))
            .expect("alt impl");
        assert_eq!(picked.id(), ALT_ID);
    }

    #[test]
    fn unknown_name_and_unknown_format_error() {
        let mut registry = CodecRegistry::new();
        registry.register(Box::new(NoopCodec { id: FORMAT_ID }));
        let err = registry
            .codec_for_format(FORMAT_ID, ImplPreference::Named("nope"))
            .err()
            .expect("unknown impl name must error");
        assert!(err.to_string().contains("nope"), "{err}");
        assert!(err.to_string().contains("reference"), "{err}");

        let err = registry
            .codec_for_format(ALT_ID, ImplPreference::Default)
            .err()
            .expect("unregistered format must error");
        assert!(matches!(err, OmnizipError::Unsupported { .. }));
    }
}
