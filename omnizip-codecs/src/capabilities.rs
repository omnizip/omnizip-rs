//! Codec capability metadata (TODO 261).
//!
//! Static metadata describing what a [`Codec`](crate::Codec) supports.
//! Lets callers discover capabilities without try/except or docs lookup.

/// Per-codec capability descriptor. Returned by
/// [`Codec::capabilities`](crate::Codec::capabilities).
///
/// ## Determinism
///
/// All fields are static — same codec returns same Capabilities every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Minimum supported compression level (inclusive).
    pub min_level: u8,
    /// Maximum supported compression level (inclusive).
    pub max_level: u8,

    /// Whether the codec implements [`StreamingEncoder`](crate::StreamingEncoder) /
    /// [`StreamingDecoder`](crate::StreamingDecoder).
    pub streaming: bool,

    /// Whether the codec supports [`ParallelBatch`](crate::ParallelBatch) (always true
    /// via blanket impl, but a codec may disable by overriding).
    pub parallel_batch: bool,

    /// Whether the codec uses a built-in static dictionary (text-heavy codecs).
    pub has_static_dictionary: bool,

    /// Whether the codec is content-type-aware (uses [`ContentType`](crate::ContentType) hints).
    pub content_type_aware: bool,

    /// Approximate best-case throughput in MB/s on modern hardware.
    /// Ballpark figure for caller-side planning; not a guarantee.
    pub approx_throughput_mbps: u32,
}

impl Capabilities {
    /// Returns true if `level` is within `[min_level, max_level]`.
    #[must_use]
    pub const fn supports_level(self, level: u8) -> bool {
        level >= self.min_level && level <= self.max_level
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            min_level: 1,
            max_level: 9,
            streaming: false,
            parallel_batch: true,
            has_static_dictionary: false,
            content_type_aware: false,
            approx_throughput_mbps: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_sensible() {
        let caps = Capabilities::default();
        assert!(caps.supports_level(5));
        assert!(!caps.supports_level(0));
        assert!(caps.parallel_batch);
    }

    #[test]
    fn supports_level_respects_bounds() {
        let caps = Capabilities {
            min_level: 0,
            max_level: 11,
            ..Capabilities::default()
        };
        assert!(caps.supports_level(0));
        assert!(caps.supports_level(11));
        assert!(!caps.supports_level(12));
    }
}
