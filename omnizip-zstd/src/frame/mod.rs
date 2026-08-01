//! ZSTD frame module — header + block parsers.

pub mod block;
pub mod header;

pub use block::BlockHeader;
pub use header::{detect_frame_kind, strip_magic, FrameHeader};
