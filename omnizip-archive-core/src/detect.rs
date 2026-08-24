//! Format detection by magic bytes — port of `omnizip/file_type.rb`'s
//! sniffing (the Ruby spec vectors drive the tests).
#![forbid(unsafe_code)]

/// Detected archive/compression format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatKind {
    Tar,
    Zip,
    Gzip,
    Bzip2,
    Xz,
    Zstd,
    Lz4,
    SevenZip,
    Rar4,
    Rar5,
    Cpio,
    Lzip,
    LzmaAlone,
    Unknown,
}

/// Sniff a format from leading bytes. TAR has no magic at offset 0 —
/// detected via the `ustar` string at offset 257 or heuristically from
/// a plausible header checksum (tar files under 512 bytes or missing
/// the magic cannot be told apart from arbitrary data; we report
/// `Unknown` rather than guess).
#[must_use]
pub fn detect_format(data: &[u8]) -> FormatKind {
    if data.len() < 4 {
        return FormatKind::Unknown;
    }
    if data.starts_with(&[0x1F, 0x8B]) {
        return FormatKind::Gzip;
    }
    if data.starts_with(b"BZh") {
        return FormatKind::Bzip2;
    }
    if data.starts_with(&[0xFD, b'7', b'z', b'X', b'Z']) {
        return FormatKind::Xz;
    }
    if data.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return FormatKind::Zstd;
    }
    if data.starts_with(&[0x04, 0x22, 0x4D, 0x18]) {
        return FormatKind::Lz4;
    }
    if data.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return FormatKind::SevenZip;
    }
    if data.starts_with(b"Rar!\x1A\x07\x00") {
        return FormatKind::Rar4;
    }
    if data.starts_with(b"Rar!\x1A\x07\x01\x00") {
        return FormatKind::Rar5;
    }
    if data.starts_with(b"PK\x03\x04")
        || data.starts_with(b"PK\x05\x06")
        || data.starts_with(b"PK\x07\x08")
    {
        return FormatKind::Zip;
    }
    if data.starts_with(b"0707") || data.starts_with(b"\x71\xC7") {
        return FormatKind::Cpio;
    }
    if data.starts_with(b"LZIP") {
        return FormatKind::Lzip;
    }
    if data.starts_with(&[0x5D, 0x00, 0x00]) {
        return FormatKind::LzmaAlone;
    }
    if data.len() >= 262 && (&data[257..262] == b"ustar") {
        return FormatKind::Tar;
    }
    if data.len() >= 512 && checksum_is_tar_plausible(data) {
        return FormatKind::Tar;
    }
    FormatKind::Unknown
}

/// Pre-POSIX tar (no magic): the header checksum must match.
fn checksum_is_tar_plausible(header: &[u8]) -> bool {
    let field = &header[148..156];
    let stored = trim_octal(field);
    let sum: u32 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                u32::from(b' ') * 8
            } else {
                u32::from(b)
            }
        })
        .sum();
    sum == stored
}

fn trim_octal(field: &[u8]) -> u32 {
    let text: Vec<u8> = field
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .map(|b| b as char)
        .filter(char::is_ascii_digit)
        .map(|c| c as u8)
        .collect();
    let s = String::from_utf8_lossy(&text).trim().to_string();
    u32::from_str_radix(&s, 8).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_every_magic() {
        assert_eq!(detect_format(&[0x1F, 0x8B, 8, 0]), FormatKind::Gzip);
        assert_eq!(detect_format(b"BZh9xxxx"), FormatKind::Bzip2);
        assert_eq!(
            detect_format(&[0xFD, b'7', b'z', b'X', b'Z', 0]),
            FormatKind::Xz
        );
        assert_eq!(
            detect_format(&[0x28, 0xB5, 0x2F, 0xFD, 0, 1]),
            FormatKind::Zstd
        );
        assert_eq!(
            detect_format(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]),
            FormatKind::SevenZip
        );
        assert_eq!(detect_format(b"Rar!\x1A\x07\x00abcd"), FormatKind::Rar4);
        assert_eq!(detect_format(b"Rar!\x1A\x07\x01\x00abcd"), FormatKind::Rar5);
        assert_eq!(detect_format(b"PK\x03\x04xxxx"), FormatKind::Zip);
        assert_eq!(detect_format(b"070701xx"), FormatKind::Cpio);
        assert_eq!(detect_format(b"LZIP-1.0"), FormatKind::Lzip);
    }

    #[test]
    fn sniffs_tar_by_ustar_magic() {
        let mut buf = vec![0u8; 600];
        buf[257..262].copy_from_slice(b"ustar");
        assert_eq!(detect_format(&buf), FormatKind::Tar);
    }

    #[test]
    fn unknown_for_garbage() {
        assert_eq!(detect_format(b"hello world!"), FormatKind::Unknown);
    }
}
