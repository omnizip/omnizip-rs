//! FSE (Finite State Entropy) module — bitstream readers + decoding
//! table + decoder.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/zstandard/fse/` (486 LOC
//! across 3 Ruby files, MIT, Ribose Inc.).
//!
//! ## Layout
//!
//! - [`bitstream`]: forward + reverse bit readers.
//! - [`table`]: FSE table builder + stateful decoder.

pub mod bitstream;
pub mod encoder;
pub mod from_stream;
pub mod interleaved;
pub mod table;

pub use bitstream::{BitStream, ForwardBitStream};
pub use encoder::{build_ctable, compress as fse_compress, compress_using_ctable,
                  normalize_count, optimal_table_log, write_ncount, CTable};
pub use from_stream::read_fse_table;
pub use interleaved::decode_stream;
pub use table::{FseDecoder, FseState, Table};
