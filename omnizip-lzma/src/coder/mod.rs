//! Coder module — literal, length, and distance sub-coders used by both
//! the encoder (Phase B) and decoder (Phase A).
//!
//! The Ruby keeps encode and decode in a single class per concern
//! (`LengthCoder`, `DistanceCoder`). In Rust, encode and decode get
//! separate types because the encode half needs an `RangeEncoder`
//! (Phase B) while the decode half needs only the `RangeDecoder` shipped
//! here — keeping the type graph cycle-free.
//!
//! Shared tree-decode helpers live in this file because both length and
//! distance coders consume the same LZMA bit-tree shape.

pub mod decoder;
pub mod distance_decoder;
pub mod length_decoder;
pub mod literal_decoder;

pub use distance_decoder::DistanceDecoder;
pub use length_decoder::LengthDecoder;
pub use literal_decoder::LiteralDecoder;
