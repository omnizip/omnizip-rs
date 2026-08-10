//! User-facing compression profile.
//!
//! [`Profile`] is a semantic intent — "max ratio", "fast", "balanced"
//! — that codecs translate to their internal compression level via
//! [`Codec::profile_to_level`](crate::Codec::profile_to_level).
//!
//! ## Why profiles?
//!
//! `CompressionLevel` is a raw `u8` whose meaning varies per codec
//! (Brotli: 0-11, ZSTD: 1-22, LZMA: 0-9). Callers hard-coding
//! `CompressionLevel::new(9)` don't realize that 9 is near-max for
//! LZMA but only mid-range for ZSTD.
//!
//! `Profile::Balanced` says what the caller wants; the codec knows
//! what level that means for its algorithm.
//!
//! ## Determinism
//!
//! `Profile` resolution is deterministic: same profile + same codec
//! → same internal level → same compressed output.

use crate::content_type::ContentType;

/// User-facing compression intent. See the [module docs](crate::profile)
/// for motivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Maximum speed. Skips dictionary, context modeling, optimal
    /// parsing. Use for hot-path writes where ratio is secondary
    /// (e.g., LimniFS `max-write` profile).
    Fast,
    /// Default. Reasonable ratio at acceptable speed. TheLimniFS
    /// "balanced" profile maps here.
    Balanced,
    /// Maximum ratio. Uses all features (dictionary, context modeling,
    /// optimal parser, multi-pass). Slowest. Use for cold-storage
    /// writes where compress-once-read-many is the workload.
    MaxRatio,
    /// Profile with content-type hint. Lets the codec skip detection
    /// and tune parser parameters up front.
    ForContent {
        /// The underlying profile.
        profile: ProfileKind,
        /// The content type hint.
        content: ContentType,
    },
    /// Fully custom. Caller knows the codec and provides the raw level.
    /// Use only when the caller has codec-specific knowledge.
    Custom(u8),
}

/// The non-content-tagged subset of [`Profile`], used inside
/// [`Profile::ForContent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileKind {
    Fast,
    Balanced,
    MaxRatio,
}

impl Profile {
    /// Convert a [`Profile`] to a raw compression level using the
    /// codec's defaults. The codec maps `Fast`/`Balanced`/`MaxRatio`
    /// to its own range.
    ///
    /// `Profile::Custom(level)` returns `level` unchanged.
    #[must_use]
    pub fn to_level<F>(&self, default_level: F) -> u8
    where
        F: FnOnce(ProfileKind) -> u8,
    {
        match self {
            Profile::Fast => default_level(ProfileKind::Fast),
            Profile::Balanced => default_level(ProfileKind::Balanced),
            Profile::MaxRatio => default_level(ProfileKind::MaxRatio),
            Profile::ForContent {
                profile,
                content: _,
            } => default_level(*profile),
            Profile::Custom(level) => *level,
        }
    }

    /// Returns the content hint if any. `None` means "let the codec
    /// auto-detect".
    #[must_use]
    pub const fn content_hint(self) -> Option<ContentType> {
        match self {
            Profile::ForContent { content, .. } => Some(content),
            _ => None,
        }
    }
}

impl From<ProfileKind> for Profile {
    fn from(kind: ProfileKind) -> Self {
        match kind {
            ProfileKind::Fast => Profile::Fast,
            ProfileKind::Balanced => Profile::Balanced,
            ProfileKind::MaxRatio => Profile::MaxRatio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_defaults(kind: ProfileKind) -> u8 {
        match kind {
            ProfileKind::Fast => 1,
            ProfileKind::Balanced => 6,
            ProfileKind::MaxRatio => 11,
        }
    }

    #[test]
    fn fast_maps_to_default_fast() {
        assert_eq!(Profile::Fast.to_level(sample_defaults), 1);
    }

    #[test]
    fn balanced_maps_to_default_balanced() {
        assert_eq!(Profile::Balanced.to_level(sample_defaults), 6);
    }

    #[test]
    fn max_ratio_maps_to_default_max() {
        assert_eq!(Profile::MaxRatio.to_level(sample_defaults), 11);
    }

    #[test]
    fn custom_passes_through() {
        assert_eq!(Profile::Custom(7).to_level(sample_defaults), 7);
    }

    #[test]
    fn for_content_uses_underlying_profile() {
        let p = Profile::ForContent {
            profile: ProfileKind::Fast,
            content: ContentType::Text,
        };
        assert_eq!(p.to_level(sample_defaults), 1);
        assert_eq!(p.content_hint(), Some(ContentType::Text));
    }

    #[test]
    fn no_content_hint_returns_none() {
        assert_eq!(Profile::Balanced.content_hint(), None);
    }

    #[test]
    fn profile_kind_converts_to_profile() {
        let p: Profile = ProfileKind::Balanced.into();
        assert_eq!(p, Profile::Balanced);
    }
}
