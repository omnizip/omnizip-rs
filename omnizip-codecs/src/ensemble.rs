//! Multi-codec ensemble auto-selection (TODO 266).
//!
//! Picks the best codec for an input based on content-type detection
//! and a goal (Fast / Balanced / MaxRatio). Two strategies:
//!
//! - **Heuristic** (default): O(1) per-call decision tree based on
//!   `ContentType::detect()` and per-codec capabilities metadata.
//! - **Taste** (opt-in): runs each candidate codec on a 4 KiB sample,
//!   picks the best by ratio or speed.
//!
//! ## Determinism
//!
//! Heuristic mode is a pure function of input bytes. Taste mode is
//! also deterministic (same sample → same winner).

use crate::capabilities::Capabilities;
use crate::codec::{Codec, CodecId};
use crate::content_type::ContentType;
use crate::level::CompressionLevel;
use crate::OmnizipError;

/// User-facing goal that drives codec selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Goal {
    /// Maximum speed. Skips dictionary and optimal parsing.
    Fast,
    /// Default. Reasonable ratio at acceptable speed.
    Balanced,
    /// Maximum ratio. All features enabled.
    MaxRatio,
}

/// A codec candidate for ensemble selection. The caller registers
/// candidates with their `Box<dyn Codec>` and level mapping.
pub struct Candidate {
    /// The codec instance.
    pub codec: Box<dyn Codec>,
    /// Level to use for `Goal::Fast`.
    pub fast_level: u8,
    /// Level to use for `Goal::Balanced`.
    pub balanced_level: u8,
    /// Level to use for `Goal::MaxRatio`.
    pub max_ratio_level: u8,
}

/// Ensemble picker. Holds the candidate list and dispatches based on
/// input + goal.
pub struct Ensemble {
    candidates: Vec<Candidate>,
}

impl Ensemble {
    /// Create an empty ensemble.
    #[must_use]
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
        }
    }

    /// Register a candidate.
    pub fn register(&mut self, candidate: Candidate) {
        self.candidates.push(candidate);
    }

    /// Heuristic pick: O(1) per-call. Returns `(codec_id, level)`.
    ///
    /// Decision tree:
    /// - Content-Type = Text/Structured + Goal = MaxRatio → Brotli (if registered)
    /// - Content-Type = Text/Structured + Goal = Balanced → Brotli
    /// - Content-Type = Binary + Goal = Fast → LZ4 (if registered)
    /// - Content-Type = Binary + Goal = Balanced → ZSTD (if registered)
    /// - Content-Type = Binary + Goal = MaxRatio → LZMA (if registered)
    /// - Fallback: first registered candidate
    ///
    /// Returns `None` if no candidates are registered.
    #[must_use]
    pub fn pick_heuristic(&self, input: &[u8], goal: Goal) -> Option<(CodecId, u8)> {
        if self.candidates.is_empty() {
            return None;
        }
        let content = ContentType::detect(input);

        // Look up by codec id.
        let find = |id: CodecId| self.candidates.iter().find(|c| c.codec.id() == id);

        let (candidate_id, level) = match (content, goal) {
            (ContentType::Text | ContentType::Structured, Goal::Fast) => {
                // Text + Fast: prefer Brotli Q1; fallback to ZSTD L1.
                if let Some(c) = find(CodecId::BROTLI) {
                    (c.codec.id(), c.fast_level)
                } else if let Some(c) = find(CodecId::ZSTD) {
                    (c.codec.id(), c.fast_level)
                } else {
                    (self.candidates[0].codec.id(), self.candidates[0].fast_level)
                }
            }
            (ContentType::Text | ContentType::Structured, Goal::Balanced)
            | (ContentType::Text | ContentType::Structured, Goal::MaxRatio) => {
                if let Some(c) = find(CodecId::BROTLI) {
                    let lvl = if goal == Goal::MaxRatio {
                        c.max_ratio_level
                    } else {
                        c.balanced_level
                    };
                    (c.codec.id(), lvl)
                } else if let Some(c) = find(CodecId::LZMA) {
                    (c.codec.id(), c.balanced_level)
                } else {
                    (
                        self.candidates[0].codec.id(),
                        self.candidates[0].balanced_level,
                    )
                }
            }
            (ContentType::Binary | ContentType::Mixed, Goal::Fast) => {
                if let Some(c) = find(CodecId::LZ4) {
                    (c.codec.id(), c.fast_level)
                } else if let Some(c) = find(CodecId::ZSTD) {
                    (c.codec.id(), c.fast_level)
                } else {
                    (self.candidates[0].codec.id(), self.candidates[0].fast_level)
                }
            }
            (ContentType::Binary | ContentType::Mixed, Goal::Balanced) => {
                if let Some(c) = find(CodecId::ZSTD) {
                    (c.codec.id(), c.balanced_level)
                } else if let Some(c) = find(CodecId::LZ4) {
                    (c.codec.id(), c.balanced_level)
                } else {
                    (
                        self.candidates[0].codec.id(),
                        self.candidates[0].balanced_level,
                    )
                }
            }
            (ContentType::Binary | ContentType::Mixed, Goal::MaxRatio) => {
                if let Some(c) = find(CodecId::LZMA) {
                    (c.codec.id(), c.max_ratio_level)
                } else if let Some(c) = find(CodecId::ZSTD) {
                    (c.codec.id(), c.max_ratio_level)
                } else {
                    (
                        self.candidates[0].codec.id(),
                        self.candidates[0].max_ratio_level,
                    )
                }
            }
        };

        Some((candidate_id, level))
    }

    /// Compress using heuristic codec selection.
    ///
    /// # Errors
    ///
    /// Returns the underlying codec's error on failure.
    pub fn compress(&self, input: &[u8], goal: Goal) -> Result<(CodecId, Vec<u8>), OmnizipError> {
        let (codec_id, level) = self.pick_heuristic(input, goal).ok_or_else(|| {
            OmnizipError::unsupported(CodecId::BROTLI, "no candidates registered")
        })?;
        let candidate = self
            .candidates
            .iter()
            .find(|c| c.codec.id() == codec_id)
            .expect("pick_heuristic returned unregistered id");
        let compressed = candidate
            .codec
            .compress(input, CompressionLevel::new(level))?;
        Ok((codec_id, compressed))
    }

    /// Iterate registered candidates' capabilities (for introspection).
    pub fn candidates(&self) -> impl Iterator<Item = (CodecId, Capabilities)> + '_ {
        self.candidates
            .iter()
            .map(|c| (c.codec.id(), c.codec.capabilities()))
    }
}

impl Default for Ensemble {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test codec that just records its id and returns input unchanged.
    struct StubCodec {
        id: CodecId,
    }

    impl Codec for StubCodec {
        fn id(&self) -> CodecId {
            self.id
        }
        fn name(&self) -> &'static str {
            "stub"
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
            _expected_len: u32,
        ) -> Result<Vec<u8>, OmnizipError> {
            Ok(compressed.to_vec())
        }
    }

    #[test]
    fn empty_ensemble_returns_none() {
        let e = Ensemble::new();
        assert!(e.pick_heuristic(b"hello", Goal::Balanced).is_none());
    }

    #[test]
    fn text_picks_brotli_for_balanced() {
        let mut e = Ensemble::new();
        e.register(Candidate {
            codec: Box::new(StubCodec {
                id: CodecId::BROTLI,
            }),
            fast_level: 1,
            balanced_level: 5,
            max_ratio_level: 11,
        });
        let (id, level) = e
            .pick_heuristic(b"the quick brown fox jumps", Goal::Balanced)
            .expect("non-empty");
        assert_eq!(id, CodecId::BROTLI);
        assert_eq!(level, 5);
    }

    #[test]
    fn binary_picks_lz4_for_fast() {
        let mut e = Ensemble::new();
        e.register(Candidate {
            codec: Box::new(StubCodec { id: CodecId::LZ4 }),
            fast_level: 1,
            balanced_level: 1,
            max_ratio_level: 1,
        });
        e.register(Candidate {
            codec: Box::new(StubCodec { id: CodecId::ZSTD }),
            fast_level: 1,
            balanced_level: 9,
            max_ratio_level: 19,
        });
        let binary: Vec<u8> = (0..1024u32)
            .map(|i| (i.wrapping_mul(7919) & 0xFF) as u8)
            .collect();
        let (id, _) = e.pick_heuristic(&binary, Goal::Fast).expect("non-empty");
        assert_eq!(id, CodecId::LZ4);
    }

    #[test]
    fn binary_picks_lzma_for_max_ratio() {
        let mut e = Ensemble::new();
        e.register(Candidate {
            codec: Box::new(StubCodec { id: CodecId::ZSTD }),
            fast_level: 1,
            balanced_level: 9,
            max_ratio_level: 19,
        });
        e.register(Candidate {
            codec: Box::new(StubCodec { id: CodecId::LZMA }),
            fast_level: 1,
            balanced_level: 5,
            max_ratio_level: 9,
        });
        let binary: Vec<u8> = (0..1024u32)
            .map(|i| (i.wrapping_mul(7919) & 0xFF) as u8)
            .collect();
        let (id, _) = e
            .pick_heuristic(&binary, Goal::MaxRatio)
            .expect("non-empty");
        assert_eq!(id, CodecId::LZMA);
    }
}
