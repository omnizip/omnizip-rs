//! LZMA encoder module.

pub mod alone;
pub mod lzip;
pub mod lzma1;
pub mod lzma2;
pub mod match_finder;
pub mod optimal;
pub mod prob_state;
pub mod xz;

pub use alone::lzma_alone_compress;
pub use alone::lzma_alone_compress_with_options;
pub use lzip::lzip_compress;
pub use lzma1::Lzma1Encoder;
pub use lzma2::encode_lzma2_stream;
pub use match_finder::MatchFinder;
pub use optimal::optimal_parse_actions;
pub use prob_state::{LzmaProbState, LzmaState};
pub use xz::xz_compress;
pub use xz::xz_compress_with_options;
