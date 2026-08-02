//! Range coder module — decoder + encoder.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/range_coder.rb` and
//! `range_decoder.rb` / `range_encoder.rb` (MIT, Ribose Inc.).

pub mod decoder;
pub mod encoder;

pub use decoder::RangeDecoder;
pub use encoder::RangeEncoder;
