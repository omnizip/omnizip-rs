//! Encoder infrastructure modules.
//!
//! Extracted from `from_spec_encoder.rs` for MECE separation of concerns.
//! Each module has a single responsibility:
//!
//! - [`bitwriter`]: LSB-first bit writer for Brotli's wire format
//! - [`dict_hash`]: Pre-computed hash table for O(1) dictionary lookups
//! - [`distance_config`]: NPOSTFIX/NDIRECT distance-code configuration
//! - [`context`]: Literal context computation (LSB6, UTF8, etc.)

pub mod bitwriter;
pub mod block_splitter;
pub mod btopt;
pub mod context;
pub mod dict_hash;
pub mod dict_hash_lut;
pub mod distance_config;
pub(crate) mod emission;
pub mod zopfli_hq;
