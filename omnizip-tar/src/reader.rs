//! TAR reader — port of `omnizip/formats/tar/reader.rb`, plus GNU
//! long-name/long-link and pax extended-header consumption.
#![forbid(unsafe_code)]

use crate::header::{padding_len, parse, to_entry};
use crate::{HEADER_SIZE, TYPE_GNU_LONGLINK, TYPE_GNU_LONGNAME, TYPE_PAX_EXTENDED};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader};
use std::path::Path;

/// Reads a TAR archive held in memory (the Ruby reader reads the file
/// eagerly into entries; same shape).
pub struct TarReader {
    data: Vec<u8>,
    entries: Vec<ArchiveEntry>,
    /// (offset, len) of each entry's data in `data`.
    spans: Vec<(usize, usize)>,
}

impl TarReader {
    /// Parse a TAR from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on header checksum or
    /// structure errors.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let mut entries = Vec::new();
        let mut spans = Vec::new();
        let mut cursor = 0usize;

        // Pending overrides from GNU 'L'/'K' and pax 'x' headers.
        let mut pending_long_name: Option<String> = None;
        let mut pending_long_link: Option<String> = None;
        let mut pending_pax_path: Option<String> = None;

        loop {
            let header_data = match data.get(cursor..cursor + HEADER_SIZE) {
                Some(h) => h,
                None => break,
            };
            let raw = match parse(header_data)? {
                Some(raw) => raw,
                None => break, // end-of-archive marker
            };
            cursor += HEADER_SIZE;

            let data_len = raw.size as usize;
            let padded = data_len + padding_len(data_len);
            let body = data
                .get(cursor..cursor + padded)
                .ok_or_else(|| ArchiveError::InvalidArchive("truncated entry body".into()))?;

            match raw.typeflag {
                TYPE_GNU_LONGNAME | TYPE_GNU_LONGLINK => {
                    let text = String::from_utf8_lossy(
                        &body[..body.iter().position(|&b| b == 0).unwrap_or(body.len())],
                    )
                    .into_owned();
                    if raw.typeflag == TYPE_GNU_LONGNAME {
                        pending_long_name = Some(text);
                    } else {
                        pending_long_link = Some(text);
                    }
                }
                TYPE_PAX_EXTENDED => {
                    // pax records: "<len> <key>=<value>\n" repeated.
                    let text = String::from_utf8_lossy(&body[..data_len]).into_owned();
                    for (key, value) in parse_pax_records(&text) {
                        match key.as_str() {
                            "path" => pending_pax_path = Some(value),
                            "linkpath" => pending_long_link = Some(value),
                            _ => {}
                        }
                    }
                }
                flag => {
                    let mut entry = to_entry(&raw);
                    if let Some(name) = pending_long_name.take() {
                        entry.name = name;
                        if matches!(entry.kind, omnizip_archive_core::EntryKind::Directory)
                            && !entry.name.ends_with('/')
                        {
                            entry.name.push('/');
                        }
                    }
                    if let Some(path) = pending_pax_path.take() {
                        entry.name = path;
                    }
                    if let Some(link) = pending_long_link.take() {
                        entry.kind = match entry.kind {
                            omnizip_archive_core::EntryKind::Symlink(_) => {
                                omnizip_archive_core::EntryKind::Symlink(link)
                            }
                            omnizip_archive_core::EntryKind::HardLink(_) => {
                                omnizip_archive_core::EntryKind::HardLink(link)
                            }
                            other => other,
                        };
                    }
                    let _ = flag;
                    spans.push((cursor, data_len));
                    entries.push(entry);
                }
            }

            cursor += padded;
        }

        Ok(Self {
            data: data.to_vec(),
            entries,
            spans,
        })
    }

    /// Open a TAR file from disk.
    ///
    /// # Errors
    ///
    /// IO or archive structure errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }
}

fn parse_pax_records(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(space) = rest.find(' ') {
        let Ok(len) = rest[..space].trim().parse::<usize>() else {
            break;
        };
        if len == 0 || len > rest.len() {
            break;
        }
        let record = &rest[space + 1..len.min(rest.len())];
        if let Some(eq) = record.find('=') {
            let value = record[..record.len().saturating_sub(1)].to_string(); // strip \n
            out.push((record[..eq].to_string(), value));
        }
        rest = &rest[len..];
    }
    out
}

impl ArchiveReader for TarReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self.entries.clone())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let (offset, len) = self
            .spans
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("no entry {index}")))?;
        Ok(self.data[*offset..*offset + *len].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TarWriter;
    use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};

    #[test]
    fn round_trip_via_traits() {
        let opts = WriteOptions::deterministic();
        let mut w = TarWriter::new();
        w.add_directory(&NewEntry::directory("dir", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("dir/hello.txt", &opts),
            b"hello tar\n",
            &opts,
        )
        .unwrap();
        w.add_symlink(&NewEntry::symlink("dir/link", "../hello.txt", &opts), &opts)
            .unwrap();
        let bytes = w.finish_bytes().unwrap();

        let mut r = TarReader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "dir/");
        assert!(entries[0].is_directory());
        assert_eq!(entries[1].name, "dir/hello.txt");
        assert_eq!(r.read_entry(1).unwrap(), b"hello tar\n");
        assert_eq!(
            entries[2].kind,
            omnizip_archive_core::EntryKind::Symlink("../hello.txt".into())
        );
    }

    /// Regression: bsdtar/GNU tar write a `./` root entry for
    /// `tar -c -C dir .` archives; extraction must skip it, not abort
    /// the whole archive with a security error.
    #[test]
    fn extracts_tar_with_dot_root_entry() {
        fn header(name: &str, typeflag: u8, size: u32) -> [u8; 512] {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..108].copy_from_slice(b"0000755\0"); // mode
            h[108..116].copy_from_slice(b"0000000\0"); // uid
            h[116..124].copy_from_slice(b"0000000\0"); // gid
            h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
            h[136..148].copy_from_slice(b"00000000000\0"); // mtime
            h[148..156].copy_from_slice(b"        ");
            h[156] = typeflag;
            let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
            h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
            h
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header("./", b'5', 0)); // bsdtar root
        bytes.extend_from_slice(&header("./note.txt", b'0', 6));
        bytes.extend_from_slice(b"hello\n");
        bytes.extend_from_slice(&[0u8; 1024]); // two zero blocks

        let mut r = TarReader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        assert_eq!(entries.len(), 2);
        let dir = std::env::temp_dir().join(format!("omnizip-tar-dotroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        r.extract_to(
            &dir,
            &omnizip_archive_core::security::SecurityPolicy::default(),
        )
        .expect("extract archive containing ./ root entry");
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_gnu_long_names() {
        let long_name = format!("deep/{}", "x".repeat(120));
        let opts = WriteOptions::deterministic();
        let mut w = TarWriter::new();
        w.add_file(&NewEntry::file(&long_name, &opts), b"data", &opts)
            .unwrap();
        let bytes = w.finish_bytes().unwrap();
        let mut r = TarReader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        assert_eq!(entries[0].name, long_name);
        assert_eq!(r.read_entry(0).unwrap(), b"data");
    }
}
