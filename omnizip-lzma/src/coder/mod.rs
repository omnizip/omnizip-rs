//! Coder module — literal, length, and distance sub-coders used by both
//! the encoder (Phase B) and decoder (Phase A).

pub mod decoder;
pub mod distance_decoder;
pub mod distance_encoder;
pub mod length_decoder;
pub mod length_encoder;
pub mod literal_decoder;
pub mod literal_encoder;

pub use distance_decoder::DistanceDecoder;
pub use distance_encoder::DistanceEncoder;
pub use length_decoder::LengthDecoder;
pub use length_encoder::LengthEncoder;
pub use literal_decoder::LiteralDecoder;
pub use literal_encoder::LiteralEncoder;
