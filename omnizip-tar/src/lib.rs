//! TAR container — port of `omnizip/formats/tar/` (reader.rb,
//! writer.rb, header.rb, entry.rb, constants.rb) on the
//! [`omnizip_archive_core`] traits.
//!
//! Beyond the Ruby reference (per the task acceptance): GNU long
//! name/link (`L`/`K`) and pax extended headers (`x`) are READ, and
//! names beyond the ustar prefix split are WRITTEN as GNU `L` entries
//! — `bsdtar` and `tar` decode both.
#![forbid(unsafe_code)]

mod header;
mod reader;
mod writer;

pub use reader::TarReader;
pub use writer::TarWriter;

pub(crate) const HEADER_SIZE: usize = 512;
pub(crate) const BLOCK_SIZE: usize = 512;

pub(crate) const TYPE_REGULAR: u8 = b'0';
pub(crate) const TYPE_HARD_LINK: u8 = b'1';
pub(crate) const TYPE_SYMLINK: u8 = b'2';
pub(crate) const TYPE_DIRECTORY: u8 = b'5';
pub(crate) const TYPE_GNU_LONGNAME: u8 = b'L';
pub(crate) const TYPE_GNU_LONGLINK: u8 = b'K';
pub(crate) const TYPE_PAX_EXTENDED: u8 = b'x';

pub(crate) const USTAR_MAGIC: &[u8; 6] = b"ustar\x00";
pub(crate) const USTAR_VERSION: &[u8; 2] = b"00";
