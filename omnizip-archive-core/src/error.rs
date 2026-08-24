//! Unified container error — port of `omnizip/error.rb`'s archive
//! slice, in idiomatic Rust (dependency-free: manual `Display`).
#![forbid(unsafe_code)]

use std::fmt;
use std::path::Path;

/// Errors raised by the container layer.
#[derive(Debug)]
#[non_exhaustive]
pub enum ArchiveError {
    UnsupportedFormat(String),
    InvalidArchive(String),
    UnsupportedFeature {
        reason: String,
    },
    Checksum(String),
    Io {
        context: &'static str,
        path: String,
        source: std::io::Error,
    },
    Security(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(s) => write!(f, "unsupported format: {s}"),
            Self::InvalidArchive(s) => write!(f, "invalid archive: {s}"),
            Self::UnsupportedFeature { reason } => write!(f, "unsupported feature: {reason}"),
            Self::Checksum(s) => write!(f, "checksum mismatch: {s}"),
            Self::Io {
                context,
                path,
                source,
            } => write!(f, "io error at {context} ({path}): {source}"),
            Self::Security(s) => write!(f, "security policy violation: {s}"),
        }
    }
}

impl std::error::Error for ArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl ArchiveError {
    #[must_use]
    pub fn io(context: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            context,
            path: path.display().to_string(),
            source,
        }
    }
}
