//! ZIP reader — locate EOCD (with ZIP64), parse the central
//! directory, extract per-entry data with CRC verification.
#![forbid(unsafe_code)]

use crate::{
    CENTRAL_SIG, EOCD_SIG, LOCAL_SIG, METHOD_BZIP2, METHOD_DEFLATE, METHOD_STORE, METHOD_ZSTD,
    ZIP64_EOCD_SIG, ZIP64_EXTRA_TAG, ZIP64_LOCATOR_SIG,
};
use omnizip_archive_core::crc32;
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader};
use std::path::Path;

struct StoredEntry {
    entry: ArchiveEntry,
    method: u16,
    aes: Option<crate::aes::AesInfo>,
    crc32: u32,
    compressed_size: u64,
    local_offset: u64,
}

/// Reads a ZIP archive held in memory.
pub struct ZipReader {
    data: Vec<u8>,
    entries: Vec<StoredEntry>,
    password: Option<String>,
}

impl ZipReader {
    /// Parse a ZIP from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on a missing EOCD or a
    /// malformed central directory.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let eocd_at = find_eocd(data).ok_or_else(|| {
            ArchiveError::InvalidArchive("end of central directory not found".into())
        })?;
        let disk = u16::from_le_bytes([data[eocd_at + 4], data[eocd_at + 5]]);
        let _ = disk;
        let n_entries = u16::from_le_bytes([data[eocd_at + 10], data[eocd_at + 11]]) as usize;
        let cd_size =
            u32::from_le_bytes(data[eocd_at + 12..eocd_at + 16].try_into().expect("4")) as u64;
        let cd_offset =
            u32::from_le_bytes(data[eocd_at + 16..eocd_at + 20].try_into().expect("4")) as u64;

        // ZIP64: locator sits just before the EOCD.
        let mut n64 = n_entries as u64;
        let mut cd_start = cd_offset;
        if eocd_at >= 20 {
            let loc = &data[eocd_at - 20..eocd_at];
            if u32::from_le_bytes(loc[0..4].try_into().expect("4")) == ZIP64_LOCATOR_SIG {
                let z64_off = u64::from_le_bytes(loc[8..16].try_into().expect("8")) as usize;
                let z = &data[z64_off..];
                if u32::from_le_bytes(z[0..4].try_into().expect("4")) == ZIP64_EOCD_SIG {
                    n64 = u64::from_le_bytes(z[32..40].try_into().expect("8"));
                    cd_start = u64::from_le_bytes(z[48..56].try_into().expect("8"));
                }
            }
        }
        let _ = cd_size;

        let mut entries = Vec::with_capacity(n64.min(1 << 20) as usize);
        let mut pos = cd_start as usize;
        for _ in 0..n64 {
            let rec = data.get(pos..pos + 46).ok_or_else(|| {
                ArchiveError::InvalidArchive("truncated central directory record".into())
            })?;
            if u32::from_le_bytes(rec[0..4].try_into().expect("4")) != CENTRAL_SIG {
                return Err(ArchiveError::InvalidArchive(
                    "bad central directory signature".into(),
                ));
            }
            let method = u16::from_le_bytes([rec[10], rec[11]]);
            let mtime = u32::from_le_bytes(rec[12..16].try_into().expect("4"));
            let crc = u32::from_le_bytes(rec[16..20].try_into().expect("4"));
            let mut csize = u32::from_le_bytes(rec[20..24].try_into().expect("4")) as u64;
            let mut usize_ = u32::from_le_bytes(rec[24..28].try_into().expect("4")) as u64;
            let name_len = u16::from_le_bytes([rec[28], rec[29]]) as usize;
            let extra_len = u16::from_le_bytes([rec[30], rec[31]]) as usize;
            let comment_len = u16::from_le_bytes([rec[32], rec[33]]) as usize;
            let ext_attrs = u32::from_le_bytes(rec[38..42].try_into().expect("4"));
            let mut local_off = u32::from_le_bytes(rec[42..46].try_into().expect("4")) as u64;

            let rest = data
                .get(pos + 46..pos + 46 + name_len + extra_len)
                .ok_or_else(|| {
                    ArchiveError::InvalidArchive("truncated central directory name".into())
                })?;
            let name = String::from_utf8_lossy(&rest[..name_len]).into_owned();

            // ZIP64 extra field overrides saturated values; the
            // WinZip AES extra carries strength + real method.
            let mut aes = None;
            let mut off = name_len;
            while off + 4 <= name_len + extra_len {
                let tag = u16::from_le_bytes([rest[off], rest[off + 1]]);
                let sz = u16::from_le_bytes([rest[off + 2], rest[off + 3]]) as usize;
                if tag == crate::aes::AES_EXTRA_TAG {
                    if let Some(body) = rest.get(off + 4..off + 4 + sz) {
                        aes = crate::aes::parse_extra(body).ok();
                    }
                }
                if tag == ZIP64_EXTRA_TAG {
                    let body = &rest[off + 4..(off + 4 + sz).min(rest.len())];
                    let mut b = 0usize;
                    if usize_ == u32::MAX as u64 && b + 8 <= body.len() {
                        usize_ = u64::from_le_bytes(body[b..b + 8].try_into().expect("8"));
                        b += 8;
                    }
                    if csize == u32::MAX as u64 && b + 8 <= body.len() {
                        csize = u64::from_le_bytes(body[b..b + 8].try_into().expect("8"));
                        b += 8;
                    }
                    if local_off == u32::MAX as u64 && b + 8 <= body.len() {
                        local_off = u64::from_le_bytes(body[b..b + 8].try_into().expect("8"));
                    }
                }
                off += 4 + sz;
            }

            let is_dir = name.ends_with('/');
            let unix_mode = (ext_attrs >> 16) & 0o7777;
            let is_symlink = (ext_attrs >> 16) & 0o170000 == 0o120000;
            let kind = if is_symlink {
                // Target read lazily via the entry body.
                omnizip_archive_core::EntryKind::Symlink(String::new())
            } else if is_dir {
                omnizip_archive_core::EntryKind::Directory
            } else {
                omnizip_archive_core::EntryKind::Regular
            };

            entries.push(StoredEntry {
                entry: ArchiveEntry {
                    name,
                    size: Some(usize_),
                    mtime: Some(u64::from(mtime)),
                    mode: Some(unix_mode),
                    kind,
                    uid: None,
                    gid: None,
                    uname: String::new(),
                    gname: String::new(),
                    method: Some(aes.map(|a| a.real_method).unwrap_or(method)),
                },
                method,
                aes,
                crc32: crc,
                compressed_size: csize,
                local_offset: local_off,
            });
            pos += 46 + name_len + extra_len + comment_len;
        }

        Ok(Self {
            data: data.to_vec(),
            entries,
            password: None,
        })
    }

    /// Supply the password for WinZip-AES (method 99) entries.
    pub fn set_password(&mut self, password: &str) {
        self.password = Some(password.to_string());
    }

    /// Open a ZIP file from disk.
    ///
    /// # Errors
    ///
    /// IO or archive structure errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let tail_start = data.len().saturating_sub(22 + u16::MAX as usize);
    let mut i = data.len() - 22;
    loop {
        if u32::from_le_bytes(data[i..i + 4].try_into().expect("4")) == EOCD_SIG {
            return Some(i);
        }
        if i == tail_start {
            return None;
        }
        i -= 1;
    }
}

impl ZipReader {
    fn raw_entry(&self, index: usize) -> Result<(usize, usize, u16), ArchiveError> {
        let st = self
            .entries
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("no entry {index}")))?;
        let off = st.local_offset as usize;
        let lh = self
            .data
            .get(off..off + 30)
            .ok_or_else(|| ArchiveError::InvalidArchive("truncated local file header".into()))?;
        if u32::from_le_bytes(lh[0..4].try_into().expect("4")) != LOCAL_SIG {
            return Err(ArchiveError::InvalidArchive(
                "bad local header signature".into(),
            ));
        }
        let name_len = u16::from_le_bytes([lh[26], lh[27]]) as usize;
        let extra_len = u16::from_le_bytes([lh[28], lh[29]]) as usize;
        let data_start = off + 30 + name_len + extra_len;
        // Data-descriptor streams (flag bit 3) store sizes only in the
        // descriptor; fall back to the central directory value.
        let flags = u16::from_le_bytes([lh[6], lh[7]]);
        let csize = if flags & 0x0008 != 0 {
            st.compressed_size as usize
        } else {
            u32::from_le_bytes(lh[18..22].try_into().expect("4")) as usize
        };
        Ok((data_start, csize, st.method))
    }
}

impl ArchiveReader for ZipReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        // Symlink targets live in the entry body; resolve them now so
        // the unified model carries them.
        for i in 0..self.entries.len() {
            let needs_target = matches!(
                &self.entries[i].entry.kind,
                omnizip_archive_core::EntryKind::Symlink(t) if t.is_empty()
            );
            if needs_target {
                if let Ok(body) = self.read_entry(i) {
                    if let omnizip_archive_core::EntryKind::Symlink(ref mut empty) =
                        self.entries[i].entry.kind
                    {
                        *empty = String::from_utf8_lossy(&body).into_owned();
                    }
                }
            }
        }
        Ok(self.entries.iter().map(|s| s.entry.clone()).collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let (data_start, csize, method) = self.raw_entry(index)?;
        let raw = self
            .data
            .get(data_start..data_start + csize)
            .ok_or_else(|| ArchiveError::InvalidArchive("truncated entry data".into()))?;

        // WinZip AES: decrypt + authenticate, then decompress the
        // inner method. Wrong passwords fail on the verification
        // bytes (never on padding).
        let (method, buffer): (u16, Vec<u8>) = if method == crate::aes::METHOD_AES {
            let info = self.entries[index]
                .aes
                .ok_or_else(|| ArchiveError::InvalidArchive(
                    "AES entry missing the 0x9901 extra field".into(),
                ))?;
            let password = self.password.as_deref().ok_or_else(|| {
                ArchiveError::Security(format!(
                    "entry '{}' is WinZip-AES encrypted; supply a password",
                    self.entries[index].entry.name
                ))
            })?;
            let plain = crate::aes::decrypt(
                password.as_bytes(),
                raw,
                info.strength,
                &self.entries[index].entry.name,
            )?;
            (info.real_method, plain)
        } else {
            (method, raw.to_vec())
        };
        let raw: &[u8] = &buffer;

        let out = match method {
            METHOD_STORE => raw.to_vec(),
            METHOD_DEFLATE => {
                let mut hint = (raw.len() * 6).max(64);
                loop {
                    match omnizip_libdeflate::inflate::inflate(raw, hint) {
                        Ok(d) => break d,
                        Err(_) if hint < (1 << 32) => hint = hint.saturating_mul(4),
                        Err(e) => {
                            return Err(ArchiveError::InvalidArchive(format!("inflate: {e}")));
                        }
                    }
                }
            }
            METHOD_BZIP2 => omnizip_bzip2::decompress_framed(raw)
                .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2: {e}")))?,
            METHOD_ZSTD => omnizip_zstd::decompress(raw, u32::MAX)
                .map_err(|e| ArchiveError::InvalidArchive(format!("zstd: {e}")))?,
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("compression method {other} not supported"),
                });
            }
        };

        let st = &self.entries[index];
        let ae2 = st.aes.is_some_and(|a| a.version == 2);
        if !ae2 && st.crc32 != 0 && crc32(&out) != st.crc32 {
            return Err(ArchiveError::Checksum(format!(
                "entry '{}': stored {:08X}, computed {:08X}",
                st.entry.name,
                st.crc32,
                crc32(&out)
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        assert!(ZipReader::from_bytes(b"not a zip at all").is_err());
    }
}
