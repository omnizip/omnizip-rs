//! CPIO container — port of `omnizip/formats/cpio/` (reader.rb,
//! writer.rb, entry.rb, constants.rb) on the
//! [`omnizip_archive_core`] traits. Supports the newc ASCII format
//! and its CRC variant (both read and write). ODC and bin formats
//! remain future work.
#![forbid(unsafe_code)]

mod reader;
mod writer;

pub use reader::CpioReader;
pub use writer::CpioWriter;

/// Format selector for [`CpioWriter::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpioFormat {
    /// SVR4 new ASCII format (initramfs default).
    Newc,
    /// newc + per-entry CRC32 checksums.
    Crc,
}

const MAGIC_NEWC: &[u8; 6] = b"070701";
const MAGIC_CRC: &[u8; 6] = b"070702";

const HEADER_SIZE: usize = 110;

/// newc header fields are `width` HEX characters (the magic is ASCII
/// but every field after it is 8-digit hex — SVR4 spec). Values that
/// don't fit saturate (the real spec writes NBIN extended headers).
fn encode_hex(value: u64, width: usize) -> Vec<u8> {
    let max = if width >= 16 {
        u64::MAX
    } else {
        (1u64 << (width * 4)) - 1
    };
    let text = format!("{:0width$x}", value.min(max), width = width);
    text.into_bytes()
}
fn parse_hex_at(buf: &[u8], offset: usize, width: usize) -> u64 {
    let s = std::str::from_utf8(&buf[offset..offset + width])
        .map(|s| s.trim())
        .unwrap_or("");
    u64::from_str_radix(s, 16).unwrap_or(0)
}
fn parse_crc(buf: &[u8], offset: usize, width: usize) -> u32 {
    parse_hex_at(buf, offset, width) as u32
}
fn pad4(n: usize) -> usize {
    (4 - n % 4) % 4
}
