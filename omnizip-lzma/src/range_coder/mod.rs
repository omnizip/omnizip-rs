//! Range coder module — shared traits + the decoder (encoder lands in Phase B).
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/range_coder.rb` and
//! `range_decoder.rb` (MIT, Ribose Inc.).
//!
//! The Ruby inheritance hierarchy (`RangeCoder` base + `RangeDecoder <
//! RangeCoder`) collapses to two free-standing types here: the abstract
//! base contributes only factory helpers (`create_bit_models`,
//! `get_bit_model`) that the Rust side already has as
//! [`crate::bit_model::bit_models`]. No trait is needed.

pub mod decoder;

pub use decoder::RangeDecoder;
