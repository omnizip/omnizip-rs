//! Unified codec options builder (TODO 262).
//!
//! Codec-agnostic options struct. Callers configure common knobs (level,
//! profile, content hint) via one builder; per-codec knobs (window_log,
//! chain_log, dictionary) live in the same struct but only the relevant
//! codec reads them.

use crate::content_type::ContentType;
use crate::level::CompressionLevel;
use crate::profile::Profile;

/// Codec-agnostic options builder.
///
/// Construct via [`Options::default`] then chain `.level(...)`,
/// `.profile(...)`, `.content_hint(...)` etc. Pass to
/// [`Codec::compress_with_options`](crate::Codec::compress_with_options).
///
/// ## Determinism
///
/// Same `Options` + same input → byte-identical output (delegated to
/// the underlying codec's determinism guarantee).
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Raw compression level. Takes precedence over `profile` if both set.
    pub level: Option<CompressionLevel>,
    /// Semantic profile (Fast/Balanced/MaxRatio). Used when `level` is None.
    pub profile: Option<Profile>,
    /// Content-type hint. Lets the codec skip detection and tune parser.
    pub content_hint: Option<ContentType>,

    // Per-codec knobs. Only the relevant codec reads these.
    /// LZMA / ZSTD window log (power of 2).
    pub window_log: Option<u32>,
    /// ZSTD chain log.
    pub chain_log: Option<u32>,
    /// ZSTD hash log.
    pub hash_log: Option<u32>,
    /// ZSTD search log.
    pub search_log: Option<u32>,
    /// ZSTD min match length.
    pub min_match: Option<u32>,
    /// ZSTD target length (for btopt strategies).
    pub target_length: Option<u32>,
    /// Optional dictionary (ZSTD dictionary support).
    pub dictionary: Option<Vec<u8>>,
}

impl Options {
    /// Set raw compression level. Overrides any previously-set profile.
    #[must_use]
    pub fn with_level(mut self, level: u8) -> Self {
        self.level = Some(CompressionLevel::new(level));
        self.profile = None;
        self
    }

    /// Set semantic profile. Overrides any previously-set level.
    #[must_use]
    pub fn with_profile(mut self, profile: Profile) -> Self {
        self.profile = Some(profile);
        self.level = None;
        self
    }

    /// Set content-type hint.
    #[must_use]
    pub fn with_content_hint(mut self, content: ContentType) -> Self {
        self.content_hint = Some(content);
        self
    }

    /// Set LZMA/ZSTD window log.
    #[must_use]
    pub fn with_window_log(mut self, log: u32) -> Self {
        self.window_log = Some(log);
        self
    }

    /// Set ZSTD chain log.
    #[must_use]
    pub fn with_chain_log(mut self, log: u32) -> Self {
        self.chain_log = Some(log);
        self
    }

    /// Attach a dictionary (ZSTD).
    #[must_use]
    pub fn with_dictionary(mut self, dict: Vec<u8>) -> Self {
        self.dictionary = Some(dict);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_empty() {
        let opts = Options::default();
        assert!(opts.level.is_none());
        assert!(opts.profile.is_none());
        assert!(opts.content_hint.is_none());
    }

    #[test]
    fn builder_chain() {
        let opts = Options::default()
            .with_level(5)
            .with_content_hint(ContentType::Text)
            .with_window_log(20);
        assert_eq!(opts.level.unwrap().as_u8(), 5);
        assert_eq!(opts.content_hint.unwrap(), ContentType::Text);
        assert_eq!(opts.window_log.unwrap(), 20);
    }

    #[test]
    fn level_overrides_profile() {
        let opts = Options::default()
            .with_profile(Profile::Balanced)
            .with_level(9);
        assert!(opts.level.is_some());
        assert!(opts.profile.is_none());
    }

    #[test]
    fn profile_overrides_level() {
        let opts = Options::default().with_level(9).with_profile(Profile::Fast);
        assert!(opts.profile.is_some());
        assert!(opts.level.is_none());
    }
}
