//! Encoder errors for the Brotli codec.
//!
//! Used by [`crate::encoder`] for input-size validation.

/// Errors that the pure-Rust Brotli encoder can return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// Input exceeds the encoder's maximum supported size.
    InputTooLarge {
        /// Length of the input that exceeded the cap.
        len: usize,
        /// Maximum allowed length.
        max: usize,
    },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge { len, max } => {
                write!(f, "brotli input too large: {len} bytes (max {max})")
            }
        }
    }
}

impl std::error::Error for EncodeError {}