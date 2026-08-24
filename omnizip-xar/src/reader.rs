//! XAR reader — header, zlib TOC (quick-xml parse), heap access with
//! per-file decompression and SHA-1 checksum verification.
#![forbid(unsafe_code)]

use crate::toc::{self, Toc};
use crate::{parse_header, zlib_decompress, ENCODING_GZIP, ENCODING_NONE, HEADER_SIZE};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

/// Reads a XAR archive held in memory.
pub struct XarReader {
    data: Vec<u8>,
    pub toc: Toc,
    heap_start: usize,
}

impl XarReader {
    /// Parse a XAR from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on header, TOC, or checksum problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let header = parse_header(data)?;
        let toc_start = HEADER_SIZE;
        let toc_end = toc_start + header.toc_compressed_size as usize;
        let compressed = data
            .get(toc_start..toc_end)
            .ok_or_else(|| ArchiveError::InvalidArchive("xar: TOC out of bounds".into()))?;
        let xml = zlib_decompress(compressed)?;
        if xml.len() as u64 != header.toc_uncompressed_size {
            return Err(ArchiveError::InvalidArchive(format!(
                "xar: TOC size mismatch: header says {}, got {}",
                header.toc_uncompressed_size,
                xml.len()
            )));
        }
        let toc = toc::parse_toc(&xml)
            .map_err(|e| ArchiveError::InvalidArchive(format!("xar: TOC XML: {e}")))?;

        // TOC checksum sits right after the compressed TOC; verify it
        // when the style says SHA-1.
        let after_toc = toc_end;
        if toc.checksum.0 == "sha1" {
            let (offset, size) = (toc.checksum.1 as usize, toc.checksum.2 as usize);
            if size == 20 {
                // XAR's TOC-checksum offset counts from the heap start,
                // which is where the checksum itself begins.
                let stored = data
                    .get(after_toc + offset..after_toc + offset + size)
                    .ok_or_else(|| {
                        ArchiveError::InvalidArchive("xar: TOC checksum missing".into())
                    })?;
                let computed = omnizip_crypto::sha1(compressed);
                if stored != computed {
                    return Err(ArchiveError::Checksum("xar: TOC checksum mismatch".into()));
                }
            }
        }

        // Heap offsets (including the checksum's own offset 0) count
        // from the byte after the compressed TOC.
        Ok(Self {
            data: data.to_vec(),
            toc,
            heap_start: after_toc,
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

    /// Raw archived bytes for one entry (by TOC index).
    fn raw_entry(&self, index: usize) -> Result<(Vec<u8>, &toc::TocData), ArchiveError> {
        let entry = self
            .toc
            .entries
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("xar: no entry {index}")))?;
        let data = entry.data.as_ref().ok_or_else(|| {
            ArchiveError::InvalidArchive(format!("xar: entry '{}' has no data", entry.name))
        })?;
        let start = self.heap_start + data.offset as usize;
        let raw = self
            .data
            .get(start..start + data.length as usize)
            .ok_or_else(|| ArchiveError::InvalidArchive("xar: heap data out of bounds".into()))?
            .to_vec();
        Ok((raw, data))
    }
}

impl ArchiveReader for XarReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .toc
            .entries
            .iter()
            .map(|e| ArchiveEntry {
                name: e.name.clone(),
                size: e.size,
                mtime: e.mtime.map(|t| t as u64),
                mode: e.mode,
                kind: match e.kind.as_str() {
                    "directory" => EntryKind::Directory,
                    "symlink" => EntryKind::Symlink(
                        e.link.as_ref().map(|(_, t)| t.clone()).unwrap_or_default(),
                    ),
                    _ => EntryKind::Regular,
                },
                uid: e.uid.map(|x| x as u32),
                gid: e.gid.map(|x| x as u32),
                uname: String::new(),
                gname: String::new(),
                method: None,
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let (raw, data) = self.raw_entry(index)?;

        // Archived checksum over the stored bytes.
        if let Some(expected) = &data.archived_checksum {
            let computed = to_hex(&omnizip_crypto::sha1(&raw));
            if !expected.eq_ignore_ascii_case(&computed) {
                return Err(ArchiveError::Checksum(format!(
                    "xar: entry archived-checksum mismatch: expected {expected}, got {computed}"
                )));
            }
        }

        let out = match data.encoding.as_str() {
            "" | ENCODING_NONE => raw,
            ENCODING_GZIP => zlib_decompress(&raw)?,
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("xar: encoding '{other}' not supported"),
                });
            }
        };

        // Extracted checksum over the plain bytes.
        if let Some(expected) = &data.extracted_checksum {
            let computed = to_hex(&omnizip_crypto::sha1(&out));
            if !expected.eq_ignore_ascii_case(&computed) {
                return Err(ArchiveError::Checksum(format!(
                    "xar: entry extracted-checksum mismatch: expected {expected}, got {computed}"
                )));
            }
        }
        if data.size != 0 && out.len() as u64 != data.size {
            return Err(ArchiveError::InvalidArchive(format!(
                "xar: entry size mismatch: TOC says {}, got {}",
                data.size,
                out.len()
            )));
        }
        Ok(out)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_xar() {
        assert!(XarReader::from_bytes(b"not a xar archive at all").is_err());
    }
}
