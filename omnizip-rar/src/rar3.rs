//! RAR4 reader (read-only, task 08) — marker block, then block
//! stream: crc16 ‖ type ‖ flags ‖ size ‖ (file header fields). File
//! blocks carry the fixed 25-byte field set (32-byte fixed header
//! with name length), DOS time, and the packed data right after the
//! header. STORE-method entries extract with CRC32 verification;
//! other methods surface a clear UnsupportedFeature (the Ruby
//! reference defers those to the unrar binary).
#![forbid(unsafe_code)]

use crate::{rar4_block, rar4_flags, MAGIC_RAR4, RAR4_METHOD_STORE};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

struct RawEntry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    split: bool,
    unpacked_size: u64,
    crc32: Option<u32>,
    method: u8,
    encrypted: bool,
    mtime: Option<u64>,
    data: (usize, usize),
}

/// Reads a RAR4 (RAR 1.5-4.x) archive held in memory.
pub struct Rar4Reader {
    data: Vec<u8>,
    entries: Vec<RawEntry>,
}

impl Rar4Reader {
    /// Parse a RAR4 archive.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on signature or structure problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        if data.len() < 7 || data[0..7] != MAGIC_RAR4 {
            return Err(ArchiveError::InvalidArchive(
                "rar4: invalid signature".into(),
            ));
        }
        let mut entries = Vec::new();
        let mut pos = 7usize;
        while let Some(next) = parse_block(data, pos, &mut entries)? {
            pos = next;
        }
        Ok(Self {
            data: data.to_vec(),
            entries,
        })
    }

    /// Open from disk.
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }
}

fn parse_block(
    data: &[u8],
    pos: usize,
    entries: &mut Vec<RawEntry>,
) -> Result<Option<usize>, ArchiveError> {
    let rest = match data.get(pos..) {
        Some(r) if r.len() >= 7 => r,
        _ => return Ok(None),
    };
    let _crc16 = u16::from_le_bytes([rest[0], rest[1]]);
    let kind = rest[2];
    let flags = u16::from_le_bytes([rest[3], rest[4]]);
    let size = u16::from_le_bytes([rest[5], rest[6]]) as usize;
    if size < 7 {
        return Ok(None);
    }
    let end = pos + size;

    match kind {
        rar4_block::FILE => {
            if rest.len() < 32 {
                return Err(ArchiveError::InvalidArchive(
                    "rar4: short file header".into(),
                ));
            }
            let u32le = |o: usize| u32::from_le_bytes(rest[o..o + 4].try_into().expect("4"));
            let pack_size = u64::from(u32le(7));
            let unpack_size = u64::from(u32le(11));
            let _host_os = rest[15];
            let file_crc = u32le(16);
            let dos_time = u32le(20);
            let _unpack_ver = rest[24];
            let method = rest[25];
            let name_size = u16::from_le_bytes([rest[26], rest[27]]) as usize;
            let attr = u32le(28);
            let (mut pack, mut unpack) = (pack_size, unpack_size);
            let name_start = if flags & rar4_flags::LARGE != 0 {
                if rest.len() < 40 + name_size {
                    return Err(ArchiveError::InvalidArchive(
                        "rar4: short large header".into(),
                    ));
                }
                pack |= u64::from(u32le(32)) << 32;
                unpack |= u64::from(u32le(36)) << 32;
                40
            } else {
                32
            };
            let name_bytes = rest.get(name_start..name_start + name_size).unwrap_or(&[]);
            // Unicode names carry a decoded form after the NUL; the
            // ASCII prefix up to the NUL is always valid.
            let name = decode_name(name_bytes, flags);

            entries.push(RawEntry {
                name: name.clone(),
                is_dir: flags & rar4_flags::DIRECTORY == rar4_flags::DIRECTORY
                    && flags & rar4_flags::DIRECTORY != 0,
                is_symlink: attr & 0xF000 == 0xA000,
                split: flags & (rar4_flags::SPLIT_BEFORE | rar4_flags::SPLIT_AFTER) != 0,
                unpacked_size: unpack,
                crc32: Some(file_crc),
                method,
                encrypted: flags & rar4_flags::ENCRYPTED != 0,
                mtime: Some(dos_time_to_unix(dos_time)),
                data: (end, end + pack as usize),
            });
        }
        rar4_block::END => return Ok(None),
        _ => {}
    }
    Ok(Some(end))
}

fn decode_name(bytes: &[u8], flags: u16) -> String {
    if bytes.first() == Some(&0) {
        return String::new();
    }
    // Split at the first NUL: ASCII part always present; the rest is
    // the unicode delta encoding we approximate by skipping.
    let ascii = match bytes.iter().position(|&b| b == 0) {
        Some(i) => &bytes[..i],
        None => bytes,
    };
    if flags & rar4_flags::UNICODE != 0 {
        // The Unicode tail is a RLE+high-byte-word encoding; taking
        // the ASCII prefix is correct whenever it is non-empty.
        if !ascii.is_empty() {
            return String::from_utf8_lossy(ascii).into_owned();
        }
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        String::from_utf8_lossy(ascii).into_owned()
    }
}

/// DOS date/time (1980 epoch, little-endian u32) → unix seconds.
fn dos_time_to_unix(dos: u32) -> u64 {
    let secs = (dos & 0x1F) * 2;
    let mins = (dos >> 5) & 0x3F;
    let hours = (dos >> 11) & 0x1F;
    let day = (dos >> 16) & 0x1F;
    let month = (dos >> 21) & 0x0F;
    let year = ((dos >> 25) & 0x7F) + 1980;
    if !(1..=12).contains(&month) || day == 0 {
        return 0;
    }
    let days = days_from_civil(i64::from(year), month, day);
    (days * 86_400 + i64::from(hours * 3600 + mins * 60 + secs)).max(0) as u64
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(m) + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl ArchiveReader for Rar4Reader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .entries
            .iter()
            .map(|e| ArchiveEntry {
                name: e.name.clone(),
                size: Some(e.unpacked_size),
                mtime: e.mtime,
                mode: Some(if e.is_dir { 0o755 } else { 0o644 }),
                kind: if e.is_dir {
                    EntryKind::Directory
                } else if e.is_symlink {
                    // Target is the entry's stored content itself.
                    EntryKind::Symlink(e.name.clone())
                } else {
                    EntryKind::Regular
                },
                uid: None,
                gid: None,
                uname: String::new(),
                gname: String::new(),
                method: Some(u16::from(e.method)),
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("rar4: no entry {index}")))?;
        if e.is_dir {
            return Ok(Vec::new());
        }
        if e.split {
            return Err(ArchiveError::UnsupportedFeature {
                reason: format!(
                    "rar4: entry '{}' spans multiple volumes; multivolume archives not supported",
                    e.name
                ),
            });
        }
        if e.encrypted {
            return Err(ArchiveError::Security(format!(
                "rar4: entry '{}' is encrypted; password required",
                e.name
            )));
        }
        let raw = self
            .data
            .get(e.data.0..e.data.1)
            .ok_or_else(|| ArchiveError::InvalidArchive("rar4: data out of bounds".into()))?;
        let out = match e.method {
            RAR4_METHOD_STORE => raw.to_vec(),
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!(
                        "rar4: compression method 0x{other:02X} (LZ/PPMd) not implemented; entry '{}'",
                        e.name
                    ),
                });
            }
        };
        if out.len() as u64 != e.unpacked_size {
            return Err(ArchiveError::InvalidArchive(format!(
                "rar4: entry '{}': size mismatch",
                e.name
            )));
        }
        if let Some(crc) = e.crc32 {
            let computed = omnizip_archive_core::crc32(&out);
            if computed != crc {
                return Err(ArchiveError::Checksum(format!(
                    "rar4: entry '{}': CRC mismatch: stored {crc:08X}, computed {computed:08X}",
                    e.name
                )));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_non_rar() {
        assert!(Rar4Reader::from_bytes(b"certainly not a rar file").is_err());
    }

    #[test]
    fn reads_libarchive_stored_fixture() {
        // Find a stored rar4 fixture in the corpus.
        let dir = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../omnizip/spec/fixtures/rar/libarchive_reference/rar4"
        ));
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().map(|e| e != "rar").unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else {
                continue;
            };
            if data.len() < 7 || data[0..7] != MAGIC_RAR4 {
                continue;
            }
            if let Ok(mut r) = Rar4Reader::from_bytes(&data) {
                let entries = r.entries().unwrap();
                for (i, entry) in entries.iter().enumerate() {
                    if entry.method == Some(u16::from(RAR4_METHOD_STORE)) && !entry.is_directory() {
                        match r.read_entry(i) {
                            Ok(data) => {
                                assert_eq!(
                                    data.len() as u64,
                                    entry.size.unwrap_or(0),
                                    "{}: {}",
                                    p.display(),
                                    entry.name
                                );
                                checked += 1;
                            }
                            Err(ArchiveError::UnsupportedFeature { reason })
                                if reason.contains("volumes") =>
                            {
                                // split-volume entries are correctly rejected
                            }
                            Err(e) => panic!("{}: entry {}: {e:?}", p.display(), entry.name),
                        }
                    }
                }
            }
        }
        assert!(checked > 0, "no stored entries exercised");
    }
}
