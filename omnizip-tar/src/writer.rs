//! TAR writer — port of `omnizip/formats/tar/writer.rb` on the
//! [`ArchiveWriter`] trait, with deterministic normalization from
//! [`WriteOptions`] (task 17) instead of the Ruby's `Time.now`
//! defaults.
#![forbid(unsafe_code)]

use crate::header::{build, padding_len};
use crate::{
    BLOCK_SIZE, TYPE_DIRECTORY, TYPE_GNU_LONGNAME, TYPE_PAX_EXTENDED, TYPE_REGULAR, TYPE_SYMLINK,
};
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry, WriteOptions};

/// Builds an in-memory TAR.
pub struct TarWriter {
    out: Vec<u8>,
    finished: bool,
}

impl TarWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            finished: false,
        }
    }

    /// Finish and return the archive bytes (convenience over
    /// [`ArchiveWriter::finish`] + [`Self::into_bytes`]).
    ///
    /// # Errors
    ///
    /// As [`ArchiveWriter::finish`].
    pub fn finish_bytes(&mut self) -> Result<Vec<u8>, ArchiveError> {
        self.finish()?;
        Ok(std::mem::take(&mut self.out))
    }
}

impl Default for TarWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveWriter for TarWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.write_named(&entry.name, TYPE_REGULAR, "", data, entry, options)
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let mut name = entry.name.clone();
        if !name.ends_with('/') {
            name.push('/');
        }
        self.write_named(&name, TYPE_DIRECTORY, "", &[], entry, options)
    }

    fn add_symlink(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let target = match &entry.kind {
            EntryKind::Symlink(t) => t.clone(),
            _ => {
                return Err(ArchiveError::InvalidArchive(
                    "add_symlink expects a Symlink entry".into(),
                ));
            }
        };
        self.write_named(&entry.name, TYPE_SYMLINK, &target, &[], entry, options)
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        if self.finished {
            return Ok(());
        }
        // Two zero blocks mark the end of the archive.
        self.out.extend_from_slice(&[0u8; BLOCK_SIZE * 2]);
        self.finished = true;
        Ok(())
    }
}

impl TarWriter {
    fn write_named(
        &mut self,
        name: &str,
        typeflag: u8,
        linkname: &str,
        data: &[u8],
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let mtime = if options.mtime == 0 {
            entry.mtime
        } else {
            options.mtime
        };

        // Names that cannot split into the ustar prefix/name fields
        // go out as a GNU 'L' long-name entry followed by the real
        // header with a truncated name.
        let fits = name.len() <= 100 || {
            let bytes = name.as_bytes();
            bytes.len() <= 256
                && bytes[..bytes.len() - 1]
                    .iter()
                    .rposition(|&b| b == b'/')
                    .is_some_and(|i| i <= 155 && bytes.len() - i - 1 <= 100)
        };
        let mut short_name = String::new();
        if !fits {
            let long_data = {
                let mut v = name.as_bytes().to_vec();
                v.push(0);
                v
            };
            let header = build(
                "././@LongLink",
                0o644,
                0,
                0,
                long_data.len() as u64,
                0,
                TYPE_GNU_LONGNAME,
                "",
            );
            self.out.extend_from_slice(&header);
            self.out.extend_from_slice(&long_data);
            let pad = padding_len(long_data.len());
            self.out.extend(std::iter::repeat(0).take(pad));
            short_name = name.chars().take(99).collect();
        }

        let header_name = if short_name.is_empty() {
            name
        } else {
            &short_name
        };
        let header = build(
            header_name,
            entry.mode,
            entry.uid,
            entry.gid,
            data.len() as u64,
            mtime,
            typeflag,
            linkname,
        );
        self.out.extend_from_slice(&header);
        if !data.is_empty() {
            self.out.extend_from_slice(data);
            let pad = padding_len(data.len());
            self.out.extend(std::iter::repeat(0).take(pad));
        }
        let _ = TYPE_PAX_EXTENDED;
        Ok(())
    }
}
