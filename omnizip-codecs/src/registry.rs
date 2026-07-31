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
}
