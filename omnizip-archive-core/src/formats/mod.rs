//! Single-file compression formats built on the shipped codecs.
#![forbid(unsafe_code)]

pub mod bzip2_file;
pub mod gzip;
pub mod lzip;
pub mod lzma_alone;
