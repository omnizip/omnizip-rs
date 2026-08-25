//! XAR writer — deterministic archives: header + zlib-compressed
//! quick-xml TOC + SHA-1 TOC checksum + heap of zlib-compressed file
//! bodies (fixed creation-time from `WriteOptions`, sorted entries).
#![forbid(unsafe_code)]

use crate::toc::{self, Toc, TocData, TocEntry};
use crate::{zlib_compress, ENCODING_GZIP, HEADER_SIZE, MAGIC};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry};
use std::collections::BTreeMap;

/// Builds a deterministic XAR in memory.
pub struct XarWriter {
    files: BTreeMap<String, (NewEntry, Vec<u8>)>,
    dirs: BTreeMap<String, NewEntry>,
    symlinks: BTreeMap<String, (NewEntry, String)>,
}

impl XarWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
            symlinks: BTreeMap::new(),
        }
    }

    /// Serialize the archive.
    ///
    /// # Errors
    ///
    /// Compression or XML generation failures.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        // 1. Heap: the TOC checksum physically occupies heap offset 0
        //    (the <checksum> element's offset refers to it), so file
        //    bodies start at offset 20. The checksum bytes themselves
        //    are emitted between the compressed TOC and the heap.
        let mut heap: Vec<u8> = Vec::new();
        let mut entries: Vec<TocEntry> = Vec::new();
        let mut next_id = 1u64;

        let push_file = |name: &str,
                         body: &[u8],
                         heap: &mut Vec<u8>,
                         entries: &mut Vec<TocEntry>,
                         next_id: &mut u64,
                         mtime: u64,
                         mode: u32| {
            let archived = zlib_compress(body)?;
            let offset = 20 + heap.len() as u64;
            heap.extend_from_slice(&archived);
            entries.push(TocEntry {
                id: *next_id,
                name: name.to_string(),
                kind: "file".into(),
                mode: Some(mode),
                uid: Some(0),
                gid: Some(0),
                size: Some(body.len() as u64),
                mtime: Some(mtime as f64),
                data: Some(TocData {
                    offset,
                    length: archived.len() as u64,
                    size: body.len() as u64,
                    encoding: ENCODING_GZIP.into(),
                    archived_checksum: Some(hex(&omnizip_crypto::sha1(&archived))),
                    extracted_checksum: Some(hex(&omnizip_crypto::sha1(body))),
                }),
                link: None,
            });
            *next_id += 1;
            Ok::<(), ArchiveError>(())
        };

        for (name, (_, body)) in &self.files {
            push_file(
                name,
                body,
                &mut heap,
                &mut entries,
                &mut next_id,
                options.mtime,
                0o644,
            )?;
        }
        for (name, (_, target)) in &self.symlinks {
            let archived = target.as_bytes().to_vec();
            let offset = 20 + heap.len() as u64;
            heap.extend_from_slice(&archived);
            entries.push(TocEntry {
                id: next_id,
                name: name.clone(),
                kind: "symlink".into(),
                mode: Some(0o777),
                uid: Some(0),
                gid: Some(0),
                size: Some(archived.len() as u64),
                mtime: Some(options.mtime as f64),
                data: Some(TocData {
                    offset,
                    length: archived.len() as u64,
                    size: archived.len() as u64,
                    encoding: crate::ENCODING_NONE.into(),
                    archived_checksum: Some(hex(&omnizip_crypto::sha1(&archived))),
                    extracted_checksum: Some(hex(&omnizip_crypto::sha1(&archived))),
                }),
                link: Some(("symbolic".into(), target.clone())),
            });
            next_id += 1;
        }
        // Explicit directories plus every parent implied by a file
        // path — nesting resolves full paths on read.
        let mut dir_names: Vec<String> = self.dirs.keys().cloned().collect();
        for path in self.files.keys().chain(self.symlinks.keys()) {
            let mut p = path.as_str();
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                if !dir_names.iter().any(|d| d == p) {
                    dir_names.push(p.to_string());
                }
            }
        }
        dir_names.sort();
        for name in dir_names {
            entries.push(TocEntry {
                id: next_id,
                name: name.clone(),
                kind: "directory".into(),
                mode: Some(0o755),
                uid: Some(0),
                gid: Some(0),
                size: Some(0),
                mtime: Some(options.mtime as f64),
                data: None,
                link: None,
            });
            next_id += 1;
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        // 2. TOC with checksum placeholder, then fix up after
        // serializing (checksum occupies the 20 bytes after the TOC).
        let mut toc = Toc {
            creation_time: options.mtime as f64,
            checksum: ("sha1".into(), 0, 20),
            entries,
        };
        let xml = toc::write_toc(&toc)
            .map_err(|e| ArchiveError::InvalidArchive(format!("xar: TOC write: {e}")))?;
        let compressed = zlib_compress(&xml)?;
        let toc_checksum = omnizip_crypto::sha1(&compressed);

        // 3. Assemble: header (patched after lengths are known) +
        //    compressed TOC + checksum + heap.
        let mut out = Vec::with_capacity(HEADER_SIZE + compressed.len() + 20 + heap.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&(HEADER_SIZE as u16).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // version
        out.extend_from_slice(&(compressed.len() as u64).to_be_bytes());
        out.extend_from_slice(&(xml.len() as u64).to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes()); // cksum alg: sha1
        out.extend_from_slice(&compressed);
        out.extend_from_slice(&toc_checksum);
        out.extend_from_slice(&heap);
        let _ = &mut toc;
        Ok(out)
    }
}

impl Default for XarWriter {
    fn default() -> Self {
        Self::new()
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

impl ArchiveWriter for XarWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files
            .insert(entry.name.clone(), (entry.clone(), data.to_vec()));
        Ok(())
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.dirs.insert(entry.name.clone(), entry.clone());
        Ok(())
    }

    fn add_symlink(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let target = match &entry.kind {
            EntryKind::Symlink(t) => t.clone(),
            _ => {
                return Err(ArchiveError::InvalidArchive(
                    "add_symlink expects a Symlink entry".into(),
                ));
            }
        };
        self.symlinks
            .insert(entry.name.clone(), (entry.clone(), target));
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::XarReader;
    use omnizip_archive_core::ArchiveReader;

    fn build() -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = XarWriter::new();
        w.add_directory(&NewEntry::directory("docs", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("docs/readme.txt", &opts),
            b"xar round trip\n".repeat(30).as_slice(),
            &opts,
        )
        .unwrap();
        w.add_symlink(&NewEntry::symlink("docs/link", "readme.txt", &opts), &opts)
            .unwrap();
        w.finish_bytes(&opts).unwrap()
    }

    #[test]
    fn round_trip() {
        let bytes = build();
        let mut r = XarReader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["docs", "docs/link", "docs/readme.txt"]);
        let readme = names.iter().position(|n| *n == "docs/readme.txt").unwrap();
        assert_eq!(
            r.read_entry(readme).unwrap(),
            b"xar round trip\n".repeat(30)
        );
        assert_eq!(entries[1].kind, EntryKind::Symlink("readme.txt".into()));
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(), build());
    }
}
