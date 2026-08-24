//! ZIP container — port of `omnizip/formats/zip/` + `omnizip/zip/` on
//! the [`omnizip_archive_core`] traits: local headers, central
//! directory, EOCD (+ ZIP64), methods store(0) / deflate(8) /
//! bzip2(12) / zstd(93), deterministic normalization.
#![forbid(unsafe_code)]

mod reader;
mod writer;

pub use reader::ZipReader;
pub use writer::{ZipMethod, ZipWriter};

pub(crate) const LOCAL_SIG: u32 = 0x0403_4B50;
pub(crate) const CENTRAL_SIG: u32 = 0x0201_4B50;
pub(crate) const EOCD_SIG: u32 = 0x0605_4B50;
pub(crate) const ZIP64_EOCD_SIG: u32 = 0x0606_4B50;
pub(crate) const ZIP64_LOCATOR_SIG: u32 = 0x0706_4B50;
pub(crate) const ZIP64_EXTRA_TAG: u16 = 0x0001;

pub(crate) const METHOD_STORE: u16 = 0;
pub(crate) const METHOD_DEFLATE: u16 = 8;
pub(crate) const METHOD_BZIP2: u16 = 12;
pub(crate) const METHOD_ZSTD: u16 = 93;

pub(crate) const FLAG_UTF8: u16 = 0x0800;

pub(crate) const VERSION_DEFAULT: u16 = 20;
pub(crate) const VERSION_ZIP64: u16 = 45;
pub(crate) const VERSION_BZIP2: u16 = 46;
pub(crate) const VERSION_ZSTD: u16 = 2000; // common mapping for 7-zip-style zstd

pub(crate) const ATTR_DIRECTORY: u32 = 0x10;
pub(crate) const UNIX_DIR_MODE: u32 = 0o755 << 16;
pub(crate) const UNIX_FILE_MODE: u32 = 0o644 << 16;
pub(crate) const UNIX_SYMLINK_MODE: u32 = 0o120777 << 16;
