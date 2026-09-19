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

use crate::codec::CodecId;
use crate::content_type::ContentType;

/// User-facing compression intent. See the [module docs](crate::profile)
/// for motivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Maximum speed. Skips dictionary, context modeling, optimal
    /// parsing. Use for hot-path writes where ratio is secondary
    /// (e.g., `LimniFS` `max-write` profile).
    Fast,
    /// Default. Reasonable ratio at acceptable speed. `LimniFS`
    /// `balanced` profile maps here.
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

// ============================================================================
// Named profiles — the Ruby gem's profile/ subsystem, ported
// field-by-field (TODO.ref-parity/60). The intent-based `Profile`
// above stays the codec-facing API; these are the user-facing
// presets the gem's CLI exposes.
// ============================================================================

/// Filter attached to a named profile (Ruby `filter:` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFilter {
    None,
    /// `:bcj_x86` — x86 branch filter ahead of the codec.
    BcjX86,
    /// `:auto` — pick the filter by content (maximum profile).
    Auto,
}

/// A named compression preset — Ruby `CompressionProfile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedProfile {
    pub name: &'static str,
    /// Algorithm (Ruby `algorithm:`); `LZMA2` maps to our LZMA codec
    /// (LZMA2 chunking is its encoder form), `:store` to STORE.
    pub codec: CodecId,
    pub level: u8,
    pub filter: ProfileFilter,
    pub solid: bool,
    pub description: &'static str,
}

/// The shipped presets, ported verbatim:
///
/// | name | algorithm | level | filter | solid |
/// |------|-----------|-------|--------|-------|
/// | fast | deflate | 1 | — | no |
/// | balanced | deflate | 6 | — | no |
/// | binary | lzma2 | 6 | bcj_x86 | no |
/// | archive | store | 0 | — | no |
/// | maximum | lzma2 | 9 | auto | yes |
#[must_use]
pub fn named_profiles() -> [NamedProfile; 5] {
    [
        NamedProfile {
            name: "fast",
            codec: CodecId::DEFLATE,
            level: 1,
            filter: ProfileFilter::None,
            solid: false,
            description: "Fast compression, lower ratio",
        },
        NamedProfile {
            name: "balanced",
            codec: CodecId::DEFLATE,
            level: 6,
            filter: ProfileFilter::None,
            solid: false,
            description: "Balanced speed/compression (default)",
        },
        NamedProfile {
            name: "binary",
            codec: CodecId::LZMA,
            level: 6,
            filter: ProfileFilter::BcjX86,
            solid: false,
            description: "Optimized for executables",
        },
        NamedProfile {
            name: "archive",
            codec: CodecId::STORE,
            level: 0,
            filter: ProfileFilter::None,
            solid: false,
            description: "No compression (already compressed files)",
        },
        NamedProfile {
            name: "maximum",
            codec: CodecId::LZMA,
            level: 9,
            filter: ProfileFilter::Auto,
            solid: true,
            description: "Maximum compression, slower",
        },
    ]
}

/// Look up a shipped preset by name.
#[must_use]
pub fn named_profile(name: &str) -> Option<NamedProfile> {
    named_profiles().into_iter().find(|p| p.name == name)
}

/// Ruby `CustomProfile`: user overrides, optionally inheriting a
/// base preset's unset fields.
#[derive(Debug, Clone)]
pub struct CustomProfile {
    pub name: String,
    pub codec: CodecId,
    pub level: u8,
    pub filter: ProfileFilter,
    pub solid: bool,
    pub description: String,
}

impl CustomProfile {
    /// Ruby default: deflate level 6, no filter, not solid.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            codec: CodecId::DEFLATE,
            level: 6,
            filter: ProfileFilter::None,
            solid: false,
            description: String::new(),
        }
    }

    /// Inherit every field from `base` (Ruby `base_profile:`).
    #[must_use]
    pub fn from_base(name: impl Into<String>, base: &NamedProfile) -> Self {
        Self {
            name: name.into(),
            codec: base.codec,
            level: base.level,
            filter: base.filter,
            solid: base.solid,
            description: base.description.to_string(),
        }
    }
}

/// Content category the detector maps to a profile priority order
/// (Ruby `MimeClassifier.profile_category`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCategory {
    Text,
    Binary,
    Archive,
    Other,
}

/// The sniffed prefixes that decide `Binary`/`Archive` categories:
/// executables and already-compressed media/containers. Pure
/// function of the first bytes; no fs access.
fn sniff_category(input: &[u8]) -> ContentCategory {
    // Archives / compressed containers.
    const ARCHIVE_MAGICS: [&[u8]; 9] = [
        b"PK\x03\x04",
        &[0x1F, 0x8B],
        &[0xFD, 0x37, 0x7A, 0x58, 0x5A],
        b"BZh",
        &[0x28, 0xB5, 0x2F, 0xFD],
        b"\x89PNG",
        &[0xFF, 0xD8, 0xFF],
        b"GIF8",
        &[0xFF, 0xFB],
    ];
    fn starts(input: &[u8], magic: &[u8]) -> bool {
        input.len() >= magic.len() && &input[..magic.len()] == magic
    }
    // Executables.
    if starts(input, b"\x7fELF")
        || starts(input, &[0xFE, 0xED, 0xFA, 0xCE])
        || starts(input, &[0xFE, 0xED, 0xFA, 0xCF])
        || starts(input, &[0xCE, 0xFA, 0xED, 0xFE])
        || starts(input, &[0xCF, 0xFA, 0xED, 0xFE])
        || starts(input, b"MZ")
    {
        return ContentCategory::Binary;
    }
    if ARCHIVE_MAGICS.iter().any(|m| starts(input, m)) || starts(input, b"ID3") {
        return ContentCategory::Archive;
    }
    match crate::content_type::ContentType::detect(input) {
        ContentType::Text | ContentType::Structured => ContentCategory::Text,
        ContentType::Binary | ContentType::Mixed => ContentCategory::Binary,
    }
}

/// Port of `ProfileDetector`: content → best named profile.
///
/// Selection = suitable profiles ∩ Ruby priority order:
/// text → [`text`(ghost), balanced] (no text profile ships in the
/// Ruby registry either — the entry is vestigial, balanced wins);
/// binary → [binary, maximum, balanced]; archive →
/// [archive, fast, balanced]; else → [balanced]. fast/balanced/
/// maximum are suitable for everything; binary for executables;
/// archive for archives/media. Deterministic: a pure function of
/// the sniffed bytes.
#[must_use]
pub fn detect_profile(input: &[u8]) -> NamedProfile {
    let profiles = named_profiles();
    let category = sniff_category(input);
    let suitable: Vec<NamedProfile> = profiles
        .into_iter()
        .filter(|p| match p.name {
            "binary" => category == ContentCategory::Binary,
            "archive" => category == ContentCategory::Archive,
            _ => true,
        })
        .collect();
    let priority: &[&str] = match category {
        ContentCategory::Text => &["text", "balanced"],
        ContentCategory::Binary => &["binary", "maximum", "balanced"],
        ContentCategory::Archive => &["archive", "fast", "balanced"],
        ContentCategory::Other => &["balanced"],
    };
    for name in priority {
        if let Some(p) = suitable.iter().find(|p| p.name == *name) {
            return *p;
        }
    }
    suitable.first().copied().unwrap_or(profiles[1])
}

#[cfg(test)]
mod named_tests {
    use super::*;

    #[test]
    fn mapping_table_matches_the_ruby_classes() {
        let p = named_profiles();
        let fast = p.iter().find(|p| p.name == "fast").unwrap();
        assert_eq!(
            (fast.codec, fast.level, fast.filter, fast.solid),
            (CodecId::DEFLATE, 1, ProfileFilter::None, false)
        );
        let balanced = p.iter().find(|p| p.name == "balanced").unwrap();
        assert_eq!(
            (
                balanced.codec,
                balanced.level,
                balanced.filter,
                balanced.solid
            ),
            (CodecId::DEFLATE, 6, ProfileFilter::None, false)
        );
        let binary = p.iter().find(|p| p.name == "binary").unwrap();
        assert_eq!(
            (binary.codec, binary.level, binary.filter, binary.solid),
            (CodecId::LZMA, 6, ProfileFilter::BcjX86, false)
        );
        let archive = p.iter().find(|p| p.name == "archive").unwrap();
        assert_eq!(
            (archive.codec, archive.level, archive.filter, archive.solid),
            (CodecId::STORE, 0, ProfileFilter::None, false)
        );
        let maximum = p.iter().find(|p| p.name == "maximum").unwrap();
        assert_eq!(
            (maximum.codec, maximum.level, maximum.filter, maximum.solid),
            (CodecId::LZMA, 9, ProfileFilter::Auto, true)
        );
    }

    #[test]
    fn detector_picks_the_ruby_priorities() {
        // Text → balanced (the Ruby `text` priority entry is a ghost).
        assert_eq!(
            detect_profile(b"hello world, plain text\n").name,
            "balanced"
        );
        // Executable → binary.
        assert_eq!(detect_profile(b"\x7fELF payload bytes").name, "binary");
        // PNG → archive.
        assert_eq!(detect_profile(b"\x89PNG\r\n\x1a\n rest").name, "archive");
        // Zip → archive.
        assert_eq!(detect_profile(b"PK\x03\x04zip body").name, "archive");
        // Generic binary blob (not executable, not archive) →
        // Ruby maps octet-stream to :binary → binary profile.
        assert_eq!(detect_profile(&[0u8, 159, 3, 255, 7, 0, 99]).name, "binary");
    }

    #[test]
    fn detector_is_deterministic_and_prefix_bounded() {
        let input = b"\x7fELF more bytes";
        let a = detect_profile(input);
        let b = detect_profile(input);
        assert_eq!(a, b);
        assert!(named_profile("maximum").unwrap().solid);
    }

    #[test]
    fn custom_profile_inheritance() {
        let c = CustomProfile::new("mine");
        assert_eq!((c.codec, c.level), (CodecId::DEFLATE, 6));
        let base = named_profile("maximum").unwrap();
        let c2 = CustomProfile::from_base("turbo", &base);
        assert_eq!((c2.codec, c2.level, c2.solid), (CodecId::LZMA, 9, true));
    }
}
