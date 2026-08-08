//! Encoder infrastructure modules.
//!
//! Extracted from `from_spec_encoder.rs` for MECE separation of concerns.
//! Each module has a single responsibility:
//!
//! - [`bitwriter`]: LSB-first bit writer for Brotli's wire format
//! - [`distance_config`]: NPOSTFIX/NDIRECT distance-code configuration
//! - [`context`]: Literal context computation (LSB6, UTF8, etc.)

pub mod bitwriter;
pub mod context;
pub mod distance_config;
